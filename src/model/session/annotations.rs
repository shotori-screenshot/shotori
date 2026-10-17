//! Annotation glue: the session-side half of the annotation tools —
//! routing pointer gestures into the shared [`Annotations`] state,
//! the text/badge edit entries, and the local-coordinate projections
//! the highlight layer renders from. The tools' own logic lives in
//! [`crate::annotation`]; everything here is session plumbing around
//! it.

use super::{CLICK_SLOP, EraserChrome, HandleDrag, Loupe, MoveDrag, ScreenshotSession};
use crate::model::selection::Selection;
use gpui_kit::*;

// One inset for placement, editing and movement; the outline is painted inside
// the text bounds, so no separate border outset may consume this breathing room.
const TEXT_INSET: f32 = 2.;
fn text_area(selection: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let inset = point(px(TEXT_INSET), px(TEXT_INSET));
    (selection.size.width > px(TEXT_INSET * 2.) && selection.size.height > px(TEXT_INSET * 2.))
        .then(|| Bounds::from_corners(selection.origin + inset, selection.bottom_right() - inset))
}

impl ScreenshotSession {
    pub(crate) fn edit_annotations(
        &mut self,
        edit: impl FnOnce(&mut crate::annotation::Annotations),
    ) {
        if !self.blocked && self.selection.is_selected() {
            edit(&mut self.annotations);
        }
    }

    pub(crate) fn edit_annotation_settings(
        &mut self,
        edit: impl FnOnce(&mut crate::annotation::Annotations),
    ) {
        if self.annotations.editing_text().is_some() {
            edit(&mut self.annotations);
        } else {
            self.edit_annotations(edit);
        }
    }

    pub(crate) fn preview_text(&mut self, bounds: Bounds<Pixels>, value: String) {
        self.annotations.preview_text(bounds, value);
    }
    pub(crate) fn begin_text_edit(&mut self, ix: usize) -> bool {
        if self.blocked || !self.selection.is_selected() || !self.annotations.begin_text_edit(ix) {
            return false;
        }
        self.blocked = true;
        true
    }

    pub(crate) fn finish_text_edit(&mut self, commit: bool) {
        self.annotations.finish_text_edit(commit);
        self.blocked = false;
    }

    pub(crate) fn text_bounds(
        &self,
        output: &str,
        local: Point<Pixels>,
        font_size: f32,
    ) -> Option<Bounds<Pixels>> {
        if self.blocked || !self.selection.is_selected() {
            return None;
        }
        let bounds = self.selection.bounds()?;
        let origin = local + self.screen(output).bounds().origin;
        if !bounds.contains(&origin) {
            return None;
        }
        let area = text_area(bounds)?;
        let origin = point(origin.x.max(area.left()), origin.y.max(area.top()));
        let available = Bounds::from_corners(origin, area.bottom_right());
        (origin.x <= area.right()
            && origin.y <= area.bottom()
            && available.size.width >= px(16.)
            && available.size.height >= px(font_size * 1.35))
        .then_some(available)
    }

    /// A double-click landing on a NUMBER badge: the shape index (for
    /// the overlay's value editor) plus the badge origin in the
    /// RECEIVING window's local coordinates (for the editor box).
    /// None for every other kind or place. The select tool gates it:
    /// a draw tool's first click would ink over the badge, so value
    /// editing belongs to the mode whose clicks select.
    pub(crate) fn number_at_double_click(
        &self,
        name: &str,
        local: Point<Pixels>,
    ) -> Option<(usize, Point<Pixels>)> {
        if self.annotations.tool() != Some(crate::annotation::ShapeKind::Select)
            || self.selection.bounds().is_none()
        {
            return None;
        }
        let origin = self.screen(name).bounds().origin;
        let p = local + origin;
        let ix = self.annotations.hit_test(p)?;
        let shape = self.annotations.committed().get(ix)?;
        (shape.kind == crate::annotation::ShapeKind::Number)
            .then(|| (ix, shape.bounds.origin - origin))
    }

