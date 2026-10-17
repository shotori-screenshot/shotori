//! Shared screenshot selection and captures for every overlay in one session.
//!
//! This module is the session core: the state (screens, selection,
//! pointer, toolbar drag), the selection gestures and the geometry
//! mapping. Three concern blocks live in child modules, reached as
//! plain `impl ScreenshotSession` extensions: [`smart`] (pixel-based
//! region detection), [`annotations`] (annotation-tool glue) and
//! [`export`] (crop and the filtered preview pipeline). The tests
//! stay centralized here — they share one fixture set and drive the
//! session through its public surface.
use std::sync::Arc;

use gpui_kit::*;

use crate::{model::selection::Selection, platform::capture::Capture};

/// How far a press on an annotation shape may travel before it stops
/// being a click (select) and turns into a draw stroke instead.
const CLICK_SLOP: f32 = 4.;

// One inset for placement, editing and movement; the outline is painted inside
// the text bounds, so no separate border outset may consume this breathing room.

struct Screen {
    capture: Arc<Capture>,
    logical_size: Size<Pixels>,
    /// Pixel-based smart-region candidates for this output's frozen
    /// capture (see [`crate::model::regiondetect`]). None until the
    /// background analysis lands — hover/click fall back to the
    /// compositor snap rects until then.
    detector: Option<Arc<crate::model::regiondetect::RegionDetector>>,
}

/// An in-flight move of the selected shape (issue #5 phase B): the
/// press-time snapshot plus the press point — every move re-derives
/// the shape from the snapshot, and commit records one Edit.
struct MoveDrag {
    ix: usize,
    before: crate::annotation::Shape,
    press: Point<Pixels>,
}

/// An in-flight handle drag (issue #5 phase C): which handle anchor of
/// the selected shape, plus the press-time snapshot. The anchor
/// follows the pointer exactly (no snapping yet); commit records one
/// Edit.
struct HandleDrag {
    ix: usize,
    anchor: usize,
    before: crate::annotation::Shape,
}

/// The magnifier loupe's target (issue #19): the desktop-global point a
/// precision drag is placing — a selection CORNER resize (an edge drag
/// aims a line, not a pixel) or any placed-shape handle drag
/// (endpoints, corners and vertices are all point placements). The
/// CONTENT centers on the focus; the INSET floats `outward`, the ±1
/// diagonal away from the resized body — visible beside the point when
/// fine-tuning, out of the way when not (covering the point itself
/// blocks the coarse pass; tried, reverted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Loupe {
    /// The point being placed (desktop-global).
    pub(crate) focus: Point<Pixels>,
    /// ±1 per axis: the diagonal away from the resized body.
    pub(crate) outward: (f32, f32),
}

/// The eraser's pointer chrome (issue #14), already mapped into the
/// asking window's local coordinates. The ring shows exactly the
/// circle deletion tests against ("what you see is what erases");
/// the rect is the in-flight area sweep, dashed as a promise rather
/// than a selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EraserChrome {
    /// Brush footprint: center + radius, logical px.
    Ring { center: Point<Pixels>, radius: f32 },
    /// The area eraser's dragged rectangle.
    Rect(Bounds<Pixels>),
}

impl Screen {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: point(
                px(self.capture.logical_pos.0 as f32),
                px(self.capture.logical_pos.1 as f32),
            ),
            size: self.logical_size,
        }
    }
}

pub(crate) struct RasterSelection {
    scale: f32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) bounds: Bounds<Pixels>,
}

/// A long-screenshot capture region: output-local logical integer px,
/// the coordinate contract of `capture_output_region` (see
/// [`ScreenshotSession::scroll_region`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

struct FilterPreview {
    selection: Option<Bounds<Pixels>>,
    committed: Vec<crate::annotation::Shape>,
    draft: Option<crate::annotation::Shape>,
    draft_generation: u64,
    original: RasterSelection,
    pixels: Vec<u8>,
    image: Arc<RenderImage>,
    stroke: Option<crate::annotation::StrokePreview>,
}

struct DisplayPreview {
    selection: Option<Bounds<Pixels>>,
    committed: Vec<crate::annotation::Shape>,
    draft_kind: Option<crate::annotation::ShapeKind>,
    draft_generation: u64,
    bounds: Bounds<Pixels>,
    image: Arc<RenderImage>,
}

/// An in-flight toolbar drag: `grab` is the press offset from the
/// toolbar's origin (press − origin, window-local) so the toolbar
/// follows the pointer without jumping; `restore` is the pre-drag
/// position override (None = anchored) for Esc.
#[derive(Clone, Copy)]
struct ToolbarDrag {
    grab: Point<Pixels>,
    restore: Option<Point<Pixels>>,
}

pub struct ScreenshotSession {
    screens: Vec<Screen>,
    selection: Selection,
    active_output: Option<String>,
    /// The pointer's last known position in GLOBAL desktop coordinates,
    /// reported by whichever window actually receives its events. Under
    /// Wayland's implicit grab a gesture's events all go to the PRESS
    /// window — the window the pointer physically sits over may have
    /// never seen a single move. Cursor affordances therefore read THIS
    /// (mapped into their own window), not a per-window position, or a
    /// state flip that lands chrome under a "fresh" window shows a
    /// stale cursor until the user wiggles the mouse.
    pointer_global: Option<Point<Pixels>>,
    /// A press that landed on a selectable annotation shape, waiting to
    /// resolve: release without motion CLICKS (select), motion past
    /// CLICK_SLOP converts it into a normal draw stroke starting at the
    /// press point (draw-through — pressing on a shape never blocks
    /// drawing, issue #5).
    pending_click: Option<(usize, Point<Pixels>)>,
    /// Drag-move of the selected annotation in flight (phase B). Set
    /// when a press on the ALREADY-SELECTED shape moves past the click
    /// slop; the shape follows the pointer until release.
    moving: Option<MoveDrag>,
    /// Handle drag of the selected annotation in flight (phase C):
    /// grabbed an endpoint/vertex/corner of the selected shape.
    handle_drag: Option<HandleDrag>,
    blocked: bool,
    /// The user-dragged toolbar position (window-local to the ACTIVE
    /// output); None → the placement anchor decides. Reset by a NEW
    /// selection or a change of host window — moving/resizing the
    /// current selection keeps it (the user put it there deliberately).
    toolbar_pos: Option<Point<Pixels>>,
    /// Set while the toolbar is being dragged by an edge grip.
    toolbar_drag: Option<ToolbarDrag>,
    /// The grip strips as last PAINTED on this output (window-local),
    /// reported by the measure canvas inside each grip element at paint
    /// time. The cursor's grab affordance reads THIS, not the basis
    /// derivation in [`ScreenshotSession::toolbar_grips`] — a stale
    /// width basis once put the right strip ~70px left of the rendered
    /// grip, over the buttons (user-reported; the two-row state sized
    /// the bar at the pre-customization TB_W while row one had grown to
    /// its configured width). One frame stale after any geometry
    /// change (paint runs after the render that reads it); the
    /// derivation stays as the first-frame fallback.
    toolbar_grips_reported: Option<(SharedString, Bounds<Pixels>, Bounds<Pixels>)>,
    /// Window-snap targets in global logical coordinates; empty when the
    /// compositor exposes no supported IPC — see [`crate::platform::windowsnap`]
    snaps: Vec<crate::platform::windowsnap::SnapRect>,
    /// The hover outline's rect (GLOBAL logical coordinates), whichever
    /// source produced it: a smart detected region (which includes the
    /// snap windows injected as candidates) or, before detection has
    /// landed, a raw compositor snap rect.
    hovered: Option<Bounds<Pixels>>,
    /// Smart-selection hover state (debounce + manual wheel level) —
    /// see [`crate::model::regiondetect::SelectionCursor`]. Lives in the
    /// owning screen's PHYSICAL pixel space; `cursor_screen` pins it.
    smart_cursor: crate::model::regiondetect::SelectionCursor,
    cursor_screen: Option<usize>,
    /// Press point of the ongoing interaction (global). The click-snap
    /// hit-tests the PRESS position, not wherever a jittery release lands
    press: Option<Point<Pixels>>,
    annotations: crate::annotation::Annotations,
    filter_preview: std::cell::RefCell<Option<FilterPreview>>,
    preview_busy: bool,
    preview_display: Option<DisplayPreview>,
    geometry_revision: u64,
}

impl ScreenshotSession {
    pub fn new(
        captures: Vec<Arc<Capture>>,
        snaps: Vec<crate::platform::windowsnap::SnapRect>,
    ) -> Self {
        Self {
            screens: captures
                .into_iter()
                .map(|capture| Screen {
                    logical_size: {
                        let (w, h) = capture.logical_size_f32();
                        size(px(w), px(h))
                    },
                    capture,
                    detector: None,
                })
                .collect(),
            selection: Selection::Idle,
            active_output: None,
            pointer_global: None,
            pending_click: None,
            moving: None,
            handle_drag: None,
            blocked: false,
            toolbar_pos: None,
            toolbar_drag: None,
            toolbar_grips_reported: None,
            snaps,
            hovered: None,
            smart_cursor: Default::default(),
            cursor_screen: None,
            press: None,
            annotations: Default::default(),
            filter_preview: Default::default(),
            preview_busy: false,
            preview_display: None,
            geometry_revision: 0,
        }
    }

    fn screen(&self, name: &str) -> &Screen {
        self.screens
            .iter()
            .find(|s| s.capture.output_name == name)
            .expect("registered overlay output")
    }

    /// Full-screen selection: the union of every screen's bounds — the
    /// `full` subcommand's non-interactive path. A cross-screen union
    /// exports at the highest participating density with transparent
    /// gaps, exactly like a user-drawn spanning selection.
    pub fn select_all(&mut self) {
        if let Some(bounds) = self
            .screens
            .iter()
            .map(|s| s.bounds())
            .reduce(|a, b| a.union(&b))
        {
            let first = self
                .screens
                .first()
                .map(|s| s.capture.output_name.clone())
                .unwrap_or_default();
            self.set_active_output(&first);
            self.selection = Selection::Selected { bounds };
        }
    }

    /// Ctrl+A: cycle the selection between "this whole screen" and
    /// "every screen". Any state (idle, partial drag, another screen's
    /// selection) lands on the current screen first — that reads as
    /// "select all" to a user looking at one monitor — the second press
    /// spans everything, the third wraps back. Returns whether the
    /// selection changed.
    pub(crate) fn cycle_select_all(&mut self, name: &str) -> bool {
        let Some(screen_bounds) = self
            .screens
            .iter()
            .find(|s| s.capture.output_name == name)
            .map(|s| s.bounds())
        else {
            return false; // unregistered output — overlay contract broken
        };
        let Some(union) = self
            .screens
            .iter()
            .map(|s| s.bounds())
            .reduce(|a, b| a.union(&b))
        else {
            return false;
        };
        let current = self.selection.bounds();
        let next = if current == Some(union) {
            screen_bounds // all → this screen (wrap)
        } else if current == Some(screen_bounds) {
            union // this screen → all
        } else {
            screen_bounds // idle / partial / elsewhere → this screen
        };
        self.toolbar_pos = None; // the selection was replaced: re-anchor
        self.set_active_output(name);
        self.selection = Selection::Selected { bounds: next };
        true
    }

