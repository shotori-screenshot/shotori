//! Pinned images share one ordered scene across all output surfaces.
//! Clicking a pin raises it without recreating any native windows.
//!
//! Submodules keep responsibilities crisp:
//! - [`ops`]: Copy, Save, OCR, QR, Clipboard paste, restore, transforms,
//!   opacity, ghost click-through, collapse, nudge, and pin cycling
//! - [`annotate`]: in-pin secondary annotation state & floating toolbar
//! - [`menu`]: flagship context menu & transient HUD pill
//! - [`render`]: Wayland input-region management & GPUI element tree
//! - [`transport`]: single-owner Unix-socket IPC (`SHTRPIN2`)

use std::sync::{Arc, OnceLock};

#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

pub use crate::actions::{ClosePin, CopyPin, DismissPinMenu, OpenPinMenu, ResetPinZoom};
use crate::annotation::Annotations;
use crate::model::pin::{
    CollapsedPin, apply_color_effects, center_square_crop, clamp_to, expand_bounds,
};
pub use crate::model::pin::{MIN_VISIBLE, clamp_origin, snap_pin_origin};
use crate::model::pin_history::PinHistoryStore;
use crate::ui::image_util;

/// Zoom range: from a postage stamp to deep-pixel-peeping.
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 24.;
/// Side length of the corner lock button kept interactive on a click-through pin.
pub(super) const GHOST_CHIP_SIDE: f32 = 18.;

/// Top-right unlock button bounds for a ghost (click-through) pin.
pub(super) fn ghost_chip_bounds(pin_rect: Bounds<Pixels>) -> Bounds<Pixels> {
    let side = px(GHOST_CHIP_SIDE)
        .min(pin_rect.size.width)
        .min(pin_rect.size.height);
    Bounds::new(
        point(
            (pin_rect.right() - side).max(pin_rect.left()),
            pin_rect.top(),
        ),
        size(side, side),
    )
}

/// One output the pin can live on: global logical bounds + the
/// display the layer surface binds to + true physical-to-logical scale
/// (`captured_width / logical_width`, immune to `window.scale_factor()`
/// cross-monitor misreports).
#[derive(Clone)]
struct PinOutput {
    bounds: Bounds<Pixels>,
    display_id: Option<DisplayId>,
    scale: f32,
}

/// The desktop's outputs, registered by `main` once displays resolve.
static OUTPUTS: OnceLock<std::sync::Mutex<Vec<PinOutput>>> = OnceLock::new();

/// Remember one output (bounds in GLOBAL logical px — the session's
/// coordinate space) so pins can span outputs and auxiliary windows
/// know each output's true physical scale.
pub fn register_output(bounds: Bounds<Pixels>, display_id: Option<DisplayId>, scale: f32) {
    OUTPUTS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(PinOutput {
            bounds,
            display_id,
            scale: scale.max(1.0),
        });
}