    pub(crate) fn pointer_down(&mut self, name: &str, local: Point<Pixels>, alt: bool) {
        if self.blocked {
            return;
        }
        self.pointer_global = Some(self.to_global(name, local));
        if let Some(selection) = self.selection.bounds() {
            let p = local + self.screen(name).bounds().origin;
            // a handle of the SELECTED shape grabs first (phase C);
            // the handles sit on the shape's body, so this must run
            // before the shape hit-test parks a click
            if let Some(anchor) = self.annotations.selected().and_then(|s| s.handle_at(p)) {
                let ix = self.annotations.selected_index().expect("selected");
                let before = self.annotations.selected().expect("selected").clone();
                self.handle_drag = Some(HandleDrag { ix, anchor, before });
                return;
            }
            // press on a shape: click-or-drag resolves on the
            // following move/up events — and only the select tool
            // parks (draw tools draw right over existing ink)
            if selection.contains(&p)
                && self.annotations.parks_click_select()
                && let Some(ix) = self.annotations.hit_test(p)
            {
                self.pending_click = Some((ix, p));
                return;
            }
            if self.annotations.enabled() {
                self.annotations.deselect(); // click on blank canvas
                self.annotations.begin(p, selection, alt);
                return;
            }
        }
        self.annotations.deselect();
        // A finalized selection is editable in place: an edge/corner
        // band grabs a resize handle, the interior starts a move. Only
        // a press OUTSIDE starts a fresh selection (which also clears
        // the annotations — an edit must not).
        if self
            .selection
            .begin_edit(local + self.screen(name).bounds().origin)
        {
            self.hovered = None;
            self.set_active_output(name);
            return;
        }
        self.begin(name, local);
    }

    pub(crate) fn pointer_move(&mut self, name: &str, local: Point<Pixels>, square: bool) -> bool {
        if self.blocked {
            return false;
        }
        self.pointer_global = Some(self.to_global(name, local));
        if self.annotations.enabled()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.pending_click.is_some()
        {
            if let Some(selection) = self.selection.bounds() {
                let p = local + self.screen(name).bounds().origin;
                // handle grab wins over everything: handles only exist
                // for the selected shape, and catching one must not
                // fall through to selecting/drawing on the body
                if let Some(drag) = &self.handle_drag {
                    let clamped_p = point(
                        p.x.clamp(selection.left(), selection.right()),
                        p.y.clamp(selection.top(), selection.bottom()),
                    );
                    self.annotations
                        .place_handle(drag.ix, drag.anchor, &drag.before, clamped_p);
                    return true;
                }
                if let Some((ix, press)) = self.pending_click {
                    let dx = f32::from(p.x - press.x);
                    let dy = f32::from(p.y - press.y);
                    if dx * dx + dy * dy <= CLICK_SLOP * CLICK_SLOP {
                        return false; // still within click slop
                    }
                    self.pending_click = None;
                    // dragging an existing shape moves it
                    self.annotations.select_index(ix);
                    if let Some(before) = self.annotations.selected().cloned() {
                        self.moving = Some(MoveDrag { ix, before, press });
                    }
                }
                if let Some(drag) = &self.moving {
                    let selection = if drag.before.kind == crate::annotation::ShapeKind::Text {
                        text_area(selection).unwrap_or(selection)
                    } else {
                        selection
                    };
                    let b = drag.before.bounds;
                    let requested_delta = p - drag.press;
                    let (min_dx, max_dx) = if b.size.width <= selection.size.width {
                        (
                            f32::from(selection.left() - b.left()),
                            f32::from(selection.right() - b.right()),
                        )
                    } else {
                        let d = f32::from(selection.left() - b.left());
                        (d, d)
                    };
                    let (min_dy, max_dy) = if b.size.height <= selection.size.height {
                        (
                            f32::from(selection.top() - b.top()),
                            f32::from(selection.bottom() - b.bottom()),
                        )
                    } else {
                        let d = f32::from(selection.top() - b.top());
                        (d, d)
                    };
                    let dx = f32::from(requested_delta.x).clamp(min_dx, max_dx);
                    let dy = f32::from(requested_delta.y).clamp(min_dy, max_dy);
                    let clamped_delta = point(px(dx), px(dy));
                    self.annotations
                        .place_shape(drag.ix, &drag.before, clamped_delta);
                    return true;
                }
                if self.annotations.enabled() {
                    return self.annotations.drag_to(p, selection, square);
                }
            }
            false
        } else if self.selection.is_editing() {
            let Some(desktop) = self.desktop_bounds() else {
                return false;
            };
            self.selection
                .edit_to(local + self.screen(name).bounds().origin, desktop)
        } else {
            self.drag_to(name, local)
        }
    }

