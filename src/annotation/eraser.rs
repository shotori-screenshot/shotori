//! The object eraser (issues #14, #41): whole-shape deletion by brush
//! sweep or rectangle. Unlike drawing tools it carries no draft shape
//! and never masks pixels — the gesture state is the sweep itself, and
//! its only output is whole-mark removals folded into one reversible
//! [`super::history::HistoryEntry::Remove`] step per gesture.

use super::{Annotations, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point, px};

/// An object-eraser gesture in flight (issues #14, #41).
#[derive(Clone, Copy)]
pub(super) enum EraserGesture {
    /// Brush sweep. `last` is the previous sample; the segment to the
    /// next one is interpolated so a fast flick cannot jump over thin
    /// ink between two pointer events.
    Stroke { last: Point<Pixels> },
    /// Area sweep: the dragged rectangle, committed on release (the
    /// bounds are only final when the pointer comes up).
    Rect {
        start: Point<Pixels>,
        end: Point<Pixels>,
    },
}

impl Annotations {
    /// The stroke eraser's brush radius — half the eraser tool's
    /// remembered size; the overlay's ring chrome renders the same
    /// circle, so what you see is what erases.
    pub(crate) fn erase_radius(&self) -> f32 {
        self.size_of(ShapeKind::Eraser) / 2.
    }

    /// The area eraser's in-flight rect in selection-global
    /// coordinates (the dashed chrome outline). `None` outside the
    /// gesture.
    pub(crate) fn eraser_rect_bounds(&self) -> Option<Bounds<Pixels>> {
        match self.eraser {
            Some(EraserGesture::Rect { start, end }) => Some(Bounds::from_corners(
                point(start.x.min(end.x), start.y.min(end.y)),
                point(start.x.max(end.x), start.y.max(end.y)),
            )),
            _ => None,
        }
    }