fn outputs() -> Vec<PinOutput> {
    OUTPUTS
        .get()
        .map(|o| {
            o.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
        .unwrap_or_default()
}

/// The display whose output rect contains `p` (global logical coords) —
/// how the scroll bar finds the right screen to live on without
/// reaching into the overlay's window internals.
pub(crate) fn display_containing(p: Point<Pixels>) -> Option<DisplayId> {
    outputs()
        .into_iter()
        .find(|o| o.bounds.contains(&p))
        .and_then(|o| o.display_id)
}

/// The first output that is NOT `display`: chrome that must leave its
/// capture output entirely docks there instead.
pub(crate) fn other_output_than(
    display: Option<DisplayId>,
) -> Option<(DisplayId, Bounds<Pixels>, f32)> {
    outputs().into_iter().find_map(|o| match o.display_id {
        Some(id) if Some(id) != display => Some((id, o.bounds, o.scale)),
        _ => None,
    })
}

/// Scroll lines → next zoom factor, clamped to `[ZOOM_MIN, ZOOM_MAX]`.
fn next_zoom_with(zoom: f32, lines: f32, step_pct: u8) -> f32 {
    let step = 1.0 + f32::from(step_pct.clamp(5, 50)) / 100.0;
    (zoom * step.powf(lines)).clamp(ZOOM_MIN, ZOOM_MAX)
}

fn next_zoom(zoom: f32, lines: f32) -> f32 {
    next_zoom_with(
        zoom,
        lines,
        crate::model::settings::current().pin().zoom_step,
    )
}

/// The on-screen size for a zoom factor.
fn zoomed_size(base: Size<Pixels>, zoom: f32) -> Size<Pixels> {
    size(
        px((f32::from(base.width) * zoom).max(1.)),
        px((f32::from(base.height) * zoom).max(1.)),
    )
}

/// Compatibility wrapper preserved for output-only edge-snap unit tests.
pub fn snap_origin_to_edges(
    origin: Point<Pixels>,
    pin: Size<Pixels>,
    outputs: &[Bounds<Pixels>],
) -> Point<Pixels> {
    snap_pin_origin(origin, pin, outputs, &[], true, false)
}

/// Active pointer drag of a pin across desktop-global coordinates.
#[derive(Clone, Copy, Debug)]
pub(super) struct PinDrag {
    pub id: u64,
    pub grab: Point<Pixels>,
    pub press_global: Point<Pixels>,
    pub moved: bool,
}

/// Snapshot kept in memory so `Ctrl+Shift+T` restores recently closed pins
/// immediately even when disk history is disabled (`max_history = 0`).
pub(super) struct ClosedPin {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub base: Size<Pixels>,
    pub rect: Bounds<Pixels>,
    pub opacity: u8,
    pub history_id: Option<String>,
    pub label: Option<String>,
}

/// One floating pin inside the shared [`PinBoard`] scene.
pub(super) struct Pin {
    pub id: u64,
    /// Upright base RGBA8 buffer `(w, h, rgba)` prior to live annotations & color filters.
    pub base_crop: Arc<(u32, u32, Vec<u8>)>,
    /// Secondary annotations in base-local logical space `(0, 0 .. base.width, base.height)`.
    pub annotations: Annotations,
    /// Cached composited RGBA8 `(w, h, rgba)` = `base_crop` + committed annotations + color filters.
    pub composited_crop: Arc<(u32, u32, Vec<u8>)>,
    pub image: Arc<RenderImage>,
    pub thumb_image: Arc<RenderImage>,
    pub base: Size<Pixels>,
    pub rect: Bounds<Pixels>,
    pub opacity: u8,
    pub grayscale: bool,
    pub invert: bool,
    pub ghost: bool,
    pub collapsed: Option<CollapsedPin>,
    pub hud: Option<(u64, String)>,
    pub history_id: Option<String>,
    pub label: Option<String>,
    pub dirty_raster: bool,
}

impl Pin {
    fn from_spec(
        id: u64,
        spec: PinSpec,
        base: Size<Pixels>,
        rect: Bounds<Pixels>,
        opacity: u8,
        history_id: Option<String>,
    ) -> Self {
        let PinSpec {
            w, h, rgba, label, ..
        } = spec;
        let base_crop = Arc::new((w, h, rgba.clone()));
        let (tw, th, thumb_rgba) = center_square_crop(w, h, &rgba);
        let image = image_util::rgba_to_render_image(rgba.clone(), w, h);
        let thumb_image = image_util::rgba_to_render_image(thumb_rgba, tw, th);
        let composited_crop = Arc::new((w, h, rgba));
        Self {
            id,
            base_crop,
            annotations: Annotations::from_settings(),
            composited_crop,
            image,
            thumb_image,
            base,
            rect,
            opacity: opacity.clamp(20, 100),
            grayscale: false,
            invert: false,
            ghost: false,
            collapsed: None,
            hud: None,
            history_id,
            label,
            dirty_raster: false,
        }
    }

    pub(super) fn physical_scale(&self) -> f32 {
        (self.base_crop.0 as f32 / f32::from(self.base.width).max(1.)).max(0.1)
    }

    /// Map a desktop-global logical point onto this pin's base-local logical space.
    pub(super) fn global_to_base(&self, global: Point<Pixels>) -> Point<Pixels> {
        let zoom = (f32::from(self.rect.size.width) / f32::from(self.base.width).max(1.)).max(0.01);
        let bx = clamp_to(
            f32::from(global.x - self.rect.origin.x) / zoom,
            0.,
            f32::from(self.base.width),
        );
        let by = clamp_to(
            f32::from(global.y - self.rect.origin.y) / zoom,
            0.,
            f32::from(self.base.height),
        );
        point(px(bx), px(by))
    }

    /// Bake committed annotations permanently onto `base_crop` and reset `annotations`
    /// (used right before a 90° rotation or mirror flip).
    pub(super) fn bake_annotations_into_base(&mut self) {
        if self.annotations.committed().is_empty() {
            return;
        }
        let (w, h, ref orig) = *self.base_crop;
        let mut baked = orig.clone();
        let scale = self.physical_scale();
        Annotations::rasterize_shapes(
            self.annotations.committed().iter(),
            &mut baked,
            w,
            h,
            point(px(0.), px(0.)),
            scale,
        );
        self.base_crop = Arc::new((w, h, baked));
        self.annotations.reset();
    }

    /// Recompute `composited_crop`, `image`, and `thumb_image` from `base_crop`
    /// + committed annotations + color effects.
    pub(super) fn rebuild_visuals(&mut self) {
        let (w, h, ref orig) = *self.base_crop;
        let mut pixels = orig.clone();
        if !self.annotations.committed().is_empty() {
            let scale = self.physical_scale();
            Annotations::rasterize_shapes(
                self.annotations.committed().iter(),
                &mut pixels,
                w,
                h,
                point(px(0.), px(0.)),
                scale,
            );
        }
        apply_color_effects(&mut pixels, self.grayscale, self.invert);
        let (tw, th, thumb_rgba) = center_square_crop(w, h, &pixels);
        self.image = image_util::rgba_to_render_image(pixels.clone(), w, h);
        self.thumb_image = image_util::rgba_to_render_image(thumb_rgba, tw, th);
        self.composited_crop = Arc::new((w, h, pixels));
    }

    /// Fast live-draft preview during pointer drag in annotation mode.
    pub(super) fn rebuild_live_preview(&mut self) {
        let (w, h, ref orig) = *self.base_crop;
        let mut pixels = orig.clone();
        let scale = self.physical_scale();
        self.annotations
            .rasterize(&mut pixels, w, h, point(px(0.), px(0.)), scale);
        apply_color_effects(&mut pixels, self.grayscale, self.invert);
        self.image = image_util::rgba_to_render_image(pixels, w, h);
    }
}

struct PinBoard {
    pins: Vec<Pin>,
    next_id: u64,
    drag: Option<PinDrag>,
    menu: Option<(u64, Point<Pixels>)>,
    annotating: Option<u64>,
    annot_gesture: Option<annotate::PinAnnotGesture>,
    text_prompt: Option<annotate::PinTextPrompt>,
    closed_stack: Vec<ClosedPin>,
    history_store: Option<PinHistoryStore>,
    hidden: bool,
    save_dialog_open: bool,
    hud_seq: u64,
    windows: Vec<AnyWindowHandle>,
    outputs: Vec<Bounds<Pixels>>,
}

impl PinBoard {
    fn new(outputs: Vec<Bounds<Pixels>>) -> Self {
        Self {
            pins: Vec::new(),
            next_id: 0,
            drag: None,
            menu: None,
            annotating: None,
            annot_gesture: None,
            text_prompt: None,
            closed_stack: Vec::new(),
            history_store: None,
            hidden: false,
            save_dialog_open: false,
            hud_seq: 0,
            windows: Vec::new(),
            outputs,
        }
    }

    fn with_default_history(mut self) -> Self {
        if let Ok(store) = PinHistoryStore::default_store() {
            self.history_store = Some(store);
        }
        self
    }

    fn add(&mut self, spec: PinSpec, cx: &mut Context<Self>) {
        let record_new = spec.history_id.is_none();
        self.add_internal(spec, None, record_new, cx);
    }

    pub(super) fn add_internal(
        &mut self,
        spec: PinSpec,
        explicit_base: Option<Size<Pixels>>,
        record_history: bool,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut rect = spec.rect;
        rect.origin = clamp_origin(rect.origin, rect.size, &self.outputs);
        let base = explicit_base.unwrap_or(spec.rect.size);
        let pin_cfg = crate::model::settings::current().pin();
        let opacity = spec.opacity.unwrap_or(pin_cfg.opacity).clamp(20, 100);

        let history_id = if record_history {
            self.history_store.as_ref().and_then(|store| {
                match store.record(
                    crate::model::pin_history::NewPinRecord {
                        width: spec.w,
                        height: spec.h,
                        rgba: &spec.rgba,
                        rect,
                        opacity,
                        label: spec.label.clone(),
                    },
                    pin_cfg.max_history,
                ) {
                    Ok(saved) => saved.map(|e| e.id),
                    Err(e) => {
                        eprintln!("[shotori] pin history record failed: {e:#}");
                        None
                    }
                }
            })
        } else {
            spec.history_id.clone()
        };

        self.pins
            .push(Pin::from_spec(id, spec, base, rect, opacity, history_id));
        self.hidden = false;
        self.menu = None;
        self.drag = None;
        cx.notify();
        id
    }

    /// The menu's pin while a menu is open; otherwise the topmost interactive
    /// pin (falling back to the last pin when every pin is in ghost mode).
    pub(super) fn selected_id(&self) -> Option<u64> {
        self.menu
            .map(|(id, _)| id)
            .or(self.annotating)
            .or_else(|| self.pins.iter().rev().find(|p| !p.ghost).map(|p| p.id))
            .or_else(|| self.pins.last().map(|pin| pin.id))
    }

    /// Composited crop of one pin (includes transforms, filters, and annotations).
    pub(super) fn crop_of(&self, id: u64) -> Option<Arc<(u32, u32, Vec<u8>)>> {
        self.pins
            .iter()
            .find(|pin| pin.id == id)
            .map(|pin| pin.composited_crop.clone())
    }

    /// Back to 100%, keeping the visual center anchored and grabbable.
    pub(super) fn reset_zoom(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(pin) = self.pins.iter_mut().find(|pin| pin.id == id) {
            if let Some(saved) = pin.collapsed.take() {
                pin.rect = expand_bounds(pin.rect, saved, &self.outputs);
            }
            let center = point(
                pin.rect.origin.x + pin.rect.size.width / 2.,
                pin.rect.origin.y + pin.rect.size.height / 2.,
            );
            let origin = point(
                center.x - pin.base.width / 2.,
                center.y - pin.base.height / 2.,
            );
            pin.rect = Bounds::new(clamp_origin(origin, pin.base, &self.outputs), pin.base);
            self.show_hud(id, "100%".into(), cx);
            self.sync_pin_to_history(id);
        }
    }

    pub(super) fn raise(&mut self, id: u64) {
        if let Some(ix) = self.pins.iter().position(|pin| pin.id == id) {
            let pin = self.pins.remove(ix);
            self.pins.push(pin);
        }
    }

    pub(super) fn move_drag(&mut self, position: Point<Pixels>, bypass_snap: bool) {
        let Some(mut drag) = self.drag else {
            return;
        };
        let travel = f32::from(position.x - drag.press_global.x)
            .hypot(f32::from(position.y - drag.press_global.y));
        if travel > 4.0 {
            drag.moved = true;
        }
        self.drag = Some(drag);

        let siblings: Vec<Bounds<Pixels>> = self
            .pins
            .iter()
            .filter(|p| p.id != drag.id && !self.hidden)
            .map(|p| p.rect)
            .collect();
        let cfg = crate::model::settings::current().pin();
        if let Some(pin) = self.pins.iter_mut().find(|pin| pin.id == drag.id) {
            let mut raw = position - drag.grab;
            if !bypass_snap && (cfg.snap_to_edge || cfg.snap_to_pins) {
                raw = snap_pin_origin(
                    raw,
                    pin.rect.size,
                    &self.outputs,
                    &siblings,
                    cfg.snap_to_edge,
                    cfg.snap_to_pins,
                );
            }
            pin.rect.origin = clamp_origin(raw, pin.rect.size, &self.outputs);
        }
    }

    pub(super) fn finish_drag(&mut self, position: Point<Pixels>, bypass_snap: bool) {
        self.move_drag(position, bypass_snap);
        let Some(drag) = self.drag.take() else {
            return;
        };
        // A clean click (no drag travel) on a collapsed thumbnail expands it.
        if !drag.moved
            && let Some(pin) = self.pins.iter_mut().find(|p| p.id == drag.id)
            && let Some(saved) = pin.collapsed.take()
        {
            pin.rect = expand_bounds(pin.rect, saved, &self.outputs);
        }
        self.sync_pin_to_history(drag.id);
    }

    /// Zoom anchored at `cursor`: the image point under the pointer stays under
    /// the pointer, clamped so a grabbable slice remains on the desktop.
    fn zoom_at(&mut self, id: u64, lines: f32, cursor: Point<Pixels>) -> Option<u32> {
        let pin = self.pins.iter_mut().find(|pin| pin.id == id)?;
        if pin.collapsed.is_some() {
            return None;
        }
        let z0 = f32::from(pin.rect.size.width) / f32::from(pin.base.width).max(1.);
        let z1 = next_zoom(z0, lines);
        let size = zoomed_size(pin.base, z1);
        let img = point(
            (f32::from(cursor.x - pin.rect.origin.x) / z0).clamp(0., f32::from(pin.base.width)),
            (f32::from(cursor.y - pin.rect.origin.y) / z0).clamp(0., f32::from(pin.base.height)),
        );
        let origin = point(cursor.x - px(img.x * z1), cursor.y - px(img.y * z1));
        pin.rect = Bounds::new(clamp_origin(origin, size, &self.outputs), size);
        Some((z1 * 100.).round() as u32)
    }

    pub(super) fn zoom_at_with_hud(
        &mut self,
        id: u64,
        lines: f32,
        cursor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(pct) = self.zoom_at(id, lines, cursor) {
            self.show_hud(id, format!("{pct}%"), cx);
            self.sync_pin_to_history(id);
        }
    }
}

struct PinSurface {
    board: Entity<PinBoard>,
    focus_handle: FocusHandle,
    output: PinOutput,
    _subscription: Subscription,
    _activation: Subscription,
}

impl PinSurface {
    fn new(
        board: Entity<PinBoard>,
        output: PinOutput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let subscription = cx.observe(&board, |_, _, cx| cx.notify());
        let activation = cx.observe_window_activation(window, |_, _, cx| cx.notify());
        Self {
            board,
            focus_handle,
            output,
            _subscription: subscription,
            _activation: activation,
        }
    }
}

pub(crate) struct PinSpec {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub rect: Bounds<Pixels>,
    pub opacity: Option<u8>,
    pub history_id: Option<String>,
    pub label: Option<String>,
}

impl PinSpec {
    pub(crate) fn new(w: u32, h: u32, rgba: Vec<u8>, rect: Bounds<Pixels>) -> Self {
        Self {
            w,
            h,
            rgba,
            rect,
            opacity: None,
            history_id: None,
            label: None,
        }
    }
}

/// Build the scene once. Later requests add to this same scene rather than
/// opening more layer surfaces whose stacking would be compositor-dependent.
fn open_board(spec: PinSpec, cx: &mut App) -> anyhow::Result<Entity<PinBoard>> {
    let outputs = outputs();
    anyhow::ensure!(!outputs.is_empty(), "no outputs registered");
    let board = cx.new(|_| {
        PinBoard::new(outputs.iter().map(|output| output.bounds).collect()).with_default_history()
    });
    board.update(cx, |board, cx| board.add(spec, cx));
    let mut windows: Vec<AnyWindowHandle> = Vec::new();
    for output in outputs {
        let st = board.clone();
        let handle = cx.open_window(
            WindowOptions {
                app_id: Some(crate::APP_ID.into()),
                titlebar: None,
                window_background: WindowBackgroundAppearance::Transparent,
                focus: false,
                display_id: output.display_id,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    {
                        #[cfg(target_os = "linux")]
                        {
                            point(px(0.), px(0.))
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            output.bounds.origin
                        }
                    },
                    output.bounds.size,
                ))),
                #[cfg(target_os = "linux")]
                kind: WindowKind::LayerShell(LayerShellOptions {
                    namespace: "shotori-pin".into(),
                    layer: Layer::Top,
                    anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                    exclusive_zone: Some(px(-1.)),
                    keyboard_interactivity: KeyboardInteractivity::OnDemand,
                    ..Default::default()
                }),
                #[cfg(not(target_os = "linux"))]
                kind: WindowKind::PopUp,
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| PinSurface::new(st, output, window, cx)),
        );
        match handle {
            Ok(handle) => windows.push(handle.into()),
            Err(error) => {
                for handle in windows {
                    drop(handle.update(cx, |_, window, _| window.remove_window()));
                }
                return Err(error);
            }
        }
    }
    board.update(cx, |board, _| board.windows = windows);
    Ok(board)
}

