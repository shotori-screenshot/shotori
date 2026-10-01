//! Shared screenshot selection and captures for every overlay in one session.
use std::sync::Arc;

use gpui_kit::*;

use crate::{model::selection::Selection, platform::capture::Capture};

/// How far a press on an annotation shape may travel before it stops
/// being a click (select) and turns into a draw stroke instead.
const CLICK_SLOP: f32 = 4.;

// One inset for placement, editing and movement; the outline is painted inside
// the text bounds, so no separate border outset may consume this breathing room.
const TEXT_INSET: f32 = 2.;
fn text_area(selection: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let inset = point(px(TEXT_INSET), px(TEXT_INSET));
    (selection.size.width > px(TEXT_INSET * 2.) && selection.size.height > px(TEXT_INSET * 2.))
        .then(|| Bounds::from_corners(selection.origin + inset, selection.bottom_right() - inset))
}

struct Screen {
    capture: Arc<Capture>,
    logical_size: Size<Pixels>,
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
    /// Window-snap targets in global logical coordinates; empty when the
    /// compositor exposes no supported IPC — see [`crate::platform::windowsnap`]
    snaps: Vec<crate::platform::windowsnap::SnapRect>,
    /// Index into `snaps`: the window under the cursor (hover outline)
    hovered: Option<usize>,
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
            snaps,
            hovered: None,
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
        let (width, height) = crate::model::placement::toolbar_size(self.annotations.enabled());
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
    /// `ui::toolbar`. ROW ONE ONLY (the settings row does not grab), and
    /// inset by the bar's padding — this is the grip ELEMENT's exact
    /// rect, so the hand cursor and the drag trigger coincide
    /// pixel-for-pixel (a mismatch in either axis shows up instantly as
    /// "draggable but not a hand" or vice versa — user-reported).
    pub(crate) fn toolbar_grips(&self, name: &str) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
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
        let (base_w, height) = crate::model::placement::toolbar_size(self.annotations.enabled());
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
        // window sits under the PRESS point, select its rect instead
        // (window snapping). A real drag still wins: freehand beats snap.
        if !self.selection.is_dragging()
            && !self.selection.is_selected()
            && let Some(press) = self.press
            && let Some(hit) = crate::platform::windowsnap::hit_test(&self.snaps, press)
        {
            self.selection = Selection::Selected {
                bounds: self.snaps[hit].bounds,
            };
        }
        self.press = None;
        // A fresh drag can finish over the seam on another output — the
        // chrome host must follow the selection there, exactly like an
        // edit-release does (see `follow_selection_host`).
        self.follow_selection_host();
    }

    /// Track the window under the cursor for the hover outline. Returns
    /// whether the hover changed (callers decide on cx.notify()).
    /// Suppressed while dragging — the freehand region takes over — and
    /// while a modal is open.
    pub(crate) fn hover_at(&mut self, name: &str, local: Point<Pixels>) -> bool {
        if self.blocked
            || self.selection.is_dragging()
            || self.selection.is_editing()
            || self.toolbar_drag.is_some()
            || self.snaps.is_empty()
            || self.annotations.enabled()
        {
            return false;
        }
        let global = local + self.screen(name).bounds().origin;
        let hit = crate::platform::windowsnap::hit_test(&self.snaps, global);
        if hit == self.hovered {
            return false;
        }
        self.hovered = hit;
        true
    }

    /// The hovered window's rect in this output's local coordinates, for
    /// the hover outline; None when not hovering anything visible here.
    pub(crate) fn hover_bounds(&self, name: &str) -> Option<Bounds<Pixels>> {
        if self.blocked {
            return None;
        }
        let screen = self.screen(name).bounds();
        let mut b = self.snaps.get(self.hovered?)?.bounds.intersect(&screen);
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
        self.selection.cancel_drag();
    }

    pub(crate) fn annotations(&self) -> &crate::annotation::Annotations {
        &self.annotations
    }

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
    /// None for every other kind or place — the click-select flow
    /// itself is untouched.
    pub(crate) fn number_at_double_click(
        &self,
        name: &str,
        local: Point<Pixels>,
    ) -> Option<(usize, Point<Pixels>)> {
        if !self.annotations.enabled() || self.selection.bounds().is_none() {
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
            // following move/up events (polyline never parks — its
            // clicks place vertices)
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

    pub(crate) fn crop(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        self.crop_impl(output, true)
            .map(|r| (r.width, r.height, r.rgba))
    }
    /// Preserve the exact logical bounds of the rasterized crop for pin placement.
    pub(crate) fn crop_for_pin(&self, output: &str) -> Option<RasterSelection> {
        self.crop_impl(output, true)
    }

    /// Export without annotations — also the `full` subcommand's path.
    pub fn crop_original(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        self.crop_impl(output, false)
            .map(|r| (r.width, r.height, r.rgba))
    }

    /// One background render per session. Pointer updates remain in the model;
    /// when work finishes we snapshot only the newest state, never a frame queue.
    pub(crate) fn request_filtered_preview(
        &mut self,
        output: &str,
        cx: &mut Context<Self>,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        if !self.uses_raster_preview() {
            self.filter_preview.get_mut().take();
            self.preview_display = None;
            return None;
        }
        if !self.preview_busy {
            let unchanged = self.filter_preview.borrow().as_ref().is_some_and(|cache| {
                cache.selection == self.selection.bounds()
                    && cache.committed == self.annotations.committed()
                    && cache.draft.as_ref() == self.annotations.draft_shape()
                    && cache.draft_generation == self.annotations.draft_generation()
            });
            let max_scale = self
                .screens
                .iter()
                .map(|s| s.capture.width as f32 / f32::from(s.logical_size.width))
                .fold(1_f32, f32::max);
            let heavy = self.annotations.visible().any(|shape| {
                shape.points.len() > 1024
                    || (matches!(
                        shape.kind,
                        crate::annotation::ShapeKind::Blur | crate::annotation::ShapeKind::Mosaic
                    ) && f32::from(shape.bounds.size.width)
                        * f32::from(shape.bounds.size.height)
                        * max_scale
                        * max_scale
                        >= 262_144.)
            });
            if unchanged || !heavy {
                self.preview_display = None;
                return self.filtered_preview(output);
            }
            let cache = self.filter_preview.get_mut().take();
            self.preview_display = cache.as_ref().map(|cache| DisplayPreview {
                selection: cache.selection,
                committed: cache.committed.clone(),
                draft_kind: cache.draft.as_ref().map(|shape| shape.kind),
                draft_generation: cache.draft_generation,
                bounds: cache.original.bounds,
                image: cache.image.clone(),
            });
            let captures = self
                .screens
                .iter()
                .map(|screen| screen.capture.clone())
                .collect();
            let sizes: Vec<_> = self
                .screens
                .iter()
                .map(|screen| screen.logical_size)
                .collect();
            let selection = self.selection;
            let geometry_revision = self.geometry_revision;
            let annotations = self.annotations.render_snapshot();
            let output = output.to_owned();
            let worker_output = output.clone();
            self.preview_busy = true;
            let task = cx.background_spawn(async move {
                let mut snapshot = Self::new(captures, Vec::new());
                for (screen, size) in snapshot.screens.iter_mut().zip(sizes) {
                    screen.logical_size = size;
                }
                snapshot.selection = selection;
                snapshot.annotations = annotations;
                snapshot.filter_preview = std::cell::RefCell::new(cache);
                snapshot.filtered_preview(&worker_output);
                snapshot.filter_preview.into_inner()
            });
            cx.spawn(async move |this, cx| {
                let cache = task.await;
                let _ = this.update(cx, |session, cx| {
                    session.preview_busy = false;
                    // Geometry changes cannot reuse the worker's frozen crop.
                    if session.geometry_revision == geometry_revision
                        && session.selection.bounds() == selection.bounds()
                    {
                        *session.filter_preview.get_mut() = cache;
                    }
                    session.request_filtered_preview(&output, cx);
                    cx.notify();
                });
            })
            .detach();
        }
        let display = self.preview_display.as_ref()?;
        // A lagging draft may be displayed while it is being extended, but never
        // resurrect a cancelled stroke or undone history while the worker drains.
        let committed = self.annotations.committed();
        let same_history = display.committed == committed;
        let same_gesture = display.draft_generation == self.annotations.draft_generation();
        let just_finished = same_gesture
            && self.annotations.draft_shape().is_none()
            && committed.len() == display.committed.len() + 1
            && committed.starts_with(&display.committed)
            && committed.last().map(|shape| shape.kind) == display.draft_kind;
        let valid_draft = display.draft_kind.is_none()
            || (same_gesture
                && display.draft_kind == self.annotations.draft_shape().map(|shape| shape.kind));
        if display.selection != self.selection.bounds()
            || !(just_finished || (same_history && valid_draft))
        {
            return None;
        }
        let mut bounds = display.bounds;
        bounds.origin -= self.screen(output).bounds().origin;
        Some((bounds, display.image.clone()))
    }

    pub(crate) fn uses_raster_preview(&self) -> bool {
        use crate::annotation::ShapeKind;
        self.annotations.committed().len() >= 64
            || self.annotations.visible().any(|shape| {
                matches!(
                    shape.kind,
                    ShapeKind::Pencil
                        | ShapeKind::Highlighter
                        | ShapeKind::Polyline
                        | ShapeKind::Mosaic
                        | ShapeKind::Blur
                        | ShapeKind::Text
                )
            })
    }

    /// Reuse the exported composite for freehand strokes, pixel filters and text.
    /// Captures are immutable; selection, shapes and display geometry own invalidation.
    pub(crate) fn filtered_preview(
        &self,
        output: &str,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        use crate::annotation::ShapeKind;
        let mut cache = self.filter_preview.borrow_mut();
        if !self.uses_raster_preview() {
            *cache = None;
            return None;
        }
        let selection = self.selection.bounds();
        let committed = self.annotations.committed();
        let draft = self.annotations.draft_shape();
        if cache.as_ref().is_none_or(|c| c.selection != selection) {
            let original = self.crop_impl(output, false)?;
            let pixels = original.rgba.clone();
            let image = crate::ui::image_util::rgba_to_render_image(
                pixels.clone(),
                original.width,
                original.height,
            );
            *cache = Some(FilterPreview {
                selection,
                committed: Vec::new(),
                draft: None,
                draft_generation: self.annotations.draft_generation(),
                original,
                pixels,
                image,
                stroke: None,
            });
        }
        let cached = cache.as_mut()?;
        if cached.committed != committed
            || cached.draft.as_ref() != draft
            || cached.draft_generation != self.annotations.draft_generation()
        {
            let original = &cached.original;
            // Appending a finished stroke only replays the new suffix. Undo,
            // replacement and edits rebuild from the immutable capture.
            if cached.draft_generation != self.annotations.draft_generation() {
                cached.stroke = None;
            }
            let history_changed = cached.committed != committed;
            let appended_stroke = committed.len() == cached.committed.len() + 1
                && committed.starts_with(&cached.committed)
                && cached.stroke.is_some()
                && committed.last().is_some_and(|shape| {
                    matches!(shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter)
                });
            if appended_stroke {
                // Promote the final incremental draft, including a new release
                // point, instead of rasterizing the entire long stroke again.
                cached.pixels = cached
                    .stroke
                    .as_mut()
                    .unwrap()
                    .render(
                        committed.last().unwrap(),
                        &cached.pixels,
                        (original.width, original.height),
                        original.bounds.origin,
                        original.scale,
                    )
                    .to_vec();
            } else {
                if !committed.starts_with(&cached.committed) {
                    cached.pixels.clone_from(&original.rgba);
                    cached.committed.clear();
                }
                crate::annotation::Annotations::rasterize_shapes(
                    committed[cached.committed.len()..].iter(),
                    &mut cached.pixels,
                    original.width,
                    original.height,
                    original.bounds.origin,
                    original.scale,
                );
            }
            if history_changed {
                cached.stroke = None;
            }
            if cached.committed != committed {
                cached.committed = committed.to_vec();
            }
            let pixels = if let Some(shape) =
                draft.filter(|s| matches!(s.kind, ShapeKind::Pencil | ShapeKind::Highlighter))
            {
                cached
                    .stroke
                    .get_or_insert_with(Default::default)
                    .render(
                        shape,
                        &cached.pixels,
                        (original.width, original.height),
                        original.bounds.origin,
                        original.scale,
                    )
                    .to_vec()
            } else {
                cached.stroke = None;
                let mut pixels = cached.pixels.clone();
                crate::annotation::Annotations::rasterize_shapes(
                    draft.into_iter(),
                    &mut pixels,
                    original.width,
                    original.height,
                    original.bounds.origin,
                    original.scale,
                );
                pixels
            };
            cached.image = crate::ui::image_util::rgba_to_render_image(
                pixels,
                original.width,
                original.height,
            );
            cached.draft = draft.cloned();
            cached.draft_generation = self.annotations.draft_generation();
        }
        let mut bounds = cached.original.bounds;
        bounds.origin -= self.screen(output).bounds().origin;
        Some((bounds, cached.image.clone()))
    }

    /// Keep the original single-output crop when possible. Spanning selections
    /// use the highest participating pixel density; desktop gaps stay transparent.
    fn crop_impl(&self, fallback_output: &str, marked: bool) -> Option<RasterSelection> {
        // Export shortcut: the preview cache holds exactly this composite
        // (full-selection crop + committed annotations) whenever the
        // selection and history are unchanged since the last render.
        // Reusing it makes copy/save/pin/OCR O(memcpy) instead of a full
        // re-rasterization — decisive for long pencil strokes. The cache
        // only exists while `uses_raster_preview()` is true (it is taken
        // when that flips off), so no extra predicate is needed.
        if marked
            && let Some(cached) = self.filter_preview.borrow().as_ref()
            && let Some(selection) = self.selection.bounds()
            && cached.selection == Some(selection)
            && cached.draft.is_none()
            && cached.committed == self.annotations.committed()
            && cached.draft_generation == self.annotations.draft_generation()
        {
            return Some(RasterSelection {
                scale: cached.original.scale,
                width: cached.original.width,
                height: cached.original.height,
                rgba: cached.pixels.clone(),
                bounds: cached.original.bounds,
            });
        }
        let selected = self
            .selection
            .bounds()
            .unwrap_or_else(|| self.screen(fallback_output).bounds());
        let participating: Vec<_> = self
            .screens
            .iter()
            .filter_map(|screen| {
                let intersection = selected.intersect(&screen.bounds());
                (intersection.size.width > px(0.) && intersection.size.height > px(0.))
                    .then_some((screen, intersection))
            })
            .collect();
        if participating.len() == 1 {
            let (screen, mut bounds) = participating[0];
            bounds.origin -= screen.bounds().origin;
            let cap = &screen.capture;
            let scale = cap.width as f32 / f32::from(screen.logical_size.width);
            let (w, h, mut rgba) =
                crate::model::export::crop(&cap.rgba, cap.width, cap.height, bounds, scale)?;
            // crop() rounds the source offset to native pixels. Use the
            // same rounded origin when placing logical annotation edges.
            let origin = screen.bounds().origin
                + point(
                    px((f32::from(bounds.left()) * scale).round() / scale),
                    px((f32::from(bounds.top()) * scale).round() / scale),
                );
            if marked {
                self.annotations.rasterize(&mut rgba, w, h, origin, scale);
            }
            return Some(RasterSelection {
                scale,
                width: w,
                height: h,
                rgba,
                bounds: Bounds::new(origin, size(px(w as f32 / scale), px(h as f32 / scale))),
            });
        }
        if participating.is_empty() {
            return None;
        }
        let extent = participating
            .iter()
            .map(|(_, b)| *b)
            .reduce(|a, b| a.union(&b))?;
        let scale = participating
            .iter()
            .map(|(s, _)| s.capture.width as f32 / f32::from(s.logical_size.width))
            .fold(0., f32::max);
        let w = (f32::from(extent.size.width) * scale).round() as u32;
        let h = (f32::from(extent.size.height) * scale).round() as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let mut out = image::RgbaImage::new(w, h);
        for (screen, intersection) in participating {
            let mut local = intersection;
            local.origin -= screen.bounds().origin;
            let cap = &screen.capture;
            let (cw, ch, pixels) = crate::model::export::crop(
                &cap.rgba,
                cap.width,
                cap.height,
                local,
                cap.width as f32 / f32::from(screen.logical_size.width),
            )?;
            let image = image::RgbaImage::from_raw(cw, ch, pixels)?;
            let x = (f32::from(intersection.left() - extent.left()) * scale).round() as u32;
            let y = (f32::from(intersection.top() - extent.top()) * scale).round() as u32;
            let right = (f32::from(intersection.right() - extent.left()) * scale).round() as u32;
            let bottom = (f32::from(intersection.bottom() - extent.top()) * scale).round() as u32;
            let image = image::imageops::resize(
                &image,
                right - x,
                bottom - y,
                image::imageops::FilterType::Triangle,
            );
            image::imageops::replace(&mut out, &image, x.into(), y.into());
        }
        let mut rgba = out.into_raw();
        if marked {
            self.annotations
                .rasterize(&mut rgba, w, h, extent.origin, scale);
        }
        Some(RasterSelection {
            scale,
            width: w,
            height: h,
            rgba,
            bounds: Bounds::new(
                extent.origin,
                size(px(w as f32 / scale), px(h as f32 / scale)),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ScreenshotSession;
    use crate::model::selection::Selection;
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
        // released: hover tracking resumes
        assert!(s.hover_at("right", point(px(30.), px(40.))));
        assert!(s.hover_bounds("right").is_some());
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
        assert_eq!(anchored.size.width, px(crate::model::placement::TB_W_ROW1));
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
                px(1200. - crate::model::placement::TB_W_ROW1 - 8.),
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
        let pixels = s.crop("left").unwrap().2;
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
        });

        // single click on the left edge band selects
        s.pointer_down("left", point(px(101.), px(5.)), false); // local → global (1,25)
        s.pointer_up("left", point(px(101.), px(5.)), false);
        assert!(s.annotations().selected().is_some());

        // click on blank canvas deselects
        s.pointer_down("left", point(px(300.), px(300.)), false);
        s.pointer_up("left", point(px(300.), px(300.)), false);
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
        let expected = s.crop("left").unwrap().2;
        let expected: Vec<_> = expected
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
            let raster = s.crop_impl("left", true).unwrap();
            let expected = crate::ui::image_util::rgba_to_render_image(
                raster.rgba,
                raster.width,
                raster.height,
            );
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
            let raster = s.crop_impl("left", true).unwrap();
            let expected = crate::ui::image_util::rgba_to_render_image(
                raster.rgba,
                raster.width,
                raster.height,
            );
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
        });
        assert!(!s.annotations().enabled());
        assert!(s.annotations().selected().is_none());

        s.pointer_down("left", point(px(50.), px(50.)), false);
        s.pointer_up("left", point(px(50.), px(50.)), false);
        assert_eq!(s.annotations().selected_index(), Some(0));

        s.edit_annotation_settings(|a| a.set_color(1));
        assert_eq!(
            s.annotations().selected().unwrap().color,
            crate::ui::theme::c().annotation_colors[1]
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