    pub(crate) fn set_size(&mut self, name: &str, logical_size: Size<Pixels>) -> bool {
        if logical_size.width <= px(0.) || logical_size.height <= px(0.) {
            return false;
        }
        let screen = self
            .screens
            .iter_mut()
            .find(|s| s.capture.output_name == name)
            .unwrap();
        if screen.logical_size == logical_size {
            return false;
        }
        screen.logical_size = logical_size;
        self.geometry_revision += 1;
        self.preview_display = None;
        self.filter_preview.get_mut().take();
        true
    }

    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }
    pub(crate) fn blocked(&self) -> bool {
        self.blocked
    }
    pub(crate) fn set_blocked(&mut self, blocked: bool) {
        self.blocked = blocked;
    }
    pub(crate) fn active_on(&self, name: &str) -> bool {
        self.active_output.as_deref() == Some(name)
    }

    /// Change the host window of the toolbar-carrying selection. A
    /// dragged toolbar position is LOCAL to its host window — a new host
    /// must re-anchor (the same coordinates would mean somewhere else
    /// entirely on another screen).
    fn set_active_output(&mut self, name: &str) {
        if self.active_output.as_deref() != Some(name) {
            self.toolbar_pos = None;
        }
        self.active_output = Some(name.to_owned());
    }

    /// A finalized selection must carry its chrome with it. The size
    /// label and toolbar render only on the active output — but a
    /// move/resize edit (or a fresh drag) released with the selection
    /// living on a DIFFERENT output leaves `active_output` stale:
    /// Wayland's implicit grab delivers the whole gesture to the window
    /// where the press happened, and only `pointer_down` re-hosts. The
    /// result was both screens blank (the old one no longer intersects
    /// the selection, the new one is not "active") until the next click
    /// bailed it out. Follow the selection to the output holding its
    /// largest intersection. STICKY: the incumbent host wins ties — an
    /// ambiguous straddle across the seam must not churn the chrome to
    /// the other window (and a same-host call is a no-op, keeping any
    /// dragged toolbar position).
    fn follow_selection_host(&mut self) {
        let Some(bounds) = self.selection.bounds() else {
            return; // no finalized selection: nothing to follow
        };
        let overlap = |screen: &Screen| {
            let o = screen.bounds().intersect(&bounds);
            f32::from(o.size.width) * f32::from(o.size.height)
        };
        let incumbent = self
            .active_output
            .as_deref()
            .and_then(|name| self.screens.iter().find(|s| s.capture.output_name == name))
            .map(overlap)
            .unwrap_or(0.);
        // only a STRICTLY larger intersection dethrones the incumbent
        let challenger = self
            .screens
            .iter()
            .filter(|s| overlap(s) > incumbent)
            .map(|s| (s.capture.output_name.clone(), overlap(s)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        if let Some((name, _)) = challenger {
            self.set_active_output(&name);
        }
    }

    pub(crate) fn local_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        let screen = self.screen(name).bounds();
        let mut bounds = self.selection.bounds()?.intersect(&screen);
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return None;
        }
        bounds.origin -= screen.origin;
        Some(bounds)
    }

    pub(crate) fn backdrop_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        self.local_bounds(name)?;
        let mut bounds = self.selection.bounds()?;
        bounds.origin -= self.screen(name).bounds().origin;
        Some(bounds)
    }

    pub(crate) fn begin(&mut self, name: &str, local: Point<Pixels>) {
        if self.blocked {
            return;
        }
        let global = local + self.screen(name).bounds().origin;
        self.press = Some(global);
        self.hovered = None; // the outline steps aside for the real interaction
        self.annotations.reset();
        self.toolbar_pos = None; // a NEW selection re-anchors the toolbar
        self.set_active_output(name);
        self.selection.begin(global);
    }

    /// The union of every screen's bounds — the "desktop" a selection can
    /// occupy. Move/resize clamps to it (a selection cannot leave the
    /// captured area).
    fn desktop_bounds(&self) -> Option<Bounds<Pixels>> {
        self.screens
            .iter()
            .map(|s| s.bounds())
            .reduce(|a, b| a.union(&b))
    }

    /// Window-local → global logical coordinates for one output.
    pub(crate) fn to_global(&self, name: &str, local: Point<Pixels>) -> Point<Pixels> {
        local + self.screen(name).bounds().origin
    }

    pub(crate) fn screen_origin(&self, name: &str) -> Point<Pixels> {
        self.screen(name).bounds().origin
    }

    /// The pointer's last known global position — see the field docs.
    pub(crate) fn pointer_global(&self) -> Option<Point<Pixels>> {
        self.pointer_global
    }

    /// That position mapped into this output's LOCAL coordinates
    /// (unclamped: the pointer may legitimately sit over another
    /// output, in which case the mapped point lies outside this
    /// window — hit-tests simply miss).
    pub(crate) fn pointer_in(&self, name: &str) -> Option<Point<Pixels>> {
        Some(self.pointer_global? - self.screen(name).bounds().origin)
    }

    /// This output's overlay window size (logical px), as last reported by
    /// [`ScreenshotSession::set_size`]. Chrome geometry (toolbar anchor)
    /// needs it outside render — e.g. the cursor's toolbar hit-test.
    pub(crate) fn overlay_size(&self, name: &str) -> Option<Size<Pixels>> {
        self.screens
            .iter()
            .find(|s| s.capture.output_name == name)
            .map(|s| s.logical_size)
            .filter(|s| s.width > px(0.) && s.height > px(0.))
    }

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

    pub(crate) fn drag_to(&mut self, name: &str, local: Point<Pixels>) -> bool {
        if self.blocked {
            return false;
        }
        self.selection
            .drag_to(local + self.screen(name).bounds().origin)
    }

    pub(crate) fn end(&mut self, name: &str, local: Point<Pixels>) {
        if self.blocked {
            return;
        }
        self.selection
            .end(local + self.screen(name).bounds().origin);
        // An in-place click (< selection::MIN_SIZE) leaves Idle — if a
        // smart region or a compositor window sits under the PRESS
        // point, select its rect instead (smart selection / window
        // snapping). A real drag still wins: freehand beats snap.
        if !self.selection.is_dragging()
            && !self.selection.is_selected()
            && let Some(press) = self.press
        {
            let snapped = self.smart_region_at(press).or_else(|| {
                crate::platform::windowsnap::hit_test(&self.snaps, press)
                    .map(|hit| self.snaps[hit].bounds)
            });
            if let Some(bounds) = snapped {
                self.selection = Selection::Selected { bounds };
            }
            self.smart_cursor.reset();
        }
        self.press = None;
        // A fresh drag can finish over the seam on another output — the
        // chrome host must follow the selection there, exactly like an
        // edit-release does (see `follow_selection_host`).
        self.follow_selection_host();
    }

    /// Track the region under the cursor for the hover outline. Returns
    /// whether the hover changed (callers decide on cx.notify()).
    /// Suppressed while dragging — the freehand region takes over — and
    /// while a modal is open. A FINALIZED selection ends the phase
    /// entirely: once the user has picked a region (drawn or snapped),
    /// hunting for more candidates under the pointer is noise, not a
    /// hint. The smart layer answers first (it has the compositor
    /// windows injected as candidates); the raw snap rects are the
    /// fallback until its background analysis lands.
    pub(crate) fn hover_at(&mut self, name: &str, local: Point<Pixels>) -> bool {
        if self.blocked
            || self.selection.is_dragging()
            || self.selection.is_selected()
            || self.selection.is_editing()
            || self.toolbar_drag.is_some()
            || self.annotations.enabled()
        {
            return false;
        }
        let global = local + self.screen(name).bounds().origin;
        let next = self.smart_region_at(global).or_else(|| {
            crate::platform::windowsnap::hit_test(&self.snaps, global)
                .map(|hit| self.snaps[hit].bounds)
        });
        if next == self.hovered {
            return false;
        }
        self.hovered = next;
        true
    }

    /// The hovered region's rect in this output's local coordinates, for
    /// the hover outline; None when not hovering anything visible here.
    /// Also None once a selection is finalized — a stale `hovered` set
    /// before the selection landed (Ctrl+A mid-hover) must not paint.
    pub(crate) fn hover_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        if self.blocked || self.selection.is_selected() {
            return None;
        }
        let screen = self.screen(name).bounds();
        let mut b = self.hovered?.intersect(&screen);
        if b.size.width <= px(0.) || b.size.height <= px(0.) {
            return None;
        }
        b.origin -= screen.origin;
        Some(b)
    }

    pub(crate) fn cancel_drag(&mut self) {
        if let Some(drag) = self.toolbar_drag.take() {
            // Esc mid-toolbar-drag: back to where it was. One Esc, one
            // thing — the selection below is untouched.
            self.toolbar_pos = drag.restore;
            return;
        }
        self.press = None;
        self.hovered = None;
        self.smart_cursor.reset();
        self.selection.cancel_drag();
    }

    pub(crate) fn annotations(&self) -> &crate::annotation::Annotations {
        &self.annotations
    }
}

