//! Layer ordering, duplication and Alt-drag cloning over placed marks.

use super::history::HistoryEntry;
use super::{Annotations, Shape, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point, px};

/// Z-order operations on the currently selected marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerOrder {
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
}

/// Snapshot state needed to commit or roll back an in-flight Alt-drag clone.
pub(crate) struct CloneDrag {
    pub(crate) count: usize,
    pub(crate) previous_selected: Vec<usize>,
    pub(crate) number_before: Option<u32>,
}

const DUPLICATE_STEP: f32 = 12.;

impl Annotations {
    /// Reorder all selected marks within the stack while keeping their
    /// relative order and updating `selected` indices so repeated steps
    /// continue walking the same group.
    pub(crate) fn reorder_selected(&mut self, order: LayerOrder) -> bool {
        if self.selected.is_empty() || self.shapes.len() < 2 || self.text_original.is_some() {
            return false;
        }
        self.end_size_drag();
        let n = self.shapes.len();
        let is_sel = |ix: usize| self.selected.contains(&ix);
        let mut perm: Vec<usize> = (0..n).collect();
        match order {
            LayerOrder::BringToFront => {
                perm.sort_by_key(|&ix| is_sel(ix));
            }
            LayerOrder::SendToBack => {
                perm.sort_by_key(|&ix| !is_sel(ix));
            }
            LayerOrder::BringForward => {
                for i in (0..n - 1).rev() {
                    if is_sel(perm[i]) && !is_sel(perm[i + 1]) {
                        perm.swap(i, i + 1);
                    }
                }
            }
            LayerOrder::SendBackward => {
                for i in 1..n {
                    if is_sel(perm[i]) && !is_sel(perm[i - 1]) {
                        perm.swap(i - 1, i);
                    }
                }
            }
        }
        if perm.iter().copied().eq(0..n) {
            return false;
        }
        let before = self.shapes.clone();
        let mut new_index = vec![0; n];
        let mut after = Vec::with_capacity(n);
        for (new_ix, &old_ix) in perm.iter().enumerate() {
            new_index[old_ix] = new_ix;
            after.push(before[old_ix].clone());
        }
        self.shapes = after.clone();
        for sel in &mut self.selected {
            *sel = new_index[*sel];
        }
        self.history.push(HistoryEntry::Reorder { before, after });
        self.redo.clear();
        true
    }

    /// Duplicate every selected mark in layer order, offset inside `area`
    /// and advancing sequence badge numbers, recorded as a single undo step.
    pub(crate) fn duplicate_selected(&mut self, area: Bounds<Pixels>) -> bool {
        if self.selected.is_empty() || self.text_original.is_some() {
            return false;
        }
        self.end_size_drag();
        let mut ordered: Vec<usize> = self
            .selected
            .iter()
            .copied()
            .filter(|&ix| ix < self.shapes.len())
            .collect();
        if ordered.is_empty() {
            return false;
        }
        ordered.sort_unstable();
        ordered.dedup();
        let mut union: Option<Bounds<Pixels>> = None;
        for &ix in &ordered {
            let b = if self.shapes[ix].kind == ShapeKind::Text {
                super::select::inflate(&self.shapes[ix].ink_bounds(), px(2.))
            } else {
                self.shapes[ix].selection_box()
            };
            union = Some(match union {
                Some(acc) => acc.union(&b),
                None => b,
            });
        }
        let Some(bounds) = union else {
            return false;
        };
        let mut delta = clamp_offset(bounds, area, point(px(DUPLICATE_STEP), px(DUPLICATE_STEP)));
        if delta == point(px(0.), px(0.)) {
            delta = clamp_offset(
                bounds,
                area,
                point(px(-DUPLICATE_STEP), px(-DUPLICATE_STEP)),
            );
        }
        let number_before = self.number_override;
        let base_len = self.shapes.len();
        let mut added = Vec::with_capacity(ordered.len());
        let mut map = Vec::with_capacity(ordered.len());
        for (offset_ix, &old_ix) in ordered.iter().enumerate() {
            let mut copy = self.shapes[old_ix].clone();
            copy.translate(delta);
            if copy.kind == ShapeKind::Number {
                let value = self.next_number();
                copy.number = Some(value);
                if number_before.is_some() {
                    self.number_override = Some(value.saturating_add(1));
                }
                self.shapes.push(copy.clone());
            } else {
                self.shapes.push(copy.clone());
            }
            added.push(copy);
            map.push((old_ix, base_len + offset_ix));
        }
        self.selected = self
            .selected
            .iter()
            .filter_map(|old| map.iter().find(|(k, _)| k == old).map(|(_, new)| *new))
            .collect();
        self.history.push(HistoryEntry::AddMany {
            shapes: added,
            number_before,
            number_after: self.number_override,
        });
        self.redo.clear();
        true
    }