mod annotate;
mod menu;
mod ops;
#[cfg(target_os = "linux")]
mod render;
mod transport;

#[cfg(target_os = "linux")]
pub(crate) use transport::{ControlCommand, Prepared, prepare, send_control};

#[cfg(not(target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlCommand {
    UnlockAll,
    ToggleVisibility,
    CloseAll,
    RestoreLast,
}
#[cfg(not(target_os = "linux"))]
pub(crate) struct Prepared(PinSpec);
#[cfg(not(target_os = "linux"))]
pub(crate) fn prepare(spec: PinSpec) -> anyhow::Result<Prepared> {
    Ok(Prepared(spec))
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn send_control(_cmd: ControlCommand) -> anyhow::Result<bool> {
    Ok(false)
}

pub(crate) fn open(prepared: Prepared, cx: &mut App) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    match prepared {
        Prepared::Forwarded => Ok(()),
        Prepared::Owner(spec, server) => {
            let requests = server.start()?;
            let board = open_board(spec, cx)?;
            cx.spawn(async move |cx| {
                while let Ok(request) = requests.recv().await {
                    let result = cx.update(|cx| {
                        anyhow::ensure!(
                            !board.read(cx).pins.is_empty(),
                            "pin session is closing; try again"
                        );
                        match request.incoming {
                            transport::Incoming::Add(spec) => {
                                board.update(cx, |board, cx| board.add(spec, cx));
                                Ok(())
                            }
                            transport::Incoming::Control(cmd) => match cmd {
                                ControlCommand::UnlockAll => {
                                    board.update(cx, |b, cx| b.unlock_all_ghosts(cx));
                                    Ok(())
                                }
                                ControlCommand::ToggleVisibility => {
                                    board.update(cx, |b, cx| b.toggle_visibility(cx));
                                    Ok(())
                                }
                                ControlCommand::CloseAll => {
                                    let handles = board.update(cx, |b, cx| {
                                        let ids: Vec<u64> = b.pins.iter().map(|p| p.id).collect();
                                        for id in ids {
                                            b.remove_pin_by_id(id, cx);
                                        }
                                        b.windows.clone()
                                    });
                                    for handle in handles {
                                        drop(handle.update(cx, |_, w, _| w.remove_window()));
                                    }
                                    Ok(())
                                }
                                ControlCommand::RestoreLast => {
                                    let ok = board.update(cx, |b, cx| b.restore_last_closed(cx));
                                    anyhow::ensure!(ok, "no closed pin available to restore");
                                    Ok(())
                                }
                            },
                        }
                    });
                    drop(
                        request
                            .reply
                            .send(result.map_err(|error: anyhow::Error| error.to_string())),
                    );
                }
            })
            .detach();
            Ok(())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        open_board(prepared.0, cx).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_VISIBLE, ZOOM_MAX, ZOOM_MIN, clamp_origin, next_zoom, next_zoom_with, zoomed_size,
    };
    use gpui_kit::{Bounds, Pixels, point, px, size};

    fn desk() -> Bounds<Pixels> {
        Bounds::new(point(px(-100.), px(0.)), size(px(1100.), px(500.)))
    }

    #[test]
    fn zoom_tracks_the_base_size_and_clamps() {
        let base = size(px(100.), px(60.));
        let mut zoom = next_zoom(1., 3.);
        assert!((zoom - 1.12_f32.powi(3)).abs() < 1e-4);
        assert!((next_zoom_with(1., 2., 25) - 1.25_f32.powi(2)).abs() < 1e-4);
        let s = zoomed_size(base, zoom);
        assert!((f32::from(s.width) - 100. * zoom).abs() < 0.01);
        assert!((f32::from(s.height) - 60. * zoom).abs() < 0.01);

        for _ in 0..200 {
            zoom = next_zoom(zoom, 3.);
        }
        assert_eq!(zoom, ZOOM_MAX);
        for _ in 0..400 {
            zoom = next_zoom(zoom, -3.);
        }
        assert_eq!(zoom, ZOOM_MIN);
        let s = zoomed_size(size(px(3.), px(3.)), ZOOM_MIN);
        assert_eq!(
            (f32::from(s.width) as u32, f32::from(s.height) as u32),
            (1, 1)
        );
    }

    #[test]
    fn dragging_avoids_gaps_and_handles_staggered_outputs() {
        let outputs = [
            Bounds::new(point(px(-200.), px(0.)), size(px(100.), px(100.))),
            Bounds::new(point(px(0.), px(150.)), size(px(120.), px(150.))),
        ];
        for pin in [size(px(20.), px(20.)), size(px(200.), px(100.))] {
            for x in [-600., -150., -50., 0., 200.] {
                for y in [-100., 50., 120., 200., 500.] {
                    let requested = point(px(x), px(y));
                    let origin = clamp_origin(requested, pin, &outputs);
                    let visible = |origin| {
                        outputs.iter().any(|output| {
                            let intersection = Bounds::new(origin, pin).intersect(output);
                            intersection.size.width >= pin.width.min(px(MIN_VISIBLE))
                                && intersection.size.height >= pin.height.min(px(MIN_VISIBLE))
                        })
                    };
                    assert!(visible(origin));
                    if visible(requested) {
                        assert_eq!(origin, requested);
                    }
                }
            }
        }
    }

    #[test]
    fn dragging_keeps_a_grabbable_slice_on_the_desktop() {
        let pin: gpui_kit::Size<Pixels> = size(px(100.), px(60.));

        let p = clamp_origin(point(px(120.), px(80.)), pin, &[desk()]);
        assert_eq!((p.x, p.y), (px(120.), px(80.)));

        let p = clamp_origin(point(px(-5000.), px(-5000.)), pin, &[desk()]);
        assert_eq!(
            (p.x, p.y),
            (px(-100. - (100. - MIN_VISIBLE)), px(-(60. - MIN_VISIBLE)))
        );

        let p = clamp_origin(point(px(9000.), px(9000.)), pin, &[desk()]);
        assert_eq!(
            (p.x, p.y),
            (px(1000. - MIN_VISIBLE), px(500. - MIN_VISIBLE))
        );

        let tiny: gpui_kit::Size<Pixels> = size(px(10.), px(8.));
        let p = clamp_origin(point(px(-500.), px(-500.)), tiny, &[desk()]);
        assert_eq!((p.x, p.y), (px(-100.), px(0.)));
    }

    #[test]
    fn degenerate_output_size_does_not_panic() {
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(10.), px(10.)));
        let pin = size(px(100.), px(60.));
        let p = clamp_origin(point(px(50.), px(50.)), pin, &[tiny]);
        assert!(f32::from(p.x).is_finite());
        assert!(f32::from(p.y).is_finite());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::{PinBoard, PinOutput, PinSpec, PinSurface};
    use gpui_kit::{AppContext, Bounds, Entity, MouseButton, TestAppContext, point, px, size};

    fn spec(x: f32, y: f32) -> PinSpec {
        PinSpec::new(
            100,
            60,
            vec![255; 100 * 60 * 4],
            Bounds::new(point(px(x), px(y)), size(px(100.), px(60.))),
        )
    }
    fn board(cx: &mut TestAppContext) -> Entity<PinBoard> {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::bind_keys(cx, &crate::actions::default_keys());
        });
        cx.new(|_| {
            PinBoard::new(vec![
                Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
                Bounds::new(point(px(200.), px(0.)), size(px(200.), px(200.))),
            ])
        })
    }

    #[gpui_kit::test]
    fn selecting_an_exposed_pin_raises_it_and_overlap_targets_the_new_top(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| {
            board.add(spec(20., 20.), cx);
            board.add(spec(60., 40.), cx);
        });
        let (_, vcx) = cx.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        vcx.simulate_resize(size(px(200.), px(200.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.simulate_click(point(px(30.), px(30.)), Default::default());
        vcx.update(|window, cx| {
            assert_eq!(
                board.read(cx).pins.iter().map(|p| p.id).collect::<Vec<_>>(),
                vec![2, 1]
            );
            window.draw(cx).clear(cx);
        });
        // Both images contain this point: only the raised image may grab it.
        vcx.simulate_mouse_down(
            point(px(80.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(90.), px(60.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.update(|window, cx| {
            let state = board.read(cx);
            assert_eq!(state.pins[0].rect.origin, point(px(60.), px(40.)));
            assert_eq!(state.pins[1].rect.origin, point(px(30.), px(30.)));
            assert!(state.drag.is_none());
            window.draw(cx).clear(cx);
        });
        vcx.simulate_mouse_down(
            point(px(90.), px(60.)),
            MouseButton::Right,
            Default::default(),
        );
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.simulate_keystrokes("enter");
        vcx.update(|_, cx| {
            assert_eq!(board.read(cx).pins.len(), 1);
            assert_eq!(board.read(cx).pins[0].id, 2);
        });
        assert_eq!(vcx.windows().len(), 1);
    }

    #[gpui_kit::test]
    fn cross_output_layout_and_order_share_the_same_geometry(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| {
            board.add(spec(150., 20.), cx);
            board.add(spec(160., 30.), cx);
        });
        let mut other = cx.clone();
        let (_, first) = cx.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        let (_, second) = other.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(200.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        for vcx in [&mut *first, &mut *second] {
            vcx.simulate_resize(size(px(200.), px(200.)));
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        }
        assert_eq!(
            first.debug_bounds("pin-image-1").unwrap().origin,
            point(px(149.), px(19.))
        );
        assert_eq!(
            second.debug_bounds("pin-image-1").unwrap().origin,
            point(px(-51.), px(19.))
        );
        assert_eq!(
            second.debug_bounds("pin-image-1").unwrap().size,
            size(px(102.), px(62.))
        );
        assert_eq!(
            second.debug_bounds("pin-img-1").unwrap().origin,
            point(px(-50.), px(20.))
        );
        assert_eq!(
            second.debug_bounds("pin-img-1").unwrap().size,
            size(px(100.), px(60.))
        );
        first.simulate_click(point(px(155.), px(25.)), Default::default());
        second.update(|window, cx| window.draw(cx).clear(cx));
        second.simulate_mouse_down(
            point(px(20.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        second.update(|_, cx| assert_eq!(board.read(cx).drag.unwrap().id, 1));
        second.simulate_mouse_up(
            point(px(180.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        first.update(|window, cx| window.draw(cx).clear(cx));
        assert!(first.debug_bounds("pin-image-1").is_none());
    }

    #[gpui_kit::test]
    fn menu_escape_outside_click_and_close_on_all_outputs(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| board.add(spec(150., 150.), cx));
        let mut other = cx.clone();
        let (_, first) = cx.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        let (_, second) = other.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(200.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        let mut handles = Vec::new();
        for vcx in [&mut *first, &mut *second] {
            vcx.simulate_resize(size(px(200.), px(200.)));
            handles.push(vcx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.window_handle()
            }));
        }
        board.update(first, |board, _| board.windows = handles.clone());
        first.simulate_mouse_down(
            point(px(190.), px(190.)),
            MouseButton::Right,
            Default::default(),
        );
        first.update(|window, cx| window.draw(cx).clear(cx));
        let menu = first.debug_bounds("pin-menu").unwrap();
        assert_eq!(menu.bottom_right(), point(px(200.), px(200.)));
        first.simulate_keystrokes("escape");
        first.update(|window, cx| window.draw(cx).clear(cx));
        assert!(first.debug_bounds("pin-menu").is_none());
        first.simulate_keystrokes("shift-f10");
        second.update(|window, cx| window.draw(cx).clear(cx));
        second.simulate_click(point(px(180.), px(10.)), Default::default());
        first.update(|window, cx| {
            assert!(board.read(cx).menu.is_none());
            assert!(board.read(cx).drag.is_none());
            window.draw(cx).clear(cx);
        });
        first.simulate_keystrokes("shift-f10");
        first.update(|window, cx| window.draw(cx).clear(cx));
        let button = first.debug_bounds("pin-close").unwrap();
        first.simulate_click(button.center(), Default::default());
        first.run_until_parked();
        for handle in handles {
            assert!(!first.windows().contains(&handle));
        }
    }

    #[gpui_kit::test]
    fn escape_without_menu_closes_the_topmost_pin(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| {
            board.add(spec(20., 20.), cx);
            board.add(spec(60., 40.), cx);
        });
        let mut other = cx.clone();
        let (_, first) = cx.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        let (_, second) = other.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(200.), px(0.)), size(px(200.), px(200.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        let mut handles = Vec::new();
        for vcx in [&mut *first, &mut *second] {
            vcx.simulate_resize(size(px(200.), px(200.)));
            handles.push(vcx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.window_handle()
            }));
        }
        board.update(first, |board, _| board.windows = handles.clone());

        first.simulate_click(point(px(80.), px(50.)), Default::default());
        first.update(|_, cx| {
            assert_eq!(
                board
                    .read(cx)
                    .pins
                    .iter()
                    .map(|pin| pin.id)
                    .collect::<Vec<_>>(),
                vec![1, 2]
            );
        });
        first.simulate_keystrokes("escape");
        first.run_until_parked();
        first.update(|_, cx| {
            let state = board.read(cx);
            assert_eq!(
                state.pins.iter().map(|pin| pin.id).collect::<Vec<_>>(),
                vec![1]
            );
            assert!(state.drag.is_none());
        });
        assert_eq!(first.windows().len(), 2, "one pin remains — surfaces stay");
        first.simulate_keystrokes("escape");
        first.run_until_parked();
        other.update(|cx| assert!(board.read(cx).pins.is_empty()));
        for handle in handles {
            assert!(!first.windows().contains(&handle));
        }
    }

    #[gpui_kit::test]
    fn shrinking_at_the_top_left_remains_grabbable(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| {
            board.add(spec(-100., -100.), cx);
            for _ in 0..20 {
                let center = board.pins[0].rect.center();
                board.zoom_at(1, -2., center);
                let rect = board.pins[0].rect;
                let best = board
                    .outputs
                    .iter()
                    .fold(size(px(0.), px(0.)), |acc, output| {
                        let visible = rect.intersect(output).size;
                        size(acc.width.max(visible.width), acc.height.max(visible.height))
                    });
                let need_w = rect.size.width.min(px(super::MIN_VISIBLE));
                let need_h = rect.size.height.min(px(super::MIN_VISIBLE));
                assert!(best.width - need_w > px(-0.05));
                assert!(best.height - need_h > px(-0.05));
            }
        });
    }

    #[gpui_kit::test]
    fn zoom_anchors_on_the_cursor_point(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| board.add(spec(20., 20.), cx));
        board.update(cx, |board, _| {
            let cursor = point(px(90.), px(70.));
            for lines in [2., -1., 3., -2., 4.] {
                let before = board.pins[0].rect;
                let base = board.pins[0].base;
                let z0 = f32::from(before.size.width) / f32::from(base.width);
                let img = (
                    f32::from(cursor.x - before.origin.x) / z0,
                    f32::from(cursor.y - before.origin.y) / z0,
                );
                board.zoom_at(1, lines, cursor);
                let after = board.pins[0].rect;
                let z1 = f32::from(after.size.width) / f32::from(base.width);
                let back = (
                    f32::from(cursor.x - after.origin.x) / z1,
                    f32::from(cursor.y - after.origin.y) / z1,
                );
                let unclamped = point(cursor.x - px(img.0 * z1), cursor.y - px(img.1 * z1));
                let expected = super::zoomed_size(base, z1);
                assert!(
                    (f32::from(after.size.width) - f32::from(expected.width)).abs() < 0.01
                        && (f32::from(after.size.height) - f32::from(expected.height)).abs() < 0.01
                );
                if after.origin == unclamped {
                    assert!(
                        (back.0 - img.0).abs() < 0.5 && (back.1 - img.1).abs() < 0.5,
                        "anchor drifted: {back:?} vs {img:?}"
                    );
                }
                assert!(after.contains(&cursor), "cursor left the pin");
            }
        });
    }

    /// Flagship capabilities: rotate, flip, opacity, collapse/expand, ghost
    /// click-through + corner unlock, nudge, closed-pin restore (`Ctrl+Shift+T`),
    /// and in-pin secondary annotation with `Ctrl+Z` undo and `Esc` innermost exit.
    #[gpui_kit::test]
    fn flagship_transforms_ghost_collapse_restore_and_in_pin_annotation(cx: &mut TestAppContext) {
        let board = board(cx);
        board.update(cx, |board, cx| {
            board.add(spec(20., 20.), cx);
            board.add(spec(70., 50.), cx);
        });
        let (_, vcx) = cx.add_window_view(|window, cx| {
            PinSurface::new(
                board.clone(),
                PinOutput {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.))),
                    display_id: None,
                    scale: 1.0,
                },
                window,
                cx,
            )
        });
        vcx.simulate_resize(size(px(400.), px(300.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        // 1. Rotate 90° CW swaps 100×60 → 60×100; Shift+R rotates back to 100×60.
        vcx.simulate_keystrokes("r");
        vcx.update(|_, cx| {
            let top = board.read(cx).pins.last().unwrap().rect.size;
            assert_eq!(top, size(px(60.), px(100.)));
        });
        vcx.simulate_keystrokes("shift-r");
        vcx.update(|_, cx| {
            let top = board.read(cx).pins.last().unwrap().rect.size;
            assert_eq!(top, size(px(100.), px(60.)));
        });

        // 2. Opacity stepping (`[` / `]`) and arrow nudge (`shift-right`).
        vcx.simulate_keystrokes("[");
        vcx.simulate_keystrokes("shift-right");
        vcx.update(|_, cx| {
            let top = board.read(cx).pins.last().unwrap();
            assert_eq!(top.opacity, 90);
            assert_eq!(top.rect.origin, point(px(80.), px(50.)));
        });

        // 3. Collapse (`t`) shrinks to 56×56; clicking without drag expands back!
        vcx.simulate_keystrokes("t");
        vcx.update(|window, cx| {
            let top = board.read(cx).pins.last().unwrap();
            assert!(top.collapsed.is_some());
            assert_eq!(top.rect.size, size(px(56.), px(56.)));
            window.draw(cx).clear(cx);
        });
        vcx.simulate_click(point(px(90.), px(60.)), Default::default());
        vcx.update(|window, cx| {
            let top = board.read(cx).pins.last().unwrap();
            assert!(top.collapsed.is_none());
            assert_eq!(top.rect.size, size(px(100.), px(60.)));
            window.draw(cx).clear(cx);
        });

        // 4. Ghost click-through (`l`) exposes the corner unlock button and clicking it unlocks.
        vcx.simulate_keystrokes("l");
        vcx.update(|window, cx| {
            assert!(board.read(cx).pins.last().unwrap().ghost);
            window.draw(cx).clear(cx);
        });
        let unlock_bounds = vcx.debug_bounds("pin-ghost-unlock-2").unwrap();
        vcx.simulate_click(unlock_bounds.center(), Default::default());
        vcx.update(|window, cx| {
            assert!(!board.read(cx).pins.last().unwrap().ghost);
            window.draw(cx).clear(cx);
        });

        // 5. In-pin annotation (`a`): draw a rectangle, undo (`ctrl-z`), redo (`ctrl-y`),
        //    and verify `escape` exits annotation mode WITHOUT closing the pin!
        vcx.simulate_keystrokes("a");
        vcx.update(|window, cx| {
            assert_eq!(board.read(cx).annotating, Some(2));
            window.draw(cx).clear(cx);
        });
        assert!(vcx.debug_bounds("pin-annot-bar").is_some());
        vcx.simulate_mouse_down(
            point(px(90.), px(60.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(140.), px(95.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(140.), px(95.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.update(|_, cx| {
            let top = board.read(cx).pins.last().unwrap();
            assert_eq!(top.annotations.committed().len(), 1);
            assert!(
                top.composited_crop.2.iter().any(|&b| b != 255),
                "committed annotation must composite into the pin's exported pixels"
            );
        });
        vcx.simulate_keystrokes("ctrl-z");
        vcx.update(|_, cx| {
            assert_eq!(
                board
                    .read(cx)
                    .pins
                    .last()
                    .unwrap()
                    .annotations
                    .committed()
                    .len(),
                0
            );
        });
        vcx.simulate_keystrokes("ctrl-y");
        vcx.update(|_, cx| {
            assert_eq!(
                board
                    .read(cx)
                    .pins
                    .last()
                    .unwrap()
                    .annotations
                    .committed()
                    .len(),
                1
            );
        });
        // Escape leaves annotation mode while keeping both pins open.
        vcx.simulate_keystrokes("escape");
        vcx.update(|_, cx| {
            assert!(board.read(cx).annotating.is_none());
            assert_eq!(board.read(cx).pins.len(), 2);
        });

        // 6. Close top pin (`escape`) and restore it with `ctrl-shift-t`!
        vcx.simulate_keystrokes("escape");
        vcx.update(|_, cx| assert_eq!(board.read(cx).pins.len(), 1));
        vcx.simulate_keystrokes("ctrl-shift-t");
        vcx.update(|_, cx| {
            let state = board.read(cx);
            assert_eq!(state.pins.len(), 2);
            assert_eq!(state.pins.last().unwrap().opacity, 90);
        });
    }

    #[test]
    fn edge_snap_and_custom_zoom_step_honor_geometry() {
        let outputs = vec![Bounds::new(
            point(px(0.), px(0.)),
            size(px(1920.), px(1080.)),
        )];
        let pin = size(px(200.), px(100.));
        assert_eq!(
            super::snap_origin_to_edges(point(px(7.), px(975.)), pin, &outputs),
            point(px(0.), px(980.))
        );
        assert_eq!(
            super::snap_origin_to_edges(point(px(50.), px(50.)), pin, &outputs),
            point(px(50.), px(50.))
        );
        assert!((super::next_zoom_with(1.0, 1.0, 25) - 1.25).abs() < 0.0001);
        assert!((super::next_zoom_with(1.0, 1.0, 12) - 1.12).abs() < 0.0001);
    }
}
