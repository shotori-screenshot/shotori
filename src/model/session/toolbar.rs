//! Toolbar geometry + dragging for [`ScreenshotSession`].
//!
//! Split from session.rs: this is one concern — the toolbar's rect, its
//! drag-grip strips, the paint-time grip report and the drag state
//! machine. One source of truth: render, the cursor hit-test and the
//! drag clamp all read THESE functions, so the three cannot drift
//! apart. Lives as a child module so it can keep touching the session's
//! private fields without widening them.

use super::ScreenshotSession;
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

/// An in-flight toolbar drag: `grab` is the press offset from the
/// toolbar's origin (press − origin, window-local) so the toolbar
/// follows the pointer without jumping; `restore` is the pre-drag
/// position override (None = anchored) for Esc.
#[derive(Clone, Copy)]
pub(super) struct ToolbarDrag {
    pub(super) grab: Point<Pixels>,
    pub(super) restore: Option<Point<Pixels>>,
}

impl ScreenshotSession {
    // ── Toolbar geometry + dragging ────────────────────────────────
    // One source of truth: render, the cursor hit-test and the drag
    // clamp all read THIS, so the three cannot drift apart.

    /// The toolbar's rect in this output's LOCAL coordinates — the
    /// placement anchor, or wherever the user last dragged it (clamped
    /// inside the window). None when this output hosts no toolbar.
    pub(crate) fn toolbar_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        if !self.selection.is_selected() || !self.active_on(name) {
            return None;
        }
        let ws = self.overlay_size(name)?;
        let sel = self
            .local_bounds(name)
            .map(crate::model::placement::round_px)?;
        let (width, height) = crate::model::placement::toolbar_size(
            // the placement basis must match the settings row's own
            // visibility gate (edit_kind), not "a tool is lit": the
            // select tool shows no settings row until a shape is
            // selected, so keying on the lit tool would under- or
            // over-reserve the toolbar's height
            self.annotations.edit_kind().is_some(),
            crate::model::settings::current(),
        );
        let mut b = crate::model::placement::toolbar_bounds(&sel, ws, width, height);
        if let Some(pos) = self.toolbar_pos {
            b.origin = point(
                px(f32::from(pos.x).clamp(
                    8.,
                    (f32::from(ws.width) - f32::from(b.size.width) - 8.).max(8.),
                )),
                px(f32::from(pos.y).clamp(
                    8.,
                    (f32::from(ws.height) - f32::from(b.size.height) - 8.).max(8.),
                )),
            );
        }
        Some(b)
    }

    /// The left/right drag-grip strips (local coords) — for the cursor's
    /// grab affordance and nothing else; the elements themselves live in
    /// `ui::toolbar`. ROW ONE ONLY (the settings row does not grab).
    /// Served from the RENDERED rects (see `toolbar_grips_reported`)
    /// whenever a full pair has been painted — the derived strips below
    /// are only the first-frame fallback, because the basis width can
    /// drift from the real bar (it did: see the field's comment).
    pub(crate) fn toolbar_grips(&self, name: &str) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        if let Some((out, l, r)) = &self.toolbar_grips_reported
            && out == name
            && l.size.width > px(0.)
            && r.size.width > px(0.)
        {
            return Some((*l, *r));
        }
        self.derived_toolbar_grips(name)
    }

    /// The strips as recomputed from the placement basis — the fallback
    /// path of [`ScreenshotSession::toolbar_grips`], kept exact to the
    /// grip element's intended geometry (BAR_PAD inset, ROW_H tall).
    fn derived_toolbar_grips(&self, name: &str) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let b = self.toolbar_bounds(name)?;
        let (w, h) = (
            px(crate::model::placement::GRIP_W),
            px(crate::model::placement::ROW_H),
        );
        let pad = px(crate::model::placement::BAR_PAD);
        Some((
            Bounds {
                origin: point(b.origin.x + pad, b.origin.y),
                size: size(w, h),
            },
            Bounds {
                origin: point(b.right() - pad - w, b.origin.y),
                size: size(w, h),
            },
        ))
    }

    /// Paint-time report from a grip element's measure canvas. `left`
    /// picks the strip. Returns true when the stored value changed, so
    /// the caller can schedule the repaint that re-derives the cursor.
    pub(crate) fn report_toolbar_grip(
        &mut self,
        name: &str,
        left: bool,
        rect: Bounds<Pixels>,
    ) -> bool {
        match &mut self.toolbar_grips_reported {
            Some((out, l, r)) if out == name => {
                let slot = if left { l } else { r };
                if *slot == rect {
                    false
                } else {
                    *slot = rect;
                    true
                }
            }
            // First report of a new output: the unreported strip stays
            // zero-sized, which keeps `toolbar_grips` on the fallback
            // until its element paints too (same frame, child order).
            _ => {
                let empty = Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(0.), px(0.)),
                };
                let (l, r) = if left { (rect, empty) } else { (empty, rect) };
                self.toolbar_grips_reported = Some((name.into(), l, r));
                true
            }
        }
    }

    /// Press on an edge grip: start dragging the toolbar. `press` is in
    /// the host window's local coordinates. Returns false when there is
    /// no draggable toolbar here (or a modal owns the session).
    pub(crate) fn toolbar_drag_begin(&mut self, name: &str, press: Point<Pixels>) -> bool {
        self.finish_nudge();
        if self.blocked {
            return false;
        }
        let Some(b) = self.toolbar_bounds(name) else {
            return false;
        };
        self.pointer_global = Some(self.to_global(name, press));
        self.toolbar_drag = Some(ToolbarDrag {
            grab: press - b.origin,
            restore: self.toolbar_pos,
        });
        true
    }

    /// Drag the toolbar under the pointer (window-local coords),
    /// clamped inside the window — the toolbar cannot leave its layer.
    /// Returns whether the position changed (callers decide on notify).
    pub(crate) fn toolbar_drag_move(&mut self, name: &str, local: Point<Pixels>) -> bool {
        let Some(grab) = self.toolbar_drag.map(|d| d.grab) else {
            return false;
        };
        let Some(ws) = self.overlay_size(name) else {
            return false;
        };
        self.pointer_global = Some(self.to_global(name, local));
        let (base_w, height) = crate::model::placement::toolbar_size(
            self.annotations.edit_kind().is_some(),
            crate::model::settings::current(),
        );
        let w = base_w.min((f32::from(ws.width) - 16.).max(1.));
        let next = point(
            px((f32::from(local.x) - f32::from(grab.x))
                .clamp(8., (f32::from(ws.width) - w - 8.).max(8.))),
            px((f32::from(local.y) - f32::from(grab.y))
                .clamp(8., (f32::from(ws.height) - height - 8.).max(8.))),
        );
        if self.toolbar_pos == Some(next) {
            return false;
        }
        self.toolbar_pos = Some(next);
        true
    }

    /// Release: the dragged position becomes the toolbar's new home.
    pub(crate) fn toolbar_drag_end(&mut self) {
        self.toolbar_drag = None;
    }

    /// A grip drag is in flight?
    pub(crate) fn toolbar_drag_active(&self) -> bool {
        self.toolbar_drag.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::ScreenshotSession;
    use crate::platform::capture::Capture;
    use gpui_kit::{point, px, size};
    use std::sync::Arc;

    fn screen(name: &str, pos: (i32, i32), scale: f32, color: [u8; 4]) -> Arc<Capture> {
        let mut cap = Capture::for_test(pos, scale);
        cap.output_name = name.into();
        cap.width = (100. * scale) as u32;
        cap.height = (100. * scale) as u32;
        cap.rgba = color.repeat((cap.width * cap.height) as usize);
        Arc::new(cap)
    }

    fn session() -> ScreenshotSession {
        ScreenshotSession::new(
            vec![
                screen("left", (-100, 20), 1., [255, 0, 0, 255]),
                screen("right", (0, 0), 2., [0, 255, 0, 255]),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn toolbar_drag_moves_clamps_and_reverts() {
        let mut s = session();
        s.set_size("right", size(px(1200.), px(800.)));
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(200.), px(150.))); // (50,50)-(200,150), right is host

        // anchored below the box by default; grips line both edges of ROW ONE
        let anchored = s.toolbar_bounds("right").unwrap();
        assert_eq!(anchored.origin, point(px(50.), px(158.)));
        assert_eq!(
            anchored.size.width,
            px(crate::model::placement::default_row1_basis())
        );
        let (lg, rg) = s.toolbar_grips("right").unwrap();
        let pad = px(crate::model::placement::BAR_PAD);
        // the strips are the grip ELEMENTS' rects: inset by the bar
        // padding, one row tall — pixel-identical to what renders
        assert_eq!(lg.left(), anchored.left() + pad);
        assert_eq!(rg.right(), anchored.right() - pad);
        assert_eq!(lg.size.width, px(crate::model::placement::GRIP_W));
        assert_eq!(lg.size.height, px(crate::model::placement::ROW_H));
        // a different output hosts nothing
        assert!(s.toolbar_bounds("left").is_none());

        // drag from the middle of the left grip: toolbar follows, no jump
        let press = point(px(56.), px(177.)); // grab = (6, 19)
        assert!(s.toolbar_drag_begin("right", press));
        assert!(s.toolbar_drag_active());
        assert!(s.toolbar_drag_move("right", point(px(200.), px(120.))));
        let b = s.toolbar_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(194.), px(101.)));
        assert_eq!(b.size, anchored.size); // size never changes
        // unchanged position reports false (no duplicate notify)
        assert!(!s.toolbar_drag_move("right", point(px(200.), px(120.))));

        // clamped inside the window on every side
        assert!(s.toolbar_drag_move("right", point(px(2000.), px(2000.))));
        assert_eq!(
            s.toolbar_bounds("right").unwrap().origin,
            point(
                px(1200. - crate::model::placement::default_row1_basis() - 8.),
                px(800. - crate::model::placement::ROW_H - 8.),
            )
        );
        assert!(s.toolbar_drag_move("right", point(px(-999.), px(-999.))));
        assert_eq!(
            s.toolbar_bounds("right").unwrap().origin,
            point(px(8.), px(8.))
        );

        // Esc mid-drag: back to the anchor (the pre-drag override was None)
        s.cancel_drag();
        assert!(!s.toolbar_drag_active());
        assert_eq!(s.toolbar_bounds("right").unwrap(), anchored);

        // a completed drag stays put…
        assert!(s.toolbar_drag_begin("right", point(px(56.), px(177.))));
        assert!(s.toolbar_drag_move("right", point(px(300.), px(300.))));
        s.toolbar_drag_end();
        let dropped = s.toolbar_bounds("right").unwrap().origin;
        assert_eq!(dropped, point(px(294.), px(281.)));
        // …and Esc with no drag in flight does NOT move it (stage two: quit)
        s.cancel_drag();
        assert_eq!(s.toolbar_bounds("right").unwrap().origin, dropped);
    }

    #[test]
    fn editing_the_selection_keeps_a_dragged_toolbar_but_a_new_one_reanchors() {
        let mut s = session();
        s.set_size("right", size(px(1200.), px(800.)));
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(200.), px(150.)));
        let anchored = s.toolbar_bounds("right").unwrap().origin;
        assert!(s.toolbar_drag_begin("right", point(px(56.), px(177.))));
        assert!(s.toolbar_drag_move("right", point(px(400.), px(500.))));
        s.toolbar_drag_end();
        let dragged = s.toolbar_bounds("right").unwrap().origin;
        assert_ne!(dragged, anchored);

        // moving the SELECTION (an edit) keeps the user's placement
        s.pointer_down("right", point(px(120.), px(100.)), false, false); // interior
        s.pointer_move("right", point(px(220.), px(200.)), false);
        s.pointer_up("right", point(px(220.), px(200.)), false);
        assert_eq!(s.toolbar_bounds("right").unwrap().origin, dragged);

        // a NEW selection re-anchors
        s.pointer_down("right", point(px(500.), px(500.)), false, false);
        s.pointer_move("right", point(px(700.), px(650.)), false);
        s.pointer_up("right", point(px(700.), px(650.)), false);
        assert_ne!(s.toolbar_bounds("right").unwrap().origin, dragged);

        // and so does a change of host window (the override is local!)
        s.set_size("left", size(px(1200.), px(800.)));
        s.begin("left", point(px(300.), px(50.)));
        s.end("left", point(px(700.), px(150.)));
        assert!(s.toolbar_drag_begin("left", point(px(306.), px(177.))));
        assert!(s.toolbar_drag_move("left", point(px(400.), px(500.))));
        s.toolbar_drag_end();
        assert!(s.toolbar_bounds("left").unwrap().origin.y > px(158.)); // dragged
        // selection replaced from the right screen → left re-anchors
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(200.), px(150.)));
        assert!(s.toolbar_bounds("left").is_none());
    }
}