    /// Append uncommitted clones of the current selection for an Alt-drag
    /// gesture, returning the clone drag handle and press snapshots.
    pub(crate) fn begin_clone_drag(&mut self) -> Option<(CloneDrag, Vec<(usize, Shape)>)> {
        if self.selected.is_empty() || self.text_original.is_some() {
            return None;
        }
        self.end_size_drag();
        let previous_selected = self.selected.clone();
        let mut ordered: Vec<usize> = self
            .selected
            .iter()
            .copied()
            .filter(|&ix| ix < self.shapes.len())
            .collect();
        if ordered.is_empty() {
            return None;
        }
        ordered.sort_unstable();
        ordered.dedup();
        let number_before = self.number_override;
        let base_len = self.shapes.len();
        let mut map = Vec::with_capacity(ordered.len());
        for (offset_ix, &old_ix) in ordered.iter().enumerate() {
            let mut copy = self.shapes[old_ix].clone();
            if copy.kind == ShapeKind::Number {
                let value = self.next_number();
                copy.number = Some(value);
                if number_before.is_some() {
                    self.number_override = Some(value.saturating_add(1));
                }
            }
            self.shapes.push(copy);
            map.push((old_ix, base_len + offset_ix));
        }
        self.selected = previous_selected
            .iter()
            .filter_map(|old| map.iter().find(|(k, _)| k == old).map(|(_, new)| *new))
            .collect();
        let before: Vec<(usize, Shape)> = self
            .selected
            .iter()
            .filter_map(|&ix| self.shapes.get(ix).cloned().map(|s| (ix, s)))
            .collect();
        Some((
            CloneDrag {
                count: ordered.len(),
                previous_selected,
                number_before,
            },
            before,
        ))
    }

    /// Finalize an Alt-drag clone into one `AddMany` history entry.
    pub(crate) fn commit_clone_drag(&mut self, drag: CloneDrag) {
        let start = self.shapes.len().saturating_sub(drag.count);
        let shapes = self.shapes[start..].to_vec();
        if shapes.is_empty() {
            return;
        }
        self.history.push(HistoryEntry::AddMany {
            shapes,
            number_before: drag.number_before,
            number_after: self.number_override,
        });
        self.redo.clear();
    }

    /// Abort an uncommitted Alt-drag clone without touching undo/redo history.
    pub(crate) fn cancel_clone_drag(&mut self, drag: CloneDrag) {
        let keep = self.shapes.len().saturating_sub(drag.count);
        self.shapes.truncate(keep);
        self.number_override = drag.number_before;
        self.selected = drag.previous_selected;
    }
}