    /// One brush sample: delete every shape whose visible ink the
    /// circle at `p` touches. Removals happen live as the brush sweeps
    /// and fold into ONE [`super::history::HistoryEntry::Remove`] per
    /// gesture — opened on the first removal, extended afterwards — so
    /// a single Undo restores the entire sweep.
    pub(super) fn erase_sample(&mut self, p: Point<Pixels>) -> bool {
        let radius = self.erase_radius();
        let indices: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, shape)| super::select::shape_erased(shape, p, radius))
            .map(|(ix, _)| ix)
            .collect();
        self.remove_shapes(indices, true)
    }

    /// The area eraser's release: delete every shape whose visible ink
    /// bounds intersect the dragged rect, recorded as a single
    /// [`super::history::HistoryEntry::Remove`] step.
    pub(super) fn erase_rect(&mut self, area: Bounds<Pixels>) -> bool {
        if area.size.width <= px(0.) || area.size.height <= px(0.) {
            return false;
        }
        let indices: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, shape)| {
                let inter = shape.ink_bounds().intersect(&area);
                inter.size.width > px(0.) && inter.size.height > px(0.)
            })
            .map(|(ix, _)| ix)
            .collect();
        self.remove_shapes(indices, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::LensOption;
    use gpui_kit::size;

    fn selection() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(240.), px(180.)))
    }

    fn draw_rectangle(a: &mut Annotations, x0: f32, y0: f32, x1: f32, y1: f32) {
        a.toggle(ShapeKind::Rectangle);
        a.begin(point(px(x0), px(y0)), selection(), false);
        a.drag_to(point(px(x1), px(y1)), selection(), false);
        a.end();
        a.toggle(ShapeKind::Rectangle);
    }

    #[test]
    fn brush_eraser_deletes_whole_shapes_live_as_one_history_entry() {
        let mut a = Annotations::default();
        draw_rectangle(&mut a, 10., 10., 30., 40.);
        a.toggle(ShapeKind::Pencil);
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(40.)), selection(), false);
        a.end();
        a.toggle(ShapeKind::Line);
        a.begin(point(px(120.), px(120.)), selection(), false);
        a.drag_to(point(px(200.), px(120.)), selection(), false);
        a.end();
        let before = a.committed().to_vec();
        assert_eq!(before.len(), 3);

        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(24.);
        let history_len = a.history.len();
        assert!(!a.parks_click_select());
        // Press lands on the rectangle's right stroke band: deleted immediately.
        a.begin(point(px(28.), px(25.)), selection(), false);
        assert_eq!(
            a.committed().len(),
            2,
            "press itself deletes the touched mark"
        );
        // Drag crosses the diagonal pencil stroke; far line stays untouched.
        a.drag_to(point(px(50.), px(25.)), selection(), false);
        a.end();
        assert_eq!(a.committed(), &before[2..]);
        assert_eq!(
            a.history.len(),
            history_len + 1,
            "one history step per sweep"
        );

        a.undo();
        assert_eq!(a.committed(), before.as_slice());
        a.redo();
        assert_eq!(a.committed(), &before[2..]);
    }

    #[test]
    fn brush_radius_decides_touching_and_interpolation_covers_gaps() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Line);
        a.set_tool_size(2.); // 1 px half-width
        a.begin(point(px(10.), px(50.)), selection(), false);
        a.drag_to(point(px(70.), px(50.)), selection(), false);
        a.end();
        let history_len = a.history.len();

        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(16.); // radius 8 -> reaches 9 px from line center
        // Near-miss: beyond radius + half-width, leaves history untouched.
        a.begin(point(px(40.), px(60.5)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), history_len);

        // Fast flick whose endpoints both miss, but whose segment crosses the line.
        a.begin(point(px(40.), px(30.)), selection(), false);
        a.drag_to(point(px(40.), px(70.)), selection(), false);
        a.end();
        assert!(a.committed().is_empty());
        assert_eq!(a.history.len(), history_len + 1);
    }

    #[test]
    fn closed_polygon_interior_is_not_erasable_without_touching_ink() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Polyline);
        for (x, y) in [(10., 10.), (70., 10.), (70., 60.), (10., 60.), (10., 10.)] {
            let p = point(px(x), px(y));
            a.begin(p, selection(), false);
            a.drag_to(p, selection(), false);
            a.end();
        }
        a.finish_polyline();
        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(16.);

        // Sweeping inside the hollow ring must not delete it.
        a.begin(point(px(40.), px(35.)), selection(), false);
        a.drag_to(point(px(30.), px(35.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1);

        // Touching the outline deletes the whole polygon.
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.end();
        assert!(a.committed().is_empty());
    }

    #[test]
    fn rect_eraser_deletes_intersecting_ink_bounds_on_release_only() {
        let mut a = Annotations::default();
        draw_rectangle(&mut a, 10., 10., 30., 40.);
        a.toggle(ShapeKind::Line);
        a.begin(point(px(15.), px(55.)), selection(), false);
        a.drag_to(point(px(45.), px(55.)), selection(), false);
        a.end();
        a.toggle(ShapeKind::Text);
        a.add_text(
            Bounds::new(point(px(120.), px(120.)), size(px(80.), px(24.))),
            "far".into(),
        );
        let before = a.committed().to_vec();
        let history_len = a.history.len();

        a.toggle(ShapeKind::EraserRect);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(50.), px(65.)), selection(), false);
        // Nothing is deleted while the dashed area is still being dragged.
        assert_eq!(a.committed().len(), 3);
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.committed()[0].text.as_deref(), Some("far"));
        assert_eq!(a.history.len(), history_len + 1);

        // Zero-area click on a mark deletes nothing and records nothing.
        let h = a.history.len();
        a.begin(point(px(130.), px(130.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), h);

        // Uncommitted drag cancelled by undo leaves history intact.
        a.begin(point(px(110.), px(110.)), selection(), false);
        a.drag_to(point(px(210.), px(160.)), selection(), false);
        a.undo();
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), h);

        a.undo();
        assert_eq!(a.committed(), before.as_slice());
        a.redo();
        assert_eq!(a.committed().len(), 1);
    }

    #[test]
    fn mid_gesture_undo_reverts_sweep_without_merging_into_earlier_remove() {
        let mut a = Annotations::default();
        draw_rectangle(&mut a, 10., 10., 30., 40.);
        draw_rectangle(&mut a, 50., 10., 70., 40.);
        draw_rectangle(&mut a, 90., 10., 110., 40.);
        // Earlier standalone Delete creates a trailing HistoryEntry::Remove.
        a.delete_shape(0);
        let after_first_delete = a.committed().to_vec();
        assert_eq!(after_first_delete.len(), 2);
        let base_history = a.history.len();

        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(16.);
        // Press deletes the middle rectangle (now index 0).
        a.begin(point(px(50.), px(25.)), selection(), false);
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), base_history + 1);

        // Mid-sweep Undo reverts only the live sweep's removal, not the earlier Delete.
        a.undo();
        assert_eq!(a.committed(), after_first_delete.as_slice());
        assert_eq!(a.history.len(), base_history);

        // Continuing the sweep from x=50 to x=90 crosses both remaining rectangles
        // and opens a fresh Remove entry rather than merging into the old Delete.
        a.drag_to(point(px(90.), px(25.)), selection(), false);
        a.end();
        assert!(a.committed().is_empty());
        assert_eq!(a.history.len(), base_history + 1);
        a.undo();
        assert_eq!(a.committed(), after_first_delete.as_slice());
    }

    #[test]
    fn brush_sweep_renumbers_sequence_badges_and_restores_in_one_undo() {
        let mut a = Annotations {
            renumber_on_delete: true,
            ..Annotations::default()
        };
        a.toggle(ShapeKind::Number);
        for x in [30., 80., 130.] {
            a.begin(point(px(x), px(50.)), selection(), false);
            a.end();
        }
        let numbers = |a: &Annotations| -> Vec<u32> {
            a.committed().iter().filter_map(|s| s.number).collect()
        };
        assert_eq!(numbers(&a), vec![1, 2, 3]);

        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(20.);
        a.begin(point(px(30.), px(50.)), selection(), false);
        a.drag_to(point(px(80.), px(50.)), selection(), false);
        a.end();
        assert_eq!(numbers(&a), vec![1]);
        assert_eq!(a.next_number(), 2);

        a.undo();
        assert_eq!(numbers(&a), vec![1, 2, 3]);
        assert_eq!(a.next_number(), 4);
        a.redo();
        assert_eq!(numbers(&a), vec![1]);
    }

    #[test]
    fn brush_radius_touches_magnifier_and_spotlight_regions() {
        for ellipse in [false, true] {
            let mut a = Annotations::default();
            a.toggle(ShapeKind::Spotlight);
            if ellipse {
                a.set_spotlight_ellipse(true);
            }
            a.begin(point(px(60.), px(60.)), selection(), false);
            a.drag_to(point(px(120.), px(120.)), selection(), false);
            a.end();

            a.toggle(ShapeKind::Eraser);
            a.set_tool_size(24.); // radius 12
            // Brush center sits 8 px left of the spotlight edge (within radius 12).
            a.begin(point(px(52.), px(90.)), selection(), false);
            a.end();
            assert!(a.committed().is_empty(), "spotlight ellipse={ellipse}");

            a.toggle(ShapeKind::Magnifier);
            a.set_lens_option(LensOption::Ellipse, ellipse);
            a.begin(point(px(40.), px(40.)), selection(), false);
            a.drag_to(point(px(80.), px(80.)), selection(), false);
            a.end();
            assert_eq!(a.committed().len(), 1);

            a.toggle(ShapeKind::Eraser);
            a.set_tool_size(24.);
            a.begin(point(px(32.), px(60.)), selection(), false);
            a.end();
            assert!(a.committed().is_empty(), "magnifier ellipse={ellipse}");
        }
    }
}
