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

/// Axis-aligned rectangle spanned by two desktop-global points (the
/// marquee's anchor and its live corner).
fn global_rect(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
    let (left, right) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
    let (top, bottom) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
    Bounds::from_corners(point(left, top), point(right, bottom))
}

/// Union of movement bounds, preserving each mark's frame and text inset.
/// One clamped delta keeps the group's internal spacing unchanged.
pub(super) fn group_movement_bounds(
    before: &[(usize, crate::annotation::Shape)],
) -> Option<Bounds<Pixels>> {
    let mut bounds = before.iter().map(|(_, shape)| {
        if shape.kind == crate::annotation::ShapeKind::Text {
            let inset = point(px(TEXT_INSET), px(TEXT_INSET));
            let ink = shape.ink_bounds();
            Bounds::from_corners(ink.origin - inset, ink.bottom_right() + inset)
        } else {
            shape.selection_box()
        }
    });
    let first = bounds.next()?;
    Some(bounds.fold(first, |acc, b| acc.union(&b)))
}

pub(super) fn text_area(selection: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
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
            self.finish_nudge();
            if let Some(area) = self.selection.bounds() {
                let area =
                    if self.annotations.edit_kind() == Some(crate::annotation::ShapeKind::Text) {
                        text_area(area).unwrap_or(area)
                    } else {
                        area
                    };
                self.annotations.edit_within(area, edit);
            }
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
        self.finish_nudge();
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
        if !self.annotations.parks_click_select() || self.selection.bounds().is_none() {
            return None;
        }
        let origin = self.screen(name).bounds().origin;
        let p = local + origin;
        let ix = self.annotations.hit_test(p)?;
        let shape = self.annotations.committed().get(ix)?;
        (shape.kind == crate::annotation::ShapeKind::Number)
            .then(|| (ix, shape.bounds.origin - origin))
    }

    /// `shift` is the multi-select modifier: Shift+click toggles the
    /// hit shape in/out of the selection, and Shift+drag on blank
    /// canvas adds the marquee's hits to the existing selection.
    pub(crate) fn pointer_down(
        &mut self,
        name: &str,
        local: Point<Pixels>,
        alt: bool,
        shift: bool,
    ) {
        if self.blocked {
            return;
        }
        self.finish_nudge();
        self.pointer_global = Some(self.to_global(name, local));
        if let Some(selection) = self.selection.bounds() {
            let p = local + self.screen(name).bounds().origin;
            // a handle of the SELECTED shape grabs first (phase C);
            // the handles sit on the shape's body, so this must run
            // before the shape hit-test parks a click. A multi-selection
            // owns no handles (they would edit one member out of the
            // group) — the frame is the affordance, the body moves.
            //
            // Shift never grabs: it is the toggle modifier, and a small
            // shape's body lies almost entirely inside some handle's
            // GRAB_RADIUS, so grabbing first made Shift+click fail to
            // deselect exactly the marks the user meant to toggle out.
            if !shift
                && self.annotations.selected_count() == 1
                && let Some(anchor) = self.annotations.selected().and_then(|s| s.handle_at(p))
            {
                // handle_at only reports handles of the selected shape —
                // the same lookup this branch condition just proved — so
                // zip the index and the copy instead of unwrapping twice.
                let Some((ix, before)) = self
                    .annotations
                    .selected_index()
                    .zip(self.annotations.selected())
                else {
                    return;
                };
                let before = before.clone();
                self.handle_drag = Some(HandleDrag {
                    ix,
                    anchor,
                    before,
                    press: p,
                });
                return;
            }
            // press on a shape: click-or-drag resolves on the
            // following move/up events — and only the select tool
            // parks (draw tools draw right over existing ink)
            if selection.contains(&p)
                && self.annotations.parks_click_select()
                && let Some(ix) = if shift && !self.annotations.enabled() {
                    self.annotations
                        .hit_test(p)
                        .or_else(|| self.annotations.interaction_hit_test(p))
                } else {
                    self.annotations.interaction_hit_test(p)
                }
            {
                if shift {
                    // toggle on PRESS: the added/removed highlight is
                    // the feedback, and a drag that follows moves what
                    // is still selected
                    self.annotations.toggle_index(ix);
                }
                self.pending_click = Some((ix, p, shift));
                return;
            }
            if self.annotations.enabled() {
                self.annotations.deselect(); // click on blank canvas
                self.annotations.begin(p, selection, alt);
                return;
            }
            if (self.annotations.tool() == Some(crate::annotation::ShapeKind::Select) || shift)
                && selection.contains(&p)
            {
                // blank canvas under the select tool: the marquee. A
                // plain press clears the selection now — release
                // without motion is a click on empty canvas (deselect),
                // and a drag starts from nothing, replacing the old
                // selection; Shift keeps it as the additive base.
                let base = if shift {
                    self.annotations.selected_indices().to_vec()
                } else {
                    self.annotations.deselect();
                    Vec::new()
                };
                let press = self.to_global(name, local);
                self.marquee = Some(super::MarqueeDrag {
                    start: press,
                    current: press,
                    additive: shift,
                    base,
                });
                self.set_active_output(name);
                return;
            }
        }
        self.annotations.deselect();
        let before = self.selection_snapshot();
        // A finalized selection is editable in place: an edge/corner
        // band grabs a resize handle, the interior starts a move. A
        // press outside waits for a deliberate drag before reselecting.
        if self
            .selection
            .begin_edit(local + self.screen(name).bounds().origin)
        {
            if let Some(before) = before {
                self.selection_gesture = Some(super::editing::SelectionGesture::Edit(before));
            }
            self.hovered = None;
            self.set_active_output(name);
            return;
        }
        if let Some(before) = before {
            self.selection_gesture = Some(super::editing::SelectionGesture::Pending {
                press: self.to_global(name, local),
                before,
            });
        } else {
            self.begin(name, local);
        }
    }

    pub(crate) fn pointer_move(&mut self, name: &str, local: Point<Pixels>, square: bool) -> bool {
        if self.blocked {
            return false;
        }
        self.pointer_global = Some(self.to_global(name, local));
        if self.marquee.is_some() {
            let p = self.to_global(name, local);
            let Some(selection) = self.selection.bounds() else {
                return false;
            };
            let clamped = point(
                p.x.clamp(selection.left(), selection.right()),
                p.y.clamp(selection.top(), selection.bottom()),
            );
            let Some(marquee) = self.marquee.as_mut() else {
                return false;
            };
            let changed = clamped != marquee.current;
            marquee.current = clamped;
            return changed;
        }
        if matches!(
            self.selection_gesture,
            Some(
                super::editing::SelectionGesture::Pending { .. }
                    | super::editing::SelectionGesture::Reselect(_)
            )
        ) {
            return self.drag_to(name, local);
        }
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
                    if drag.anchor == crate::annotation::ROTATION_HANDLE {
                        self.annotations.rotate_shape(
                            drag.ix,
                            &drag.before,
                            drag.press,
                            p,
                            selection,
                            square,
                        );
                        return true;
                    }
                    self.annotations.place_handle(
                        drag.ix,
                        drag.anchor,
                        &drag.before,
                        clamped_p,
                        selection,
                    );
                    return true;
                }
                if let Some((ix, press, shift)) = self.pending_click {
                    let dx = f32::from(p.x - press.x);
                    let dy = f32::from(p.y - press.y);
                    if dx * dx + dy * dy <= CLICK_SLOP * CLICK_SLOP {
                        return false; // still within click slop
                    }
                    self.pending_click = None;
                    if !self.annotations.is_selected(ix) {
                        // a Shift+press that REMOVED the shape has
                        // nothing to drag; an unselected shape under a
                        // plain press becomes the single selection and
                        // then moves (the pre-marquee behavior)
                        if shift {
                            return false;
                        }
                        self.annotations.select_index(ix);
                    }
                    // dragging a shape with a live selection moves the
                    // WHOLE selection — one snapshot per member, primary
                    // last (`selected_indices` order)
                    let before: Vec<(usize, crate::annotation::Shape)> = self
                        .annotations
                        .selected_indices()
                        .iter()
                        .filter_map(|ix| self.annotations.shape(*ix).map(|s| (*ix, s.clone())))
                        .collect();
                    if !before.is_empty() {
                        self.moving = Some(MoveDrag { before, press });
                    }
                }
                if let Some(drag) = &self.moving {
                    if let [(ix, before)] = drag.before.as_slice()
                        && before.kind == crate::annotation::ShapeKind::Magnifier
                        && !before.bounds.contains(&drag.press)
                        && let Some(bounds) = before
                            .magnified_bounds()
                            .filter(|b| b.contains(&drag.press))
                    {
                        let delta =
                            super::editing::movement_delta(bounds, selection, p - drag.press);
                        self.annotations
                            .place_shape(*ix, before, point(px(0.), px(0.)));
                        self.annotations.move_selected_lens(delta);
                        return true;
                    }
                    let Some(ink) = group_movement_bounds(&drag.before) else {
                        return false;
                    };
                    let clamped_delta =
                        super::editing::movement_delta(ink, selection, p - drag.press);
                    self.annotations.place_shapes(&drag.before, clamped_delta);
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
        if self.marquee.is_some() {
            // track the final position before resolving the rectangle
            self.pointer_move(name, local, square);
            let Some(marquee) = self.marquee.take() else {
                return;
            };
            let rect = global_rect(marquee.start, marquee.current);
            // A press that never left the click slop is a click on
            // empty canvas — the press already cleared a plain
            // selection. Picking from a sliver would delete nothing and
            // select accidental shapes the pointer merely crossed.
            let spanned =
                f32::from(rect.size.width) > CLICK_SLOP || f32::from(rect.size.height) > CLICK_SLOP;
            if !spanned {
                return;
            }
            let hits = self.annotations.shapes_in(rect);
            if marquee.additive {
                // Shift+drag adds to what was selected at press time;
                // the marquee's hits come after the base so the topmost
                // hit takes primary, like a click would
                let mut all = marquee.base;
                all.extend(hits);
                self.annotations.select_indices(all);
            } else {
                self.annotations.select_indices(hits);
            }
            return;
        }
        if self.moving.is_some() || self.handle_drag.is_some() || self.pending_click.is_some() {
            self.pointer_move(name, local, square);
        }
        if self.annotations.enabled()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.pending_click.is_some()
        {
            // a press-release without drag = click-select (a Shift+press
            // already toggled on the way down)
            if let Some((ix, _, shift)) = self.pending_click.take() {
                if !shift {
                    self.annotations.select_index(ix);
                }
                return;
            }
            // release of a move drag: one Edit/EditMany when it moved
            if let Some(drag) = self.moving.take() {
                self.annotations.commit_moves(&drag.before);
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
            self.pointer_move(name, local, square);
            self.selection.end_edit();
            self.follow_selection_host();
            self.press = None;
            self.commit_selection_gesture();
        } else {
            self.end(name, local);
        }
    }

    /// Finish an active polyline first; otherwise leave annotation mode.
    pub(crate) fn finish_or_exit_annotation(&mut self) {
        if self.blocked
            || (self.annotations.tool().is_none() && self.annotations.selected_index().is_none())
        {
            return;
        }
        if self.annotations.is_drawing_polyline() {
            self.annotations.finish_polyline();
            return;
        }
        self.cancel_annotation();
        self.annotations.finish_interaction();
    }

    /// Cancel one interaction level before allowing Escape to close the screenshot.
    pub(crate) fn escape_interaction(&mut self) -> bool {
        if self.blocked {
            return true;
        }
        self.finish_nudge();
        if self.annotations.is_pressed()
            || self.annotations.is_drawing_polyline()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.pending_click.is_some()
            || self.marquee.is_some()
        {
            self.cancel_annotation();
            return true;
        }
        if self.selection.is_dragging()
            || self.selection.is_editing()
            || self.toolbar_drag_active()
            || self.selection_gesture.is_some()
        {
            self.cancel_drag();
            return true;
        }
        if self.annotations.tool().is_some() || self.annotations.selected_index().is_some() {
            self.annotations.finish_interaction();
            return true;
        }
        false
    }

    pub(crate) fn cancel_annotation(&mut self) -> bool {
        if self.blocked {
            return false;
        }
        self.finish_nudge();
        // the marquee lives in its own slot, so it must be taken before
        // the gesture chain below can answer "nothing active"
        if self.marquee.take().is_some() {
            return true;
        }
        if self.cancel_selection_gesture() || self.pending_click.take().is_some() {
            return true;
        }
        // Escape interrupts an in-flight edit drag (move or handle):
        // restore the snapshot
        if let Some(drag) = self.moving.take() {
            self.annotations
                .place_shapes(&drag.before, point(px(0.), px(0.)));
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
        // a multi-selection carries no handles — they would edit one
        // member out of the group — so hovering offers nothing to grab
        if self.annotations.selected_count() != 1 {
            return None;
        }
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
                shape.translate(-origin);
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
        if !shape.is_freehand() || self.annotations.composite_draft() {
            return None;
        }
        shape.translate(-origin);
        Some(shape)
    }

    /// Immediate source geometry; independent of background preview jobs.
    pub(crate) fn local_lens_draft_outline(&self, name: &str) -> Option<crate::annotation::Shape> {
        let mut shape = self.annotations.draft_shape()?.lens_source_outline()?;
        shape.translate(-self.screen(name).bounds().origin);
        Some(shape)
    }

    /// The selected shapes cloned into the output's local coordinates —
    /// the highlight layer paints every selection frame from this.
    /// Selection order is preserved, so the PRIMARY (the one that owns
    /// handle dots and size/number edits) is the last element.
    pub(crate) fn selected_shapes_local(&self, name: &str) -> Vec<crate::annotation::Shape> {
        let origin = self.screen(name).bounds().origin;
        self.annotations
            .selected_shapes()
            .into_iter()
            .map(|shape| {
                let mut s = shape.clone();
                s.translate(-origin);
                s
            })
            .collect()
    }

    /// The live marquee rectangle in `name`'s window-local coordinates.
    /// None while no marquee is live, or when the drag sits entirely on
    /// another output — each overlay paints only what it froze.
    pub(crate) fn marquee_bounds_local(&self, name: &str) -> Option<Bounds<Pixels>> {
        let marquee = self.marquee.as_ref()?;
        let screen = self.screen(name).bounds();
        let rect = global_rect(marquee.start, marquee.current);
        // clip to this output: a marquee dragged across monitors keeps
        // the band it actually covers here instead of vanishing
        let left = if rect.origin.x < screen.left() {
            screen.left()
        } else {
            rect.origin.x
        };
        let top = if rect.origin.y < screen.top() {
            screen.top()
        } else {
            rect.origin.y
        };
        let right = if rect.bottom_right().x > screen.right() {
            screen.right()
        } else {
            rect.bottom_right().x
        };
        let bottom = if rect.bottom_right().y > screen.bottom() {
            screen.bottom()
        } else {
            rect.bottom_right().y
        };
        (right > left && bottom > top).then(|| {
            Bounds::from_corners(
                point(left - screen.origin.x, top - screen.origin.y),
                point(right - screen.origin.x, bottom - screen.origin.y),
            )
        })
    }

    /// The unselected mark a press would pick at the shared pointer.
    pub(crate) fn hovered_annotation(&self) -> Option<usize> {
        if self.blocked
            || !self.selection.is_selected()
            || self.selection_gesture.is_some()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.toolbar_drag.is_some()
            || self.marquee.is_some()
            || !matches!(
                self.annotations.tool(),
                None | Some(crate::annotation::ShapeKind::Select)
            )
        {
            return None;
        }
        let p = self.pointer_global?;
        if !self.selection.bounds()?.contains(&p) {
            return None;
        }
        let ix = self.annotations.hit_test(p)?;
        (!self.annotations.is_selected(ix)).then_some(ix)
    }

    pub(crate) fn annotation_hover_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        let ix = self.hovered_annotation()?;
        let mut bounds = self.annotations.shape(ix)?.selection_box();
        bounds.origin -= self.screen(name).bounds().origin;
        Some(bounds)
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
        // the marquee owns the pointer: no per-shape hover while it is
        // sweeping (the marquee rectangle is the feedback)
        if self.blocked
            || !self.selection.is_selected()
            || self.selection_gesture.is_some()
            || self.marquee.is_some()
        {
            return None;
        }
        let p = self.pointer_global?;
        self.selection
            .bounds()?
            .contains(&p)
            .then(|| self.annotations.shape_hover(p))
            .flatten()
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