fn clamp_offset(
    bounds: Bounds<Pixels>,
    area: Bounds<Pixels>,
    delta: Point<Pixels>,
) -> Point<Pixels> {
    let axis = |start: Pixels, end: Pixels, low: Pixels, high: Pixels, d: Pixels| -> Pixels {
        if end - start > high - low {
            return px(0.);
        }
        let min = f32::from(low - start).min(0.);
        let max = f32::from(high - end).max(0.);
        px(f32::from(d).clamp(min, max))
    };
    point(
        axis(
            bounds.left(),
            bounds.right(),
            area.left(),
            area.right(),
            delta.x,
        ),
        axis(
            bounds.top(),
            bounds.bottom(),
            area.top(),
            area.bottom(),
            delta.y,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    fn area() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(300.), px(240.)))
    }

    fn rect(a: &mut Annotations, x: f32, y: f32, color_ix: usize) {
        a.toggle(ShapeKind::Rectangle);
        a.set_color(color_ix);
        a.begin(point(px(x), px(y)), area(), false);
        a.drag_to(point(px(x + 40.), px(y + 30.)), area(), false);
        a.end();
        a.toggle(ShapeKind::Rectangle);
    }

    #[test]
    fn layer_ordering_walks_groups_preserves_selection_and_undoes() {
        let mut a = Annotations::default();
        rect(&mut a, 10., 10., 0);
        rect(&mut a, 20., 20., 1);
        rect(&mut a, 30., 30., 2);
        rect(&mut a, 40., 40., 3);
        let original = a.committed().to_vec();

        // Select indices [0, 2] (2 is primary) and bring forward by one slot -> [1, 3]
        a.select_indices([0, 2]);
        assert!(a.reorder_selected(LayerOrder::BringForward));
        assert_eq!(a.selected_indices(), &[1, 3]);
        assert_eq!(a.committed()[0], original[1]);
        assert_eq!(a.committed()[1], original[0]);
        assert_eq!(a.committed()[2], original[3]);
        assert_eq!(a.committed()[3], original[2]);

        // Bring to front -> [2, 3]; already at front next time -> no-op
        assert!(a.reorder_selected(LayerOrder::BringToFront));
        assert_eq!(a.selected_indices(), &[2, 3]);
        assert!(!a.reorder_selected(LayerOrder::BringToFront));

        // Send to back -> [0, 1]; already at back -> no-op
        assert!(a.reorder_selected(LayerOrder::SendToBack));
        assert_eq!(a.selected_indices(), &[0, 1]);
        assert!(!a.reorder_selected(LayerOrder::SendToBack));
        assert!(!a.reorder_selected(LayerOrder::SendBackward));

        a.undo();
        a.undo();
        a.undo();
        assert_eq!(a.committed(), original.as_slice());
        a.redo();
        assert_eq!(a.committed()[1], original[0]);
    }

    #[test]
    fn duplicate_offsets_clamps_advances_badges_and_undoes_as_one_step() {
        let mut a = Annotations::default();
        rect(&mut a, 20., 30., 0);
        a.toggle(ShapeKind::Number);
        a.begin(point(px(80.), px(60.)), area(), false);
        a.end();
        a.toggle(ShapeKind::Number);

        a.select_indices([0, 1]);
        let base_history = a.history.len();
        assert!(a.duplicate_selected(area()));
        assert_eq!(a.committed().len(), 4);
        assert_eq!(a.selected_indices(), &[2, 3]);
        assert_eq!(
            a.committed()[2].bounds.origin,
            a.committed()[0].bounds.origin + point(px(12.), px(12.))
        );
        assert_eq!(a.committed()[3].number, Some(2));
        assert_eq!(a.history.len(), base_history + 1);

        a.undo();
        assert_eq!(a.committed().len(), 2);
        assert_eq!(a.next_number(), 2);
        a.redo();
        assert_eq!(a.committed().len(), 4);
        assert_eq!(a.committed()[3].number, Some(2));
    }

    #[test]
    fn batch_color_style_and_size_edits_undo_as_single_steps_across_multi_selection() {
        let mut a = Annotations::default();
        rect(&mut a, 10., 10., 0);
        rect(&mut a, 70., 10., 0);
        a.toggle(ShapeKind::Arrow);
        a.begin(point(px(10.), px(90.)), area(), false);
        a.drag_to(point(px(110.), px(90.)), area(), false);
        a.end();
        a.toggle(ShapeKind::Arrow);

        a.select_indices([0, 1, 2]);
        let before = a.committed().to_vec();

        // Batch color update across all 3 marks -> 1 undo step
        let h = a.history.len();
        a.set_color(3);
        assert_eq!(a.history.len(), h + 1);
        let green = crate::model::settings::current().annotation_colors()[3];
        assert!(a.committed().iter().all(|s| s.color == green));

        // Batch slider drag across all 3 stroke-family marks -> 1 merged undo step
        for w in [5., 8., 11.] {
            a.apply_size(w);
        }
        a.end_size_drag();
        assert_eq!(a.history.len(), h + 2);
        assert!(a.committed().iter().all(|s| s.size() == 11.));

        // Batch line style across all 3 -> 1 undo step
        a.set_line_style(crate::annotation::LineStyle::Dashed);
        assert_eq!(a.history.len(), h + 3);
        assert!(
            a.committed()
                .iter()
                .all(|s| s.line_style() == crate::annotation::LineStyle::Dashed)
        );

        a.undo();
        a.undo();
        a.undo();
        assert_eq!(a.committed(), before.as_slice());
    }

    #[test]
    fn alt_clone_drag_commits_or_cancels_cleanly_and_active_swatch_tracks_selection() {
        let mut a = Annotations::default();
        rect(&mut a, 20., 20., 4); // Blue swatch 4
        rect(&mut a, 90., 20., 1); // Orange swatch 1

        // Selecting the first mark points `color_index` / `active_swatch_index` at 4, not 1.
        a.select_index(0);
        assert_eq!(a.color_index(), 4);
        assert_eq!(a.active_swatch_index(), Some(4));

        // Cancelled Alt-drag clone leaves history and redo untouched.
        let h = a.history.len();
        let (clone_drag, before) = a.begin_clone_drag().unwrap();
        a.place_shapes(&before, point(px(30.), px(25.)));
        assert_eq!(a.committed().len(), 3);
        a.cancel_clone_drag(clone_drag);
        assert_eq!(a.committed().len(), 2);
        assert_eq!(a.selected_indices(), &[0]);
        assert_eq!(a.history.len(), h);

        // Committed Alt-drag clone records one AddMany entry.
        let (clone_drag, before) = a.begin_clone_drag().unwrap();
        a.place_shapes(&before, point(px(40.), px(35.)));
        a.commit_clone_drag(clone_drag);
        assert_eq!(a.committed().len(), 3);
        assert_eq!(a.history.len(), h + 1);
        a.undo();
        assert_eq!(a.committed().len(), 2);
    }
}