    pub(crate) fn pointer_up(&mut self, name: &str, local: Point<Pixels>, square: bool) {
        if self.blocked {
            return;
        }
        self.pointer_global = Some(self.to_global(name, local));
        if self.annotations.enabled()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.pending_click.is_some()
        {
            // a press-release without drag = click-select
            if let Some((ix, _)) = self.pending_click.take() {
                self.annotations.select_index(ix);
                return;
            }
            // release of a move drag: one Edit entry when it moved
            if let Some(drag) = self.moving.take() {
                self.annotations.commit_move(drag.ix, drag.before);
                return;
            }
            // release of a handle drag: one Edit entry when it changed
            if let Some(drag) = self.handle_drag.take() {
                self.annotations.commit_move(drag.ix, drag.before);
                return;
            }
            if self.annotations.enabled() {
                self.pointer_move(name, local, square);
                self.annotations.end();
                return;
            }
        }
        if self.selection.is_editing() {
            self.selection.end_edit();
            self.follow_selection_host();
            self.press = None;
        } else {
            self.end(name, local);
        }
    }

    pub(crate) fn cancel_annotation(&mut self) -> bool {
        // Escape interrupts an in-flight edit drag (move or handle):
        // restore the snapshot
        if let Some(drag) = self.moving.take() {
            self.annotations
                .place_shape(drag.ix, &drag.before, point(px(0.), px(0.)));
            return true;
        }
        if let Some(drag) = self.handle_drag.take() {
            self.annotations
                .place_shape(drag.ix, &drag.before, point(px(0.), px(0.)));
            return true;
        }
        !self.blocked && self.annotations.cancel()
    }

    /// Whether the body-move drag (phase B) is in flight.
    pub(crate) fn is_body_moving(&self) -> bool {
        self.moving.is_some()
    }

    /// The shape kind and anchor of the in-flight handle drag, if any.
    pub(crate) fn handle_drag_anchor(&self) -> Option<(crate::annotation::ShapeKind, usize)> {
        self.handle_drag.as_ref().map(|d| (d.before.kind, d.anchor))
    }

    /// The handle of the selected annotation under the pointer, if
    /// any — the hover probe for the directional cursor.
    pub(crate) fn annotation_handle_hover(&self) -> Option<(crate::annotation::ShapeKind, usize)> {
        let p = self.pointer_global?;
        let shape = self.annotations.selected()?;
        let anchor = shape.handle_at(p)?;
        Some((shape.kind, anchor))
    }

    pub(crate) fn local_annotations(&self, name: &str) -> Vec<crate::annotation::Shape> {
        let origin = self.screen(name).bounds().origin;
        self.annotations
            .visible()
            .cloned()
            .map(|mut shape| {
                shape.bounds.origin -= origin;
                for point in &mut shape.points {
                    *point -= origin;
                }
                shape
            })
            .collect()
    }

    /// The in-flight freehand draft (Pencil/Highlighter) in window-local
    /// coordinates. While the raster preview owns the committed marks,
    /// the live stroke paints as a vector union path on top instead of
    /// being baked into the selection image — baking meant re-uploading
    /// the whole crop to the atlas on every pointer move, and that
    /// per-frame cost is what broke strokes into visible dots.
    pub(crate) fn local_draft(&self, name: &str) -> Option<crate::annotation::Shape> {
        let origin = self.screen(name).bounds().origin;
        let mut shape = self.annotations.draft_shape()?.clone();
        if !matches!(
            shape.kind,
            crate::annotation::ShapeKind::Pencil | crate::annotation::ShapeKind::Highlighter
        ) {
            return None;
        }
        shape.bounds.origin -= origin;
        for point in &mut shape.points {
            *point -= origin;
        }
        Some(shape)
    }

    /// The selected shape cloned into the output's local coordinates —
    /// the highlight layer paints its visual geometry from this.
    pub(crate) fn selected_shape_local(&self, name: &str) -> Option<crate::annotation::Shape> {
        let origin = self.screen(name).bounds().origin;
        self.annotations.selected().map(|shape| {
            let mut s = shape.clone();
            s.bounds.origin -= origin;
            for p in &mut s.points {
                *p -= origin;
            }
            s
        })
    }

    /// The live loupe target, window-local for `name`'s output. None
    /// when no precision drag is live, or when the focus sits on
    /// another output — each overlay magnifies only the pixels it
    /// froze, so the loupe renders on the output that owns the focus.
    pub(crate) fn local_loupe(&self, name: &str) -> Option<Loupe> {
        let mut loupe = self.loupe()?;
        let screen = self.screen(name).bounds();
        if !screen.contains(&loupe.focus) {
            return None;
        }
        loupe.focus -= screen.origin;
        Some(loupe)
    }