mod annotations;
mod export;
mod smart;
#[cfg(test)]
mod tests {
    use super::ScreenshotSession;
    use crate::model::selection::Selection;
    use crate::model::session::ScrollRect;
    use crate::platform::capture::Capture;
    use gpui_kit::{Bounds, point, px, size};
    use std::sync::Arc;
    #[gpui_kit::test]
    fn background_preview_preserves_release_and_never_reuses_a_cancelled_gesture(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::annotation::ShapeKind;
        use gpui_kit::AppContext;
        let mut cap = Capture::for_test((0, 0), 1.);
        cap.output_name = "left".into();
        cap.width = 600;
        cap.height = 600;
        cap.rgba = vec![255; 600 * 600 * 4];
        let session = cx.new(|_| ScreenshotSession::new(vec![Arc::new(cap)], Vec::new()));
        session.update(cx, |s, cx| {
            s.select_all();
            s.edit_annotations(|a| a.toggle(ShapeKind::Blur));
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(580.), px(580.)), false);
            s.filtered_preview("left").unwrap();
            s.pointer_move("left", point(px(600.), px(600.)), false);
            assert!(s.request_filtered_preview("left", cx).is_some());
            assert!(s.preview_busy);
            s.pointer_up("left", point(px(600.), px(600.)), false);
            // Keep the last completed preview visible until the final one arrives.
            assert!(s.request_filtered_preview("left", cx).is_some());
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            // the committed blur auto-selected (select-on-place); this
            // second gesture must DRAW again, not move it
            s.edit_annotations(|a| a.deselect());
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(580.), px(580.)), false);
            s.filtered_preview("left").unwrap();
            s.pointer_move("left", point(px(600.), px(600.)), false);
            assert!(s.request_filtered_preview("left", cx).is_some());
            s.cancel_annotation();
            // cancel cleared the draft, but the auto-selection from the
            // first commit is still live — drop it so this is a DRAW
            s.edit_annotations(|a| a.deselect());
            // Same tool and same start position, but a distinct gesture.
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(550.), px(550.)), false);
            assert!(s.request_filtered_preview("left", cx).is_none());
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            assert!(!s.preview_busy);
            assert!(s.request_filtered_preview("left", cx).is_some());
            assert_eq!(
                s.filter_preview.borrow().as_ref().unwrap().draft_generation,
                s.annotations.draft_generation()
            );
        });
    }

    #[gpui_kit::test]
    fn background_preview_coalesces_updates_and_rejects_cancelled_geometry(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::annotation::ShapeKind;
        use gpui_kit::AppContext;
        let mut cap = Capture::for_test((0, 0), 1.);
        cap.output_name = "left".into();
        cap.width = 512;
        cap.height = 512;
        cap.rgba = (0..512 * 512)
            .flat_map(|i| [(i % 251) as u8, 70, 140, 255])
            .collect();
        let session = cx.new(|_| ScreenshotSession::new(vec![Arc::new(cap)], Vec::new()));
        session.update(cx, |s, cx| {
            s.select_all();
            s.edit_annotations(|a| a.toggle(ShapeKind::Blur));
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(512.), px(512.)), false);
            assert!(s.request_filtered_preview("left", cx).is_none());
            assert!(s.preview_busy);
            for i in 0..100 {
                s.pointer_move("left", point(px(400. + i as f32), px(500.)), false);
                s.request_filtered_preview("left", cx);
                assert!(s.preview_busy);
            }
            // Export is always computed from current model state, even while the
            // preview worker is handling an older snapshot.
            s.pointer_up("left", point(px(500.), px(500.)), false);
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            assert!(!s.preview_busy);
            let (_, image) = s.request_filtered_preview("left", cx).unwrap();
            let expected = s.crop("left").unwrap().2;
            let expected: Vec<_> = expected
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect();
            assert_eq!(image.as_bytes(0).unwrap(), expected);
            // the first blur auto-selected on commit (select-on-place);
            // this second gesture must DRAW again, not move it
            s.edit_annotations(|a| a.deselect());
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(512.), px(512.)), false);
            s.request_filtered_preview("left", cx);
            assert!(s.preview_busy);
            s.cancel_annotation();
            s.edit_annotations(|a| a.undo());
            assert!(s.request_filtered_preview("left", cx).is_none());
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            assert!(!s.preview_busy);
            assert!(s.request_filtered_preview("left", cx).is_none());
            assert!(s.filter_preview.borrow().is_none());
            // Exercise raster jobs directly: pressing on an existing blur
            // now selects/moves it instead of starting another stroke.
            let selection = s.selection.bounds().unwrap();
            s.edit_annotations(|a| a.begin(point(px(0.), px(0.)), selection, false));
            s.pointer_move("left", point(px(512.), px(512.)), false);
            s.request_filtered_preview("left", cx);
            assert!(s.preview_busy);
            s.set_size("left", size(px(400.), px(400.)));
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            assert!(!s.preview_busy);
            let (_, actual) = s.request_filtered_preview("left", cx).unwrap();
            let expected = s.crop("left").unwrap().2;
            let expected: Vec<_> = expected
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect();
            assert_eq!(actual.as_bytes(0).unwrap(), expected);
        });
    }

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
    fn pin_crop_keeps_global_geometry_independent_of_trigger_output() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(40.), px(90.)));
        let expected = Bounds::new(point(px(-20.), px(40.)), size(px(60.), px(50.)));
        for output in ["left", "right"] {
            let crop = s.crop_for_pin(output).unwrap();
            assert_eq!(crop.bounds, expected);
            assert_eq!((crop.width, crop.height), (120, 100));
            assert_eq!(crop.rgba, s.crop(output).unwrap().2);
        }
        // Native-pixel rounding and clipping must also be reflected in the
        // pin's origin, rather than using the unrounded requested selection.
        let mut s = ScreenshotSession::new(
            vec![screen("single", (-100, 20), 1.25, [255; 4])],
            Vec::new(),
        );
        s.begin("single", point(px(10.3), px(11.6)));
        s.end("single", point(px(50.3), px(61.6)));
        let crop = s.crop_for_pin("single").unwrap();
        assert_eq!(crop.bounds.origin, point(px(-89.6), px(32.)));
        assert_eq!(
            crop.bounds.size,
            size(px(crop.width as f32 / 1.25), px(crop.height as f32 / 1.25))
        );
    }

    fn snap(x: f32, y: f32, w: f32, h: f32) -> crate::platform::windowsnap::SnapRect {
        crate::platform::windowsnap::SnapRect {
            bounds: Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            },
            app_id: "fixture".into(),
            focused: false,
            recency: 0,
        }
    }

    #[test]
    fn ctrl_a_cycles_screen_then_union_then_wraps() {
        let mut s = session();
        let right = s.screens[1].bounds(); // "right" is the fixture's focused-ish screen
        let union = s
            .screens
            .iter()
            .map(|sc| sc.bounds())
            .reduce(|a, b| a.union(&b))
            .unwrap();

        // idle → this screen
        assert!(s.cycle_select_all("right"));
        assert_eq!(s.selection().bounds(), Some(right));
        // this screen → every screen
        assert!(s.cycle_select_all("right"));
        assert_eq!(s.selection().bounds(), Some(union));
        // all → wraps back to this screen
        assert!(s.cycle_select_all("right"));
        assert_eq!(s.selection().bounds(), Some(right));
    }

    #[test]
    fn ctrl_a_from_a_partial_selection_lands_on_the_whole_screen() {
        let mut s = session();
        // a user-drawn partial rectangle somewhere else
        s.selection = Selection::Selected {
            bounds: Bounds {
                origin: point(px(-40.), px(60.)),
                size: size(px(100.), px(50.)),
            },
        };
        assert!(s.cycle_select_all("left"));
        assert_eq!(s.selection().bounds(), Some(s.screens[0].bounds()));
        // and anchors the active output
        assert_eq!(s.active_output.as_deref(), Some("left"));
    }

    #[test]
    fn scroll_region_maps_local_logical_and_refuses_cross_output() {
        let mut s = session();
        // fully inside "right" (global (0,0)..(100,100), logical)
        s.selection = Selection::Selected {
            bounds: Bounds {
                origin: point(px(10.), px(20.)),
                size: size(px(50.), px(40.)),
            },
        };
        assert_eq!(
            s.scroll_region("right"),
            Some(ScrollRect {
                x: 10,
                y: 20,
                width: 50,
                height: 40
            })
        );
        // the same selection is not "right"+'s to capture from another
        // output's perspective, and a selection hanging off the screen
        // edge is refused outright (no partial clamping — the scroll
        // target would be ambiguous)
        assert_eq!(s.scroll_region("left"), None);
        s.selection = Selection::Selected {
            bounds: Bounds {
                origin: point(px(-30.), px(10.)),
                size: size(px(100.), px(30.)),
            },
        };
        assert_eq!(s.scroll_region("left"), None);
        assert_eq!(s.scroll_region("right"), None);
        // degenerate (no selection): nothing to scroll
        s.selection = Selection::Idle;
        assert_eq!(s.scroll_region("right"), None);
    }

    #[test]
    fn ctrl_a_on_an_unknown_output_is_a_no_op() {
        let mut s = session();
        assert!(!s.cycle_select_all("nope"));
        assert!(s.selection().bounds().is_none());
    }

    #[test]
    fn select_all_spans_every_screen_with_a_ready_selection() {
        let mut s = session();
        assert!(s.selection().bounds().is_none());
        s.select_all();
        let expected = s
            .screens
            .iter()
            .map(|sc| sc.bounds())
            .reduce(|a, b| a.union(&b))
            .unwrap();
        assert_eq!(s.selection().bounds(), Some(expected));
        // And it must rasterize through the normal export path
        let (w, h, _) = s.crop_original("right").unwrap();
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn selecting_another_output_replaces_the_previous_selection() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.)));
        assert!(s.local_bounds("left").is_some());
        s.begin("right", point(px(10.), px(10.)));
        s.end("right", point(px(30.), px(30.)));
        assert!(s.local_bounds("left").is_none());
        assert!(s.local_bounds("right").is_some());
        assert!(!s.active_on("left"));
        // A shortcut delivered to the old window still exports the new selection.
        let (w, h, pixels) = s.crop("left").unwrap();
        assert_eq!((w, h), (40, 40));
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 255, 0, 255])
        );
    }

    #[test]
    fn cross_output_drag_normalizes_negative_origins_and_mixed_dpi() {
        let mut s = session();
        s.begin("right", point(px(20.), px(60.)));
        assert!(s.drag_to("left", point(px(80.), px(20.))));
        s.end("left", point(px(80.), px(20.)));
        let bounds = s.selection().bounds().unwrap();
        assert_eq!(bounds.origin, point(px(-20.), px(40.)));
        assert_eq!(bounds.size, size(px(40.), px(20.)));
        let (w, h, pixels) = s.crop("left").unwrap();
        assert_eq!((w, h), (80, 40));
        for row in pixels.chunks_exact(w as usize * 4) {
            assert!(
                row[..40 * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == [255, 0, 0, 255])
            );
            assert!(
                row[40 * 4..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == [0, 255, 0, 255])
            );
        }
    }

    #[test]
    fn drag_grab_can_finish_outside_its_original_window() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("left", point(px(120.), px(40.)));
        assert!(s.selection.is_selected());
        assert!(s.local_bounds("right").is_some());
        assert_eq!(s.crop("left").unwrap().0, 80);
    }

    #[test]
    fn chrome_follows_a_selection_moved_to_another_output() {
        let mut s = session();
        s.set_size("left", size(px(100.), px(100.)));
        s.set_size("right", size(px(100.), px(100.)));
        // a selection fully on "right", made there: chrome lives there
        s.begin("right", point(px(10.), px(10.)));
        s.end("right", point(px(60.), px(60.)));
        assert!(s.active_on("right"));
        assert!(s.toolbar_bounds("right").is_some());

        // move it across the seam: press inside on right's window, drag
        // and release with the pointer already on "left" — Wayland's
        // implicit grab delivers the WHOLE gesture to the press window
        s.pointer_down("right", point(px(30.), px(30.)), false);
        assert!(s.selection.is_editing());
        s.pointer_move("right", point(px(-40.), px(50.)), false);
        s.pointer_up("right", point(px(-40.), px(50.)), false);

        // regression: without the rehost both screens rendered nothing
        // (old host no longer intersects, new host not "active") until
        // the next click re-hosted via pointer_down
        assert!(s.selection.is_selected());
        assert!(s.local_bounds("left").is_some(), "label render input");
        assert!(s.active_on("left"), "the chrome host follows the selection");
        assert!(
            s.toolbar_bounds("left").is_some(),
            "toolbar re-hosts on release"
        );
        assert!(s.toolbar_bounds("right").is_none());

        // a FRESH drag released with the bulk on another output rehosts
        // too (press screen ≠ host screen from the start)
        s.begin("right", point(px(5.), px(40.)));
        s.drag_to("right", point(px(-50.), px(60.)));
        s.end("right", point(px(-50.), px(60.)));
        assert!(s.active_on("left"), "largest-intersection output wins");
        assert!(s.toolbar_bounds("left").is_some());
    }

    #[test]
    fn desktop_gaps_are_transparent_and_fractional_scale_uses_window_size() {
        let mut s = ScreenshotSession::new(
            vec![
                screen("top", (0, 0), 1.25, [255, 0, 0, 255]),
                screen("bottom", (0, 120), 1.5, [0, 255, 0, 255]),
            ],
            Vec::new(),
        );
        s.set_size("top", size(px(100.), px(100.)));
        s.set_size("bottom", size(px(100.), px(100.)));
        s.begin("top", point(px(10.), px(90.)));
        s.end("bottom", point(px(30.), px(10.)));
        let (w, h, pixels) = s.crop("top").unwrap();
        assert_eq!((w, h), (30, 60));
        assert!(pixels[15 * 30 * 4..45 * 30 * 4].iter().all(|b| *b == 0));
    }

    #[test]
    fn modal_state_blocks_all_outputs_and_cancel_is_shared() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.drag_to("left", point(px(40.), px(40.)));
        s.set_blocked(true);
        s.begin("right", point(px(20.), px(20.)));
        assert!(!s.drag_to("right", point(px(70.), px(70.))));
        s.end("right", point(px(70.), px(70.)));
        assert!(s.active_on("left"));
        assert!(s.selection().is_dragging());
        s.set_blocked(false);
        s.cancel_drag();
        assert!(s.local_bounds("left").is_none());
        assert!(s.local_bounds("right").is_none());
    }

    // ── Window snapping ────────────────────────────────────────────

    /// "right" spans global (0,0)-(100,100) logical; one snap window at
    /// (20,30) sized 40x50 sits on it.
    fn snapped_session() -> ScreenshotSession {
        ScreenshotSession::new(
            vec![screen("right", (0, 0), 2., [0, 255, 0, 255])],
            vec![snap(20., 30., 40., 50.)],
        )
    }

    #[test]
    fn click_on_a_window_snaps_the_selection_to_its_rect() {
        let mut s = snapped_session();
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(51.), px(51.))); // < 2px: a click, not a drag
        assert!(s.selection().is_selected());
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(40.), px(50.)));
        // crop lands in physical pixels (scale 2)
        assert_eq!(s.crop("right").unwrap().0, 80);
    }

    #[test]
    fn click_off_windows_still_clears() {
        let mut s = snapped_session();
        s.begin("right", point(px(5.), px(5.))); // click outside every window
        s.end("right", point(px(6.), px(6.)));
        assert!(!s.selection().is_selected());
    }

    #[test]
    fn real_drag_over_a_window_still_wins() {
        let mut s = snapped_session();
        s.begin("right", point(px(50.), px(50.))); // inside the window
        s.end("right", point(px(90.), px(90.))); // real drag: freehand beats snap
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(50.), px(50.)));
        assert_eq!(b.size, size(px(40.), px(40.)));
    }

    #[test]
    fn hover_tracks_windows_and_yields_local_bounds() {
        let mut s = ScreenshotSession::new(
            vec![
                screen("left", (-100, 20), 1., [255, 0, 0, 255]),
                screen("right", (0, 0), 2., [0, 255, 0, 255]),
            ],
            vec![snap(20., 30., 40., 50.)],
        );
        // entering / leaving flips the hover
        assert!(s.hover_at("right", point(px(30.), px(40.))));
        assert!(!s.hover_at("right", point(px(31.), px(41.)))); // same window
        let b = s.hover_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(40.), px(50.)));
        assert!(s.hover_bounds("left").is_none()); // rect lives on right
        assert!(s.hover_at("right", point(px(5.), px(5.)))); // leave → None
        assert!(s.hover_bounds("right").is_none());

        // suppressed while dragging; press clears the outline
        s.hover_at("right", point(px(30.), px(40.)));
        s.begin("right", point(px(30.), px(40.)));
        assert!(!s.hover_at("right", point(px(35.), px(45.))));
        assert!(s.hover_bounds("right").is_none());
    }

    // ── Smart regions (pixel-based candidates) ────────────────────

    /// "right": global (0,0)-(100,100) logical, scale 2 → 200×200
    /// physical. One detected UI region at logical (20,30)-(100,70)
    /// (physical 40,60-200,140) plus a snap window at (0,0)-(100,80)
    /// that `set_region_detector` injects as a parent candidate.
    fn smart_session() -> ScreenshotSession {
        use crate::model::regiondetect::{IntRect, RegionCandidate, RegionDetector, Sources};
        let mut s = ScreenshotSession::new(
            vec![screen("right", (0, 0), 2., [0, 255, 0, 255])],
            vec![snap(0., 0., 100., 80.)],
        );
        s.set_region_detector(
            "right",
            RegionDetector::from_candidates(
                200,
                200,
                vec![RegionCandidate::synthetic(
                    IntRect::new(40, 60, 200, 140),
                    0.9,
                    Sources::CONTOUR,
                )],
            ),
        );
        s
    }

    #[test]
    fn smart_hover_outlines_the_detected_region() {
        let mut s = smart_session();
        // entering the region: outline in local coordinates
        assert!(s.hover_at("right", point(px(50.), px(50.))));
        let b = s.hover_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(80.), px(40.)));
        // same region: no repaint
        assert!(!s.hover_at("right", point(px(51.), px(51.))));
        // inside the injected window but outside the region: the window
        // candidate (the smart layer) outlines the whole window
        assert!(s.hover_at("right", point(px(5.), px(5.))));
        let b = s.hover_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(0.), px(0.)));
        assert_eq!(b.size, size(px(100.), px(80.)));
        // a point no candidate covers: no outline (the plain-snap
        // fallback path is covered by hover_tracks_windows_…)
        assert!(s.hover_at("right", point(px(5.), px(90.))));
        assert!(s.hover_bounds("right").is_none());
    }

    #[test]
    fn click_snaps_to_the_smart_region_and_drag_still_wins() {
        let mut s = smart_session();
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(51.), px(51.))); // click → the region
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(80.), px(40.)));
        // freehand still beats snapping
        s.cancel_drag();
        s.begin("right", point(px(50.), px(50.)));
        s.drag_to("right", point(px(90.), px(90.)));
        s.end("right", point(px(90.), px(90.)));
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(50.), px(50.)));
        assert_eq!(b.size, size(px(40.), px(40.)));
    }

    #[test]
    fn wheel_steps_the_smart_hierarchy_and_click_keeps_the_level() {
        let mut s = smart_session();
        s.pointer_move("right", point(px(50.), px(50.)), false);
        // up: the detected region → its parent (the injected window)
        assert!(s.smart_step(1));
        let b = s.hover_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(0.), px(0.)));
        assert_eq!(b.size, size(px(100.), px(80.)));
        // up again: no larger parent → the whole screen
        assert!(s.smart_step(1));
        assert_eq!(
            s.hover_bounds("right").unwrap().size,
            size(px(100.), px(100.))
        );
        // down: back through the window into the region
        assert!(s.smart_step(-1));
        assert_eq!(
            s.hover_bounds("right").unwrap().size,
            size(px(100.), px(80.))
        );
        assert!(s.smart_step(-1));
        let b = s.hover_bounds("right").unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(80.), px(40.)));
        // a click accepts the wheel-chosen level (the region)
        s.begin("right", point(px(50.), px(50.)));
        s.end("right", point(px(51.), px(51.)));
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(20.), px(30.)));
        assert_eq!(b.size, size(px(80.), px(40.)));
    }

    #[test]
    fn smart_step_is_gated_to_the_selection_phase() {
        let mut s = smart_session();
        assert!(!s.smart_step(1)); // no pointer seen yet
        s.pointer_move("right", point(px(50.), px(50.)), false);
        assert!(s.smart_step(1));
        // a finalized selection ends the phase
        s.begin("right", point(px(10.), px(10.)));
        s.end("right", point(px(60.), px(60.)));
        assert!(s.selection().is_selected());
        assert!(!s.smart_step(1));
        // so does a lit annotation tool (the wheel steps its size there)
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));
        assert!(!s.smart_step(-1));
    }

    #[test]
    fn hover_stops_once_a_selection_exists() {
        let mut s = smart_session();
        assert!(s.hover_at("right", point(px(50.), px(50.))));
        assert!(s.hover_bounds("right").is_some());
        // a user-drawn freehand selection ends the smart phase: no more
        // candidate hunting under the pointer
        s.begin("right", point(px(5.), px(5.)));
        s.drag_to("right", point(px(30.), px(30.)));
        s.end("right", point(px(30.), px(30.)));
        assert!(s.selection().is_selected());
        assert!(!s.hover_at("right", point(px(50.), px(50.))));
        assert!(s.hover_bounds("right").is_none());
        // even a stale hover (set before the selection landed, e.g.
        // Ctrl+A mid-hover) must not paint
        let mut s = smart_session();
        assert!(s.hover_at("right", point(px(50.), px(50.))));
        s.cycle_select_all("right");
        assert!(s.selection().is_selected());
        assert!(s.hover_bounds("right").is_none());
    }

    // ── In-place selection editing (move / resize) ────────────────

    #[test]
    fn move_selection_after_release_preserves_annotations() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.))); // global (-90,30) 20×20
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));
        s.pointer_down("left", point(px(12.), px(12.)), false);
        s.pointer_up("left", point(px(28.), px(28.)), false);
        assert_eq!(s.annotations().visible().count(), 1);
        // untoggle: back to selection mode (editing only works without a tool)
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));

        // press the interior, drag, release → translated, annotations intact
        s.pointer_down("left", point(px(20.), px(20.)), false); // global (-80,40): interior
        assert!(s.selection().is_editing());
        assert!(!s.selection().is_selected()); // toolbar hides mid-edit
        s.pointer_move("left", point(px(40.), px(40.)), false); // global (-60,60)
        s.pointer_up("left", point(px(40.), px(40.)), false);
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(-70.), px(50.))); // +20,+20
        assert_eq!(b.size, size(px(20.), px(20.)));
        assert!(s.selection().is_selected());
        assert_eq!(s.annotations().visible().count(), 1); // NOT reset by the edit
    }

    #[test]
    fn resize_selection_by_corner_handle() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.))); // global (-90,30) 20×20
        // press right at the bottom-right corner (within the 8px band)
        s.pointer_down("left", point(px(30.), px(30.)), false);
        assert!(s.selection().is_editing());
        s.pointer_move("left", point(px(50.), px(60.)), false); // global (-50,80)
        s.pointer_up("left", point(px(50.), px(60.)), false);
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(-90.), px(30.))); // top-left pinned
        assert_eq!(b.size, size(px(40.), px(50.)));
        // and the export path follows the new bounds
        assert_eq!(s.crop("left").unwrap().0, 40);
    }

    #[test]
    fn loupe_tracks_the_dragged_selection_corner() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.))); // global (-90,30) 20×20
        // grab the bottom-right corner, drag to local (50,60) →
        // global (-50,80)
        s.pointer_down("left", point(px(30.), px(30.)), false);
        s.pointer_move("left", point(px(50.), px(60.)), false);
        let loupe = s.local_loupe("left").expect("corner drag → loupe");
        assert_eq!(loupe.focus, point(px(50.), px(60.))); // global − left origin
        assert_eq!(loupe.outward, (1., 1.)); // BR → away is down-right
        assert!(s.local_loupe("right").is_none()); // focus is on left
        s.pointer_up("left", point(px(50.), px(60.)), false);
        assert!(s.local_loupe("left").is_none()); // gesture over → none
    }

    #[test]
    fn edge_resize_and_moves_get_no_loupe() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.)));
        // top-EDGE midpoint grab: aims a line, not a pixel
        s.pointer_down("left", point(px(20.), px(10.)), false);
        s.pointer_move("left", point(px(20.), px(5.)), false);
        assert!(s.local_loupe("left").is_none());
        s.pointer_up("left", point(px(20.), px(5.)), false);
        // interior move: no precise point either
        s.pointer_down("left", point(px(20.), px(20.)), false);
        s.pointer_move("left", point(px(25.), px(25.)), false);
        assert!(s.local_loupe("left").is_none());
        s.pointer_up("left", point(px(25.), px(25.)), false);
    }

    #[test]
    fn loupe_tracks_a_shape_handle_drag() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        let local = |p: gpui_kit::Point<gpui_kit::Pixels>| p - point(px(-100.), px(20.));
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Line);
            a.begin(point(px(10.), px(10.)), sel, false);
            a.drag_to(point(px(60.), px(60.)), sel, false);
            a.end();
            a.toggle(ShapeKind::Select); // click-select needs the mode
        });
        let line = s.annotations().committed()[0].clone();
        // select, then drag the END handle without releasing
        let mid = point(
            (line.points[0].x + line.points[1].x) / 2.,
            (line.points[0].y + line.points[1].y) / 2.,
        );
        s.pointer_down("left", local(mid), false);
        s.pointer_up("left", local(mid), false);
        let target = point(px(80.), px(20.));
        s.pointer_down("left", local(line.points[1]), false);
        s.pointer_move("left", local(target), false);
        // The handle now lives on the RIGHT output (target is global
        // (80,20)): the loupe routes to the output that OWNS the focus,
        // even though the drag events keep coming from the left window
        // (implicit grab).
        let loupe = s.local_loupe("right").expect("handle drag → loupe");
        assert_eq!(loupe.focus, point(px(80.), px(20.))); // global − right origin
        assert!(s.local_loupe("left").is_none());
        assert_eq!(loupe.outward, (1., 1.)); // endpoint sits BR of center
        s.pointer_up("left", local(target), false);
        assert!(s.local_loupe("right").is_none());
    }

    #[test]
    fn press_outside_selection_still_restarts_and_clears_annotations() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.)));
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));
        s.pointer_down("left", point(px(15.), px(15.)), false);
        s.pointer_up("left", point(px(25.), px(25.)), false);
        assert_eq!(s.annotations().visible().count(), 1);
        // untoggle the tool, then press well outside the box: fresh
        // selection, annotations wiped
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));
        s.pointer_down("left", point(px(60.), px(60.)), false);
        assert!(s.selection().is_dragging());
        s.pointer_move("left", point(px(80.), px(80.)), false);
        s.pointer_up("left", point(px(80.), px(80.)), false);
        assert_eq!(s.annotations().visible().count(), 0);
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(-40.), px(80.)));
    }

    #[test]
    fn esc_reverts_an_inflight_move() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(30.), px(30.)));
        s.pointer_down("left", point(px(20.), px(20.)), false);
        s.pointer_move("left", point(px(60.), px(60.)), false);
        s.cancel_drag();
        assert!(s.selection().is_selected());
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(-90.), px(30.)));
        assert_eq!(b.size, size(px(20.), px(20.)));
    }

    #[test]
    fn cross_screen_move_translates_globally() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(20.), px(60.))); // global (-20,40) 40×20, spans the seam
        // grab the part that lives on the RIGHT screen…
        s.pointer_down("right", point(px(10.), px(50.)), false); // global (10,50): interior
        // …and the drag continues with the LEFT overlay delivering events
        // (left origin is (-100,20): local (90,40) → global (-10,60))
        s.pointer_move("left", point(px(90.), px(40.)), false);
        s.pointer_up("left", point(px(90.), px(40.)), false);
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(-40.), px(50.)));
        assert_eq!(b.size, size(px(40.), px(20.)));
        assert_eq!(
            s.local_bounds("left").unwrap(),
            Bounds {
                origin: point(px(60.), px(30.)),
                size: size(px(40.), px(20.))
            }
        );
        assert!(s.local_bounds("right").is_none()); // fully on the left now
    }

    #[test]
    fn in_place_click_inside_selection_keeps_it_even_over_a_snap_window() {
        let mut s = snapped_session();
        s.begin("right", point(px(25.), px(35.)));
        s.end("right", point(px(55.), px(75.)));
        // a click (no drag) at a point that ALSO sits on a snap window:
        // editing semantics win — the current selection is kept, no re-snap
        s.pointer_down("right", point(px(40.), px(55.)), false);
        s.pointer_up("right", point(px(40.), px(55.)), false);
        assert!(s.selection().is_selected());
        let b = s.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(25.), px(35.)));
        assert_eq!(b.size, size(px(30.), px(40.)));
    }

    #[test]
    fn editing_suppresses_the_window_hover_outline() {
        let mut s = snapped_session();
        s.begin("right", point(px(25.), px(35.)));
        s.end("right", point(px(55.), px(75.)));
        s.pointer_down("right", point(px(40.), px(55.)), false); // move grab
        assert!(!s.hover_at("right", point(px(30.), px(40.)))); // over a window, but editing
        assert!(s.hover_bounds("right").is_none());
        s.pointer_up("right", point(px(40.), px(55.)), false);
        // released, but the selection still exists: the hover phase is
        // over (no candidate hunting under a finalized selection)
        assert!(!s.hover_at("right", point(px(30.), px(40.))));
        assert!(s.hover_bounds("right").is_none());
    }

    // ── Toolbar dragging ───────────────────────────────────────────

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
        s.pointer_down("right", point(px(120.), px(100.)), false); // interior
        s.pointer_move("right", point(px(220.), px(200.)), false);
        s.pointer_up("right", point(px(220.), px(200.)), false);
        assert_eq!(s.toolbar_bounds("right").unwrap().origin, dragged);

        // a NEW selection re-anchors
        s.pointer_down("right", point(px(500.), px(500.)), false);
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

    #[test]
    fn rectangle_crosses_mixed_dpi_outputs_and_is_encoded_but_not_used_for_ocr() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(20.), px(60.)));
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Rectangle));
        s.pointer_down("left", point(px(90.), px(25.)), false);
        s.pointer_up("right", point(px(10.), px(55.)), false);
        let (w, h, pixels) = s.crop("right").unwrap();
        let png = crate::model::export::encode_png(w, h, &pixels).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
        let color = s.annotations().color().0.to_be_bytes();
        assert_eq!(decoded.get_pixel(20, 10).0, color);
        assert_eq!(decoded.get_pixel(40, 10).0, color);
        assert_eq!(decoded.get_pixel(40, 20).0, [0, 255, 0, 255]);
        let (_, _, original) = s.crop_original("left").unwrap();
        assert_eq!(
            &original[(10 * w as usize + 40) * 4..(10 * w as usize + 40) * 4 + 4],
            &[0, 255, 0, 255]
        );
    }
    #[test]
    fn ellipse_crosses_mixed_dpi_outputs_with_an_unmarked_center_and_ocr_source() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(20.), px(60.)));
        s.edit_annotations(|a| {
            a.toggle(crate::annotation::ShapeKind::Ellipse);
            a.set_color(4);
        });
        s.pointer_down("left", point(px(90.), px(25.)), false);
        s.pointer_up("right", point(px(10.), px(55.)), false);
        let left = s.local_annotations("left")[0].clone();
        let right = s.local_annotations("right")[0].clone();
        assert_eq!(left.bounds.origin, point(px(90.), px(25.)));
        assert_eq!(right.bounds.origin, point(px(-10.), px(45.)));
        let (w, h, pixels) = s.crop("right").unwrap();
        let png = crate::model::export::encode_png(w, h, &pixels).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
        let color = s.annotations().color().0.to_be_bytes();
        for (x, y) in [(21, 20), (58, 20), (40, 11), (40, 28)] {
            assert_eq!(decoded.get_pixel(x, y).0, color);
        }
        assert_eq!(decoded.get_pixel(40, 20).0, [0, 255, 0, 255]);
        assert_eq!(decoded.get_pixel(20, 10).0, [255, 0, 0, 255]);
        let (_, _, original) = s.crop_original("left").unwrap();
        assert_eq!(
            &original[(11 * w as usize + 40) * 4..(11 * w as usize + 40) * 4 + 4],
            &[0, 255, 0, 255]
        );
    }
    #[test]
    fn line_and_polyline_cross_outputs_and_export_without_floating_preview() {
        for kind in [
            crate::annotation::ShapeKind::Line,
            crate::annotation::ShapeKind::Arrow,
            crate::annotation::ShapeKind::Polyline,
            crate::annotation::ShapeKind::Pencil,
        ] {
            let mut s = session();
            s.begin("left", point(px(80.), px(20.)));
            s.end("right", point(px(20.), px(60.)));
            s.edit_annotations(|a| {
                a.toggle(kind);
                a.set_color(4);
            });
            s.pointer_down("left", point(px(90.), px(30.)), false);
            if kind == crate::annotation::ShapeKind::Polyline {
                s.pointer_up("left", point(px(90.), px(30.)), false);
                s.pointer_down("right", point(px(10.), px(50.)), false);
            }
            s.pointer_up("right", point(px(10.), px(50.)), false);
            s.pointer_move("right", point(px(10.), px(58.)), false);
            s.edit_annotations(|a| a.finish_polyline());
            let left = s.local_annotations("left");
            let right = s.local_annotations("right");
            assert_eq!(left[0].points[0], point(px(90.), px(30.)));
            assert_eq!(right[0].points[0], point(px(-10.), px(50.)));
            let (w, h, pixels) = s.crop("left").unwrap();
            let png = crate::model::export::encode_png(w, h, &pixels).unwrap();
            let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
            let color = s.annotations().color().0.to_be_bytes();
            assert_eq!(decoded.get_pixel(25, 20).0, color);
            assert_eq!(decoded.get_pixel(55, 20).0, color);
            assert_eq!(decoded.get_pixel(60, 35).0, [0, 255, 0, 255]);
            let (_, _, original) = s.crop_original("right").unwrap();
            assert_eq!(
                &original[(20 * w as usize + 55) * 4..(20 * w as usize + 55) * 4 + 4],
                &[0, 255, 0, 255]
            );
        }
    }
    #[test]
    fn text_crosses_outputs_with_shared_preview_and_history() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(40.), px(90.)));
        let bounds = s.text_bounds("left", point(px(85.), px(25.)), 24.).unwrap();
        s.edit_annotations(|a| {
            a.set_tool_size(16.);
            a.set_color(4);
            a.add_text(bounds, "MMMM 中文".into());
        });
        let original = s.crop_original("left").unwrap().2;
        let (w, _, pixels) = s.crop("left").unwrap();
        let mut sides = [false; 2];
        for (i, (before, after)) in original
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks::<4>().0.iter())
            .enumerate()
        {
            if before != after {
                sides[usize::from(i % w as usize >= 40)] = true;
            }
        }
        assert_eq!(sides, [true, true]);
        let (_, left) = s.filtered_preview("left").unwrap();
        let (_, right) = s.filtered_preview("right").unwrap();
        assert!(Arc::ptr_eq(&left, &right));
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.crop("left").unwrap().2, original);
        s.edit_annotations(|a| a.redo());
        assert_eq!(s.crop("right").unwrap().2, pixels);
    }

    fn assert_preview_matches_export(s: &ScreenshotSession) {
        // While a freehand draft is live the preview intentionally lacks
        // its ink: the stroke rides as a vector overlay (Overlay::render)
        // and only committed marks are baked into the image.
        let draft_live = s
            .annotations()
            .draft_shape()
            .is_some_and(crate::annotation::Shape::is_freehand);
        let pixels = if draft_live {
            s.crop_without_freehand_draft("left").unwrap().2
        } else {
            s.crop("left").unwrap().2
        };
        match s.filtered_preview("left") {
            Some((_, preview)) => {
                let expected: Vec<_> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[2], p[1], p[0], p[3]])
                    .collect();
                assert_eq!(preview.as_bytes(0).unwrap(), expected);
                let (_, other) = s.filtered_preview("right").unwrap();
                assert!(Arc::ptr_eq(&preview, &other));
            }
            // No raster shapes left (the object eraser may delete them
            // all): the vector path renders instead, on every output.
            None => assert!(s.filtered_preview("right").is_none()),
        }
    }

    #[test]
    fn clear_all_annotations_keep_selection_and_frozen_capture() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        let pristine = s.crop("left").unwrap().2;
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Rectangle);
            a.begin(point(px(0.), px(10.)), sel, false);
            a.drag_to(point(px(40.), px(50.)), sel, false);
            a.end();
            // a raster-preview kind (mosaic) flips the session onto the
            // filtered path, so the wipe must invalidate that cache too
            a.toggle(ShapeKind::Mosaic);
            a.begin(point(px(50.), px(10.)), sel, false);
            a.drag_to(point(px(80.), px(50.)), sel, false);
            a.end();
        });
        let marked = s.crop("left").unwrap().2;
        assert_ne!(marked, pristine);
        let _ = s.filtered_preview("left");

        let mut cleared = false;
        s.edit_annotations(|a| cleared = a.clear_all());
        assert!(cleared);
        // annotations only: the selection region and the frozen capture
        // are exactly what they were before any mark existed
        assert_eq!(s.selection.bounds(), Some(sel));
        assert_eq!(s.crop("left").unwrap().2, pristine);
        // the raster path itself switches off — no filter kinds remain,
        // so the preview cache is dropped instead of being reused
        assert!(s.filtered_preview("left").is_none());
        assert!(s.filter_preview.borrow().is_none());

        // one undo restores every mark, still without touching the region
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.crop("left").unwrap().2, marked);
        assert_eq!(s.selection.bounds(), Some(sel));
    }

    #[test]
    fn click_selects_shapes_and_drag_moves_the_hit_shape() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Rectangle);
            a.begin(point(px(0.), px(10.)), sel, false); // global (0,10) → (40,50)
            a.drag_to(point(px(40.), px(50.)), sel, false);
            a.end();
            // selection is the select tool's job now — switch to it
            a.toggle(ShapeKind::Select);
        });

        // single click on the left edge band selects
        s.pointer_down("left", point(px(101.), px(5.)), false); // local → global (1,25)
        s.pointer_up("left", point(px(101.), px(5.)), false);
        assert!(s.annotations().selected().is_some());

        // click on blank canvas deselects — blank INSIDE the selection:
        // with the select tool a blank press is region editing, and a
        // blank press OUTSIDE would start a fresh selection (wiping
        // the annotations) before this click-selects anything again
        s.pointer_down("left", point(px(150.), px(60.)), false); // global (50,80)
        s.pointer_up("left", point(px(150.), px(60.)), false);
        assert!(s.annotations().selected().is_none());

        // Dragging a hit shape selects and moves it even if it was deselected.
        let before = s.annotations().committed()[0].clone();
        s.pointer_down("left", point(px(101.), px(5.)), false);
        s.pointer_move("left", point(px(160.), px(60.)), false);
        assert!(s.annotations().draft_shape().is_none());
        s.pointer_up("left", point(px(160.), px(60.)), false);
        assert!(s.annotations().selected().is_some());
        assert_eq!(s.annotations().committed().len(), 1);
        assert_ne!(s.annotations().committed()[0].bounds, before.bounds);
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.annotations().committed()[0], before);

        // a press that only wiggles within the slop still click-selects
        s.pointer_down("left", point(px(101.), px(5.)), false);
        s.pointer_move("left", point(px(103.), px(7.)), false); // < CLICK_SLOP
        s.pointer_up("left", point(px(103.), px(7.)), false);
        assert!(s.annotations().selected().is_some());
        assert_eq!(s.annotations().committed().len(), 1);
    }

    #[test]
    fn draw_tools_ink_over_shapes_instead_of_selecting() {
        // The 2026-10-06 flip: a draw tool's press ALWAYS draws —
        // parking a click-select on existing ink would both eat the
        // stroke and surprise-select. Selection is the select tool's
        // exclusive job.
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Rectangle);
            a.begin(point(px(0.), px(10.)), sel, false); // → (40,50)
            a.drag_to(point(px(40.), px(50.)), sel, false);
            a.end();
            a.toggle(ShapeKind::Pencil); // stays active for the next press
        });
        // press ON the placed rectangle's edge and drag: a new stroke
        // is inked over it, nothing gets selected
        s.pointer_down("left", point(px(101.), px(5.)), false); // global (1,25)
        s.pointer_move("left", point(px(130.), px(40.)), false);
        s.pointer_up("left", point(px(130.), px(40.)), false);
        assert_eq!(s.annotations().committed().len(), 2);
        assert!(s.annotations().selected().is_none());
    }

    #[test]
    fn select_tool_blank_press_edits_the_region_like_no_tool() {
        // The select tool has no gesture of its own: a blank-canvas
        // press falls through to region editing exactly like having
        // no tool at all (interior press = move the region).
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(60.), px(60.))); // global (-90,30)-(-40,80)
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Select));
        assert!(!s.annotations().enabled());
        s.pointer_down("left", point(px(30.), px(30.)), false); // global (-70,50): interior
        s.pointer_move("left", point(px(40.), px(35.)), false); // delta (10,5)
        s.pointer_up("left", point(px(40.), px(35.)), false);
        let b = s.selection.bounds().unwrap();
        assert_eq!(b.origin, point(px(-80.), px(35.)));
    }

    #[test]
    fn dragging_the_selection_moves_it_and_undo_restores() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        let rect = Bounds::new(point(px(0.), px(10.)), size(px(40.), px(40.)));
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Rectangle);
            a.begin(rect.origin, sel, false);
            a.drag_to(rect.bottom_right(), sel, false);
            a.end();
            a.toggle(ShapeKind::Select); // click-select needs the mode
        });
        // global → "left"-window local: left's origin is (-100, 20)
        let local = |p: gpui_kit::Point<gpui_kit::Pixels>| p - point(px(-100.), px(20.));
        // a point on the rectangle's left edge band (width 3)
        let edge = |b: Bounds<gpui_kit::Pixels>| point(b.left() + px(2.), b.top() + px(15.));

        // click-select
        s.pointer_down("left", local(edge(rect)), false);
        s.pointer_up("left", local(edge(rect)), false);
        assert!(s.annotations().selected().is_some());

        // press again and DRAG: the selected shape moves
        let delta = point(px(30.), px(10.));
        let moved_origin = rect.origin + delta;
        s.pointer_down("left", local(edge(rect)), false);
        s.pointer_move("left", local(edge(rect) + delta), false);
        s.pointer_up("left", local(edge(rect) + delta), false);
        let now = s.annotations().selected().unwrap().bounds;
        assert_eq!(now.origin, moved_origin);
        assert_eq!(now.size, rect.size);

        // one Edit entry: undo restores the pre-move position
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.annotations().committed()[0].bounds.origin, rect.origin);
        s.edit_annotations(|a| a.redo());
        assert_eq!(s.annotations().committed()[0].bounds.origin, moved_origin);

        // Escape mid-move restores the press-time snapshot. Undo/redo
        // dropped the selection — click to reselect first.
        s.pointer_down("left", local(edge(rect) + delta), false);
        s.pointer_up("left", local(edge(rect) + delta), false);
        assert!(s.annotations().selected().is_some());
        s.pointer_down("left", local(edge(rect) + delta), false);
        s.pointer_move(
            "left",
            local(edge(rect) + delta + point(px(50.), px(0.))),
            false,
        );
        assert!(s.is_body_moving());
        s.cancel_annotation();
        assert!(!s.is_body_moving());
        assert_eq!(s.annotations().committed()[0].bounds.origin, moved_origin);
    }

    #[test]
    fn handle_drags_edit_geometry_and_undo_restores() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        let local = |p: gpui_kit::Point<gpui_kit::Pixels>| p - point(px(-100.), px(20.));

        // a line; endpoints may be snapped — read them back
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Line);
            a.begin(point(px(10.), px(10.)), sel, false);
            a.drag_to(point(px(60.), px(60.)), sel, false);
            a.end();
            a.toggle(ShapeKind::Select); // click-select needs the mode
        });
        let line = s.annotations().committed()[0].clone();
        // click-select at the midpoint, then grab the END handle and
        // drag it (grabbing the handle must not move the whole shape)
        let mid = point(
            (line.points[0].x + line.points[1].x) / 2.,
            (line.points[0].y + line.points[1].y) / 2.,
        );
        s.pointer_down("left", local(mid), false);
        s.pointer_up("left", local(mid), false);
        assert!(s.annotations().selected().is_some());

        let target = point(px(80.), px(20.));
        s.pointer_down("left", local(line.points[1]), false);
        s.pointer_move("left", local(target), false);
        s.pointer_up("left", local(target), false);
        let edited = s.annotations().committed()[0].clone();
        assert_eq!(edited.points[1], target);
        assert_eq!(edited.points[0], line.points[0]); // anchor moved only
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.annotations().committed()[0].points, line.points);

        // rectangle: grab the BR handle and drag THROUGH the fixed TL
        // corner — bounds re-normalize like drawing did
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Rectangle);
            a.begin(point(px(0.), px(10.)), sel, false);
            a.drag_to(point(px(40.), px(50.)), sel, false);
            a.end();
            a.toggle(ShapeKind::Select); // click-select needs the mode
        });
        s.pointer_down("left", local(point(px(2.), px(25.))), false);
        s.pointer_up("left", local(point(px(2.), px(25.))), false);
        let br = {
            let b = s.annotations().selected().unwrap().bounds;
            point(b.right() - px(2.), b.bottom() - px(2.))
        };
        let through = point(px(-20.), px(-10.));
        s.pointer_down("left", local(br), false);
        s.pointer_move("left", local(through), false);
        s.pointer_up("left", local(through), false);
        let after = s.annotations().selected().unwrap().bounds;
        assert_eq!(
            after,
            Bounds::from_corners(point(px(-20.), sel.top()), point(px(0.), px(10.)))
        );

        // Escape mid-handle-drag restores the press-time snapshot
        let br = point(after.right() - px(2.), after.bottom() - px(2.));
        s.pointer_down("left", local(br), false);
        s.pointer_move("left", local(point(px(60.), px(70.))), false);
        assert!(s.handle_drag_anchor().is_some());
        s.cancel_annotation();
        assert!(s.handle_drag_anchor().is_none());
        assert_eq!(s.annotations().committed()[1].bounds, after);
    }

    #[test]
    fn dense_geometric_histories_use_composite_and_survive_undo() {
        use crate::annotation::ShapeKind;
        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Number,
        ] {
            let mut s = session();
            s.select_all();
            s.edit_annotations(|a| a.toggle(kind));
            let sel = s.selection.bounds().unwrap();
            for i in 0..65 {
                // Drive the annotation API directly — this test is about
                // the preview/export pipeline, and pointer_down's
                // hit-priority selection (issue #5) would swallow presses
                // landing on earlier strokes
                s.edit_annotations(|a| {
                    a.begin(point(px((i % 40) as f32 - 90.), px(50.)), sel, false);
                    a.drag_to(point(px(25.), px(60.)), sel, false);
                    a.end();
                });
                if i == 62 {
                    assert!(s.filtered_preview("left").is_none());
                }
                if i >= 63 {
                    assert_preview_matches_export(&s);
                }
            }
            s.edit_annotations(|a| a.undo());
            assert_preview_matches_export(&s);
            s.edit_annotations(|a| a.undo());
            assert!(s.filtered_preview("left").is_none());
            s.edit_annotations(|a| a.redo());
            assert_preview_matches_export(&s);
        }
    }

    #[test]
    fn incremental_preview_preserves_filter_order_erasure_and_history() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        // Mixed DPI, negative desktop coordinates and transparent gaps.
        let sel = s.selection.bounds().unwrap();
        for kind in [
            ShapeKind::Blur,
            ShapeKind::Rectangle,
            ShapeKind::Mosaic,
            ShapeKind::Eraser,
            ShapeKind::Pencil,
            ShapeKind::EraserRect,
            ShapeKind::Highlighter,
        ] {
            s.edit_annotations(|a| a.toggle(kind));
            // begin past the pointer layer so the gesture starts ON the
            // earlier shapes: drawing tools would park a click-select
            // there, the eraser instead DELETES what the sweep touches
            // (mid-gesture the preview flips as raster shapes vanish)
            s.edit_annotations(|a| a.begin(point(px(-30.), px(45.)), sel, false));
            for x in [5., 15., 30.] {
                s.pointer_move("right", point(px(x), px(60.)), false);
                assert_preview_matches_export(&s);
            }
            s.pointer_up("right", point(px(35.), px(65.)), false);
            assert_preview_matches_export(&s);
        }
        let completed = s.crop("left").unwrap().2;
        s.edit_annotations(|a| a.begin(point(px(-25.), px(50.)), sel, false));
        s.pointer_move("right", point(px(45.), px(70.)), false);
        assert_preview_matches_export(&s);
        s.cancel_annotation();
        assert_preview_matches_export(&s);
        assert_eq!(s.crop("left").unwrap().2, completed);
        for _ in 0..6 {
            s.edit_annotations(|a| a.undo());
            assert_preview_matches_export(&s);
        }
        for _ in 0..6 {
            s.edit_annotations(|a| a.redo());
            assert_preview_matches_export(&s);
        }
        assert_eq!(s.crop("left").unwrap().2, completed);
        // Changing display geometry invalidates even an unchanged history.
        s.set_size("left", size(px(80.), px(80.)));
        assert_preview_matches_export(&s);
    }

    #[test]
    #[ignore = "manual long-stroke performance measurement"]
    fn benchmark_long_pencil_preview() {
        use crate::annotation::ShapeKind;
        let mut cap = Capture::for_test((0, 0), 1.);
        cap.output_name = "left".into();
        cap.width = 1280;
        cap.height = 720;
        cap.rgba = [255; 4].repeat(1280 * 720);
        let mut s = ScreenshotSession::new(vec![Arc::new(cap)], Vec::new());
        s.select_all();
        s.edit_annotations(|a| a.toggle(ShapeKind::Pencil));
        s.pointer_down("left", point(px(10.), px(10.)), false);
        for i in 1..=20000 {
            s.pointer_move(
                "left",
                point(
                    px(10. + (i % 1200) as f32),
                    px(10. + ((i / 1200) * 35) as f32),
                ),
                false,
            );
        }
        let started = std::time::Instant::now();
        let (_, first) = s.filtered_preview("left").unwrap();
        let elapsed = started.elapsed();
        let expected: Vec<_> = s
            .crop_without_freehand_draft("left")
            .unwrap()
            .2
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        assert_eq!(first.as_bytes(0).unwrap(), expected);
        let mut full = std::time::Duration::ZERO;
        let mut incremental = std::time::Duration::ZERO;
        for i in 0..30 {
            s.pointer_move(
                "left",
                point(px(1000. + i as f32), px(650. + (i % 2) as f32)),
                false,
            );
            let start = std::time::Instant::now();
            let raster = s.crop_without_freehand_draft("left").unwrap();
            let expected =
                crate::ui::image_util::rgba_to_render_image(raster.2, raster.0, raster.1);
            full += start.elapsed();
            let start = std::time::Instant::now();
            let (_, image) = s.filtered_preview("left").unwrap();
            incremental += start.elapsed();
            assert_eq!(image.as_bytes(0).unwrap(), expected.as_bytes(0).unwrap());
        }
        eprintln!(
            "30 extensions of a 20,000-point stroke: full {full:?}, incremental {incremental:?}"
        );
        s.pointer_up("left", point(px(1000.), px(650.)), false);
        s.filtered_preview("left").unwrap();
        let start = std::time::Instant::now();
        for i in 0..30 {
            s.pointer_down("left", point(px(10.), px(10.)), false);
            s.pointer_move("left", point(px(50. + i as f32), px(50.)), false);
            s.filtered_preview("left").unwrap();
            s.pointer_up("left", point(px(50. + i as f32), px(50.)), false);
            s.filtered_preview("left").unwrap();
        }
        eprintln!(
            "20,000-point pencil preview: {elapsed:?}; 30 later strokes: {:?}",
            start.elapsed()
        );
    }

    #[test]
    #[ignore = "manual performance measurement; no timing assertion"]
    fn benchmark_drawing_after_committed_blurs() {
        use crate::annotation::ShapeKind;
        let mut cap = Capture::for_test((0, 0), 1.);
        cap.output_name = "left".into();
        cap.width = 1280;
        cap.height = 720;
        cap.rgba = [60, 90, 120, 255].repeat(1280 * 720);
        let mut s = ScreenshotSession::new(vec![Arc::new(cap)], Vec::new());
        s.select_all();
        s.edit_annotations(|a| a.toggle(ShapeKind::Blur));
        for _ in 0..6 {
            s.pointer_down("left", point(px(0.), px(0.)), false);
            s.pointer_up("left", point(px(1280.), px(720.)), false);
        }
        s.filtered_preview("left").unwrap();
        s.edit_annotations(|a| a.toggle(ShapeKind::Pencil));
        s.pointer_down("left", point(px(100.), px(100.)), false);
        let mut full = std::time::Duration::ZERO;
        let mut cached = std::time::Duration::ZERO;
        for i in 1..=30 {
            s.pointer_move("left", point(px(100. + i as f32 * 8.), px(110.)), false);
            let start = std::time::Instant::now();
            let raster = s.crop_without_freehand_draft("left").unwrap();
            let expected =
                crate::ui::image_util::rgba_to_render_image(raster.2, raster.0, raster.1);
            full += start.elapsed();
            let start = std::time::Instant::now();
            let (_, preview) = s.filtered_preview("left").unwrap();
            cached += start.elapsed();
            assert_eq!(preview.as_bytes(0).unwrap(), expected.as_bytes(0).unwrap());
        }
        eprintln!(
            "30 updates at 1280x720 after six blurs: full replay {full:?}, cached {cached:?}"
        );
    }

    #[test]
    fn eraser_chrome_rings_only_on_the_pointed_output_and_rect_spans() {
        use crate::annotation::ShapeKind;
        let mut s = session();
        s.select_all();
        let sel = s.selection.bounds().unwrap();
        s.set_size("left", size(px(100.), px(60.)));
        s.set_size("right", size(px(96.), px(60.)));
        s.edit_annotations(|a| a.toggle(ShapeKind::Eraser));
        // idle pointer over the LEFT output: only its window rings —
        // every other output's unclamped mapping falls outside its
        // own window and must stay silent
        s.pointer_move("left", point(px(50.), px(30.)), false);
        assert!(matches!(
            s.eraser_chrome("left"),
            Some(super::EraserChrome::Ring { .. })
        ));
        assert_eq!(s.eraser_chrome("right"), None);
        // the area gesture is global: each output shows its slice
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::EraserRect);
            a.begin(point(px(-40.), px(40.)), sel, false);
            a.drag_to(point(px(30.), px(50.)), sel, false);
        });
        for name in ["left", "right"] {
            assert!(
                matches!(s.eraser_chrome(name), Some(super::EraserChrome::Rect(_))),
                "{name}"
            );
        }
        s.edit_annotations(|a| a.end());
        assert_eq!(s.eraser_chrome("left"), None);
    }

    #[test]
    fn eraser_deletes_whole_shapes_across_outputs_and_undo_restores() {
        use crate::annotation::ShapeKind;
        for kind in [ShapeKind::Eraser, ShapeKind::EraserRect] {
            let mut s = session();
            s.begin("left", point(px(80.), px(20.)));
            s.end("right", point(px(40.), px(90.)));
            s.edit_annotations(|a| a.toggle(ShapeKind::Rectangle));
            s.pointer_down("left", point(px(82.), px(25.)), false);
            s.pointer_up("right", point(px(38.), px(60.)), false);
            let marked = s.crop("left").unwrap().2;
            let pristine_left = s.crop_original("left").unwrap().2;
            let pristine_right = s.crop_original("right").unwrap().2;
            assert_ne!(marked, pristine_left);
            s.edit_annotations(|a| a.toggle(kind));
            // The stroke starts ON the rectangle's edge band: with the
            // object eraser that press must erase (issue #14), not
            // park a click-select like drawing tools do.
            assert!(!s.annotations().parks_click_select());
            let sel = s.selection.bounds().unwrap();
            s.edit_annotations(|a| {
                a.begin(point(px(-19.), px(42.)), sel, false);
                a.drag_to(point(px(39.), px(70.)), sel, false);
                a.end();
            });
            // Whole-shape deletion: the committed sequence is empty
            // and both sides of the seam show the frozen capture.
            assert_eq!(s.annotations().committed().len(), 0);
            assert_eq!(s.crop("left").unwrap().2, pristine_left);
            assert_eq!(s.crop("right").unwrap().2, pristine_right);
            // nothing raster remains — no preview cache at all
            assert!(s.filtered_preview("left").is_none());
            s.edit_annotations(|a| a.undo());
            assert_eq!(s.crop("left").unwrap().2, marked);
            s.edit_annotations(|a| a.redo());
            assert_eq!(s.crop("right").unwrap().2, pristine_right);
        }
    }

    #[test]
    fn filters_share_export_pixels_across_outputs_and_invalidate_on_undo() {
        for kind in [
            crate::annotation::ShapeKind::Mosaic,
            crate::annotation::ShapeKind::Blur,
        ] {
            let mut s = session();
            s.begin("left", point(px(80.), px(20.)));
            s.end("right", point(px(20.), px(60.)));
            let original = s.crop_original("left").unwrap().2;
            s.edit_annotations(|a| a.toggle(kind));
            s.pointer_down("right", point(px(18.), px(58.)), false);
            s.pointer_up("left", point(px(82.), px(22.)), false);
            let (w, h, pixels) = s.crop("left").unwrap();
            assert_ne!(pixels, original);
            let (left_bounds, left) = s.filtered_preview("left").unwrap();
            let (right_bounds, right) = s.filtered_preview("right").unwrap();
            assert!(Arc::ptr_eq(&left, &right));
            assert_eq!(
                left_bounds.origin - right_bounds.origin,
                point(px(100.), px(-20.))
            );
            let bgra: Vec<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect();
            assert_eq!(left.as_bytes(0).unwrap(), bgra);
            let encoded = crate::model::export::encode_png(w, h, &pixels).unwrap();
            assert_eq!(
                image::load_from_memory(&encoded)
                    .unwrap()
                    .into_rgba8()
                    .into_raw(),
                pixels
            );
            assert_eq!(s.crop_original("right").unwrap().2, original);
            s.edit_annotations(|a| a.undo());
            assert!(s.filtered_preview("left").is_none());
            assert_eq!(s.crop("left").unwrap().2, original);
            s.edit_annotations(|a| a.redo());
            assert_eq!(s.crop("left").unwrap().2, pixels);
            s.pointer_down("left", point(px(82.), px(22.)), false);
            s.pointer_move("right", point(px(5.), px(55.)), false);
            let (_, draft) = s.filtered_preview("left").unwrap();
            assert!(!Arc::ptr_eq(&left, &draft));
            s.cancel_annotation();
            assert_eq!(s.crop("left").unwrap().2, pixels);
        }
    }

    #[test]
    fn highlighter_crosses_mixed_dpi_outputs_and_leaves_ocr_unmarked() {
        let mut s = session();
        s.begin("left", point(px(80.), px(20.)));
        s.end("right", point(px(20.), px(60.)));
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Highlighter));
        s.pointer_down("left", point(px(90.), px(30.)), false);
        s.pointer_up("right", point(px(10.), px(50.)), false);
        assert_eq!(
            s.local_annotations("right")[0].points[0],
            point(px(-10.), px(50.))
        );
        let (w, h, marked) = s.crop("left").unwrap();
        let (_, _, original) = s.crop_original("right").unwrap();
        let png = crate::model::export::encode_png(w, h, &marked).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
        let color = s.annotations().color().0.to_be_bytes();
        for (x, y) in [(25, 20), (55, 20)] {
            let at = ((y * w + x) * 4) as usize;
            for channel in 0..3 {
                let expected = (original[at + channel] as f32 * (159. / 255.)
                    + color[channel] as f32 * (96. / 255.))
                    .round() as u8;
                assert_eq!(decoded.get_pixel(x, y)[channel], expected);
            }
            assert_ne!(&marked[at..at + 3], &original[at..at + 3]);
        }
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.crop("left").unwrap().2, original);
        s.edit_annotations(|a| a.redo());
        assert_eq!(s.crop("left").unwrap().2, marked);
    }

    #[test]
    fn sequence_numbers_are_global_across_screens_and_export_across_the_seam() {
        let mut s = session();
        s.begin("left", point(px(80.), px(0.)));
        s.end("right", point(px(20.), px(100.)));
        s.edit_annotations(|a| {
            a.toggle(crate::annotation::ShapeKind::Number);
            a.set_color(4);
        });
        s.pointer_down("left", point(px(98.), px(20.)), false);
        s.pointer_up("left", point(px(98.), px(20.)), false);
        s.pointer_down("right", point(px(2.), px(75.)), false);
        s.pointer_up("right", point(px(2.), px(75.)), false);
        assert_eq!(
            s.annotations()
                .visible()
                .map(|mark| mark.number.unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            s.local_annotations("left")[0].bounds.origin,
            point(px(82.), px(4.))
        );
        assert_eq!(
            s.local_annotations("right")[0].bounds.origin,
            point(px(-18.), px(24.))
        );
        let (w, h, pixels) = s.crop("right").unwrap();
        let png = crate::model::export::encode_png(w, h, &pixels).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
        let color = s.annotations().color().0.to_be_bytes();
        assert_eq!(decoded.get_pixel(10, 40).0, color);
        assert_eq!(decoded.get_pixel(56, 40).0, color);
        assert_ne!(s.crop_original("left").unwrap().2, pixels);
        s.edit_annotations(|a| a.undo());
        assert_eq!(s.annotations().next_number(), 2);
    }

    #[test]
    fn placed_number_badge_can_be_selected_dragged_and_recolored() {
        let mut s = session();
        s.begin("left", point(px(0.), px(0.)));
        s.end("left", point(px(100.), px(100.)));
        s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Number));
        s.pointer_down("left", point(px(50.), px(50.)), false);
        s.pointer_up("left", point(px(50.), px(50.)), false);
        assert_eq!(s.annotations().visible().count(), 1);
        let orig_pos = s.annotations().visible().next().unwrap().bounds.origin;

        s.edit_annotations(|a| {
            a.deselect();
            a.toggle(crate::annotation::ShapeKind::Number);
            a.toggle(crate::annotation::ShapeKind::Select);
        });
        assert!(!s.annotations().enabled()); // select is a mode, not a draw tool
        assert!(s.annotations().selected().is_none());

        s.pointer_down("left", point(px(50.), px(50.)), false);
        s.pointer_up("left", point(px(50.), px(50.)), false);
        assert_eq!(s.annotations().selected_index(), Some(0));

        s.edit_annotation_settings(|a| a.set_color(1));
        assert_eq!(
            s.annotations().selected().unwrap().color,
            crate::model::settings::current().annotation_colors()[1]
        );

        s.pointer_down("left", point(px(50.), px(50.)), false);
        s.pointer_move("left", point(px(60.), px(65.)), false);
        s.pointer_up("left", point(px(60.), px(65.)), false);
        let moved_pos = s.annotations().visible().next().unwrap().bounds.origin;
        assert_ne!(moved_pos, orig_pos);
        assert_eq!(s.annotations().visible().count(), 1);
    }

    #[test]
    fn moving_text_keeps_the_same_inset_on_all_four_edges() {
        let mut s = session();
        s.select_all();
        let selection = s.selection.bounds().unwrap();
        let bounds = Bounds::new(
            selection.origin + point(px(30.), px(30.)),
            size(px(40.), px(35.)),
        );
        s.edit_annotations(|a| {
            a.toggle(crate::annotation::ShapeKind::Text);
            a.add_text(bounds, "text".into());
            a.toggle(crate::annotation::ShapeKind::Select); // moving needs the mode
        });
        let origin = s.screen_origin("left");
        for target in [
            selection.origin - point(px(100.), px(100.)),
            selection.bottom_right() + point(px(100.), px(100.)),
        ] {
            let before = s.annotations().committed()[0].bounds;
            s.pointer_down("left", before.center() - origin, false);
            s.pointer_move("left", target - origin, false);
            s.pointer_up("left", target - origin, false);
            let after = s.annotations().committed()[0].bounds;
            assert_eq!(after.size, before.size);
            assert!(after.left() >= selection.left() + px(2.));
            assert!(after.top() >= selection.top() + px(2.));
            assert!(after.right() <= selection.right() - px(2.));
            assert!(after.bottom() <= selection.bottom() - px(2.));
            if target.x < selection.left() {
                assert_eq!(after.origin, selection.origin + point(px(2.), px(2.)));
            } else {
                assert_eq!(
                    after.bottom_right(),
                    selection.bottom_right() - point(px(2.), px(2.))
                );
            }
            s.edit_annotations(|a| a.undo());
            assert_eq!(s.annotations().committed()[0].bounds, before);
        }
    }

    #[test]
    fn text_bounds_stop_at_selection_edges_and_reject_insufficient_space() {
        let mut s = session();
        s.begin("left", point(px(10.), px(10.)));
        s.end("left", point(px(200.), px(200.)));
        let bounds = s.text_bounds("left", point(px(50.), px(50.)), 24.).unwrap();
        let selection = s.selection().bounds().unwrap();
        assert_eq!(
            bounds.bottom_right(),
            selection.bottom_right() - point(px(2.), px(2.))
        );
        let local_origin = selection.origin - s.screen_origin("left");
        let at_corner = s.text_bounds("left", local_origin, 24.).unwrap();
        assert_eq!(at_corner.origin, selection.origin + point(px(2.), px(2.)));
        let half_line =
            selection.bottom_right() - s.screen_origin("left") - point(px(50.), px(20.));
        assert!(s.text_bounds("left", half_line, 24.).is_none());
        assert!(s.text_bounds("left", half_line, 12.).is_some());
        let near_edge = selection.bottom_right() - s.screen_origin("left") - point(px(3.), px(3.));
        assert!(s.text_bounds("left", near_edge, 24.).is_none());
    }
}