    /// The gesture-level loupe target (desktop-global). Selection
    /// corner resizes and shape handle drags only — everything else
    /// (moving, fresh dragging, edge resizes) places no precise point.
    fn loupe(&self) -> Option<Loupe> {
        if let Selection::Resizing { bounds, handle, .. } = self.selection {
            let (hx, hy) = handle.axes();
            let (Some(hx), Some(hy)) = (hx, hy) else {
                return None; // edge handle: aims a line, not a pixel
            };
            let focus = match (hx, hy) {
                (false, false) => bounds.origin,
                (true, false) => point(bounds.right(), bounds.top()),
                (false, true) => point(bounds.left(), bounds.bottom()),
                (true, true) => bounds.bottom_right(),
            };
            return Some(Loupe {
                focus,
                outward: (if hx { 1. } else { -1. }, if hy { 1. } else { -1. }),
            });
        }
        let drag = self.handle_drag.as_ref()?;
        let shape = self.annotations.shape(drag.ix)?;
        let focus = *shape.handle_points().get(drag.anchor)?;
        // Away from the shape's body: the diagonal the handle sits on
        // relative to the bounds center (a centered axis picks +1 —
        // deterministic; degenerate shapes barely have off-diagonal
        // handles anyway).
        let center = shape.bounds.center();
        Some(Loupe {
            focus,
            outward: (
                if f32::from(focus.x) >= f32::from(center.x) {
                    1.
                } else {
                    -1.
                },
                if f32::from(focus.y) >= f32::from(center.y) {
                    1.
                } else {
                    -1.
                },
            ),
        })
    }

    /// What a press at the pointer would do to an annotation shape:
    /// pick an unselected one, move the selected one — the hover
    /// probe the cursor maps onto pointing hand / open hand (issue
    /// #17).
    pub(crate) fn annotation_hover(&self) -> Option<crate::annotation::ShapeHover> {
        self.pointer_global
            .and_then(|p| self.annotations.shape_hover(p))
    }

    /// Whether pointer moves must repaint for the eraser's ring
    /// chrome even when nothing else changed: the ring follows the
    /// cursor while the brush tool is live (issue #14).
    pub(crate) fn eraser_ring_follows_pointer(&self) -> bool {
        !self.blocked
            && self.selection.bounds().is_some()
            && self.annotations.tool() == Some(crate::annotation::ShapeKind::Eraser)
    }

    /// The eraser's pointer chrome for one output (issue #14): the
    /// brush ring while the tool is live (center in this window's
    /// local coordinates), or the dragged area rect while an area
    /// gesture is in flight. None when the eraser is inactive or
    /// blocked, when the ring's pointer sits outside the selection
    /// (idle — no erase can start there), or when a rect does not
    /// reach this output.
    pub(crate) fn eraser_chrome(&self, name: &str) -> Option<EraserChrome> {
        use crate::annotation::ShapeKind;
        if self.blocked {
            return None;
        }
        let selection = self.selection.bounds()?;
        let origin = self.screen(name).bounds().origin;
        if let Some(area) = self.annotations.eraser_rect_bounds() {
            let mut b = area.intersect(&selection);
            b.origin -= origin;
            return (f32::from(b.size.width) > 0. && f32::from(b.size.height) > 0.)
                .then_some(EraserChrome::Rect(b));
        }
        if self.annotations.tool() != Some(ShapeKind::Eraser) {
            return None;
        }
        let center = self.pointer_in(name)?;
        // Only the output whose window actually contains the pointer
        // rings — the unclamped mapping lies outside every other
        // window, and a cross-screen drag (implicit grab) must move
        // the ring to the output the pointer is now over.
        let size = self.overlay_size(name)?;
        let (x, y) = (f32::from(center.x), f32::from(center.y));
        let over_window =
            x >= 0. && x <= f32::from(size.width) && y >= 0. && y <= f32::from(size.height);
        let active = self.pointer_global.is_some_and(|p| selection.contains(&p))
            || self.annotations.is_pressed();
        (over_window && active).then(|| EraserChrome::Ring {
            center,
            radius: self.annotations.erase_radius(),
        })
    }
}
