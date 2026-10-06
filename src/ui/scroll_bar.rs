//! # Scroll capture chrome: region frame + preview panel
//!
//! The overlays show FROZEN pixels — for the live content to scroll they
//! must unmap, and two surfaces replace them for the duration of a long
//! screenshot:
//!
//! - the **region frame** — ONE full-output transparent layer surface
//!   drawing four accent strokes around the selection. Its input region
//!   covers ONLY the strokes (the pin's `set_input_region` trick), so
//!   wheel events over the captured content pass through to the app —
//!   manual scrolling keeps working. Dragging a stroke MOVES the region:
//!   the shared capture rect updates live and the engine follows the
//!   frame to content that never scrolled into view (for the stitcher,
//!   "frame moves down" is indistinguishable from "content scrolls up").
//! - the **preview panel** — docked on the freer side of the screen:
//!   status, the action buttons and a downscaled stream of the growing
//!   canvas with a highlight marking WHERE in the long image the
//!   current viewport sits.
//!
//! Keyboard lives on the panel and mirrors the selection flow exactly:
//! Enter/Ctrl+C finish+copy, Ctrl+S finish+save, Esc cancels.

use std::sync::Arc;

#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::model::placement::{clamp_moved_rect, frame_grab_bands, frame_strokes, frame_toolbar};
use crate::model::scroll_stitch::StitchOptions;
use crate::model::session::ScrollRect;
use crate::platform::scroll_capture::{self, ScrollControls, ScrollEvent, ScrollSpec};
use crate::ui::image_util;
use crate::ui::theme;

actions!(scroll, [ScrollFinish, ScrollCancel, ScrollSave]);

/// Preview panel width (logical px).
const PANEL_W: f32 = 264.;
/// Panel padding (p_2) and border (border_1) — the preview image's
/// display width is derived from these (see [`PREVIEW_IMG_W`]).
const PANEL_PAD: f32 = 8.;
const PANEL_BORDER: f32 = 1.;
/// The preview image's display width: panel minus padding and border.
/// The img element's height is derived from this and the image's own
/// aspect — NEVER from the element's intrinsic sizing: gpui leaks the
/// BITMAP's pixel height into the layout as logical px, so on a 2×
/// screen the box rendered 2× taller than width × aspect, throwing the
/// highlight's relative() fractions (fractions of that box) off with
/// it — the whole "preview doesn't match my frame" saga (measured:
/// 1164 physical px of box where 590 was correct).
const PREVIEW_IMG_W: f32 = PANEL_W - 2. * (PANEL_PAD + PANEL_BORDER);

/// Everything `launch` needs about the target output, in logical px.
pub(crate) struct ScrollChrome {
    pub display_id: Option<DisplayId>,
    pub output_width: f32,
    pub output_height: f32,
}

/// Launch the scroll-capture chrome and start the engine. Must be called
/// BEFORE the overlays unmap — closing every window would end the app
/// loop with the capture half-started. Returns the shared capture rect
/// (already inside `spec`).
pub(crate) fn launch(spec: ScrollSpec, chrome: ScrollChrome, cx: &mut App) -> anyhow::Result<()> {
    let options = StitchOptions::default();

    // Region frame first: the user must never lose sight of WHAT is
    // being captured, not even for the frames before the panel maps.
    let frame_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        display_id: chrome.display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(chrome.output_width), px(chrome.output_height)),
        })),
        #[cfg(target_os = "linux")]
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-frame".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        }),
        #[cfg(not(target_os = "linux"))]
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    let rect_now = *spec
        .rect
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let shared_rect = spec.rect.clone();
    let frame_cell: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::WeakEntity<FrameView>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let cell = frame_cell.clone();
    let frame = cx.open_window(frame_options, |_window, cx| {
        cx.new(|cx| {
            let view = FrameView::new(
                shared_rect,
                rect_now,
                chrome.output_width,
                chrome.output_height,
            );
            *cell.borrow_mut() = Some(cx.entity().downgrade());
            view
        })
    });

    // The panel on the freer side of the selection.
    let side = if (rect_now.x + rect_now.width / 2) as f32 > chrome.output_width / 2. {
        Side::Left
    } else {
        Side::Right
    };
    let panel_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Opaque,
        focus: true,
        display_id: chrome.display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(PANEL_W), px((chrome.output_height - 20.).max(120.))),
        })),
        #[cfg(target_os = "linux")]
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-preview".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | side_anchor(side),
            exclusive_zone: Some(px(-1.)),
            // The panel owns the keyboard so the shortcuts match the
            // selection flow. NOTE: if the compositor routes wheel by
            // keyboard focus (niri does for virtual pointers), scrolling
            // degrades to frame-dragging — deliberately shipped both.
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            margin: Some((px(10.), px(0.), px(10.), px(0.))),
            ..Default::default()
        }),
        #[cfg(not(target_os = "linux"))]
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    let panel_cell: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::WeakEntity<PreviewPanel>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let pcell = panel_cell.clone();
    let panel = cx.open_window(panel_options, |window, cx| {
        cx.new(|cx| {
            let panel = PreviewPanel::new(spec, options, chrome.output_height, window, cx);
            *pcell.borrow_mut() = Some(cx.entity().downgrade());
            panel
        })
    });

    // The panel closes the frame on its terminal paths …
    if let Ok(panel) = &panel {
        let sibling = frame.as_ref().ok().map(|f| (*f).into());
        let _ = panel.update(cx, |p, _, _| {
            p.sibling = sibling;
        });
    }
    // … and the frame toolbar gets the engine + panel handles so its
    // buttons work without cross-window action dispatch.
    if let Some(frame) = frame_cell.borrow().as_ref().and_then(|w| w.upgrade())
        && let Some(panel) = panel_cell.borrow().as_ref().and_then(|w| w.upgrade())
    {
        let controls = panel.read(cx).controls();
        let weak_panel = panel.downgrade();
        frame.update(cx, |f, _| f.bind(controls, weak_panel));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn side_anchor(side: Side) -> Anchor {
    match side {
        Side::Left => Anchor::LEFT,
        Side::Right => Anchor::RIGHT,
    }
}

/// Which screen edge the preview panel docks to.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

// ── the region frame ─────────────────────────────────────────────────

/// One full-output transparent surface drawing the region frame and
/// owning its drag interaction.
pub(crate) struct FrameView {
    rect: ScrollRect,
    shared: Arc<std::sync::Mutex<ScrollRect>>,
    output: Size<Pixels>,
    /// (press point, rect at press, vertical-only?) — the flag marks
    /// drags started from the toolbar's grab button (the user drags
    /// the frame's VERTICAL position through it).
    drag: Option<(Point<Pixels>, ScrollRect, bool)>,
    /// Late-bound by `launch` (the panel starts the engine after the
    /// frame exists): toolbar exits without cross-window dispatch.
    controls: Option<ScrollControls>,
    panel: Option<gpui_kit::WeakEntity<PreviewPanel>>,
}

impl FrameView {
    /// `shared` is the VERY Arc the engine reads per capture — drags
    /// must write through it, not a lookalike.
    fn new(
        shared: Arc<std::sync::Mutex<ScrollRect>>,
        rect: ScrollRect,
        output_w: f32,
        output_h: f32,
    ) -> Self {
        Self {
            rect,
            shared,
            output: size(px(output_w), px(output_h)),
            drag: None,
            controls: None,
            panel: None,
        }
    }

    /// Late binding (see the field docs).
    pub(crate) fn bind(
        &mut self,
        controls: ScrollControls,
        panel: gpui_kit::WeakEntity<PreviewPanel>,
    ) {
        self.controls = Some(controls);
        self.panel = Some(panel);
    }

    /// The chrome painter: four strokes + the toolbar. `&mut self` with
    /// listeners (buttons need entity access).
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // Transparent full-output surface; the strokes and toolbar are
        // the only painted pixels. The input region (set in Render)
        // covers only their rects — the captured content stays
        // interactive for the app beneath.
        let strokes = frame_strokes(self.rect);
        let toolbar = frame_toolbar(
            self.rect,
            f32::from(self.output.width),
            f32::from(self.output.height),
        );
        // The GRAB button: press-and-hold enters drag mode — the frame
        // then follows the mouse's vertical position until release
        // (the existing window-level drag listeners carry the gesture,
        // implicit grab included). Same machinery as dragging a strip,
        // flagged vertical-only.
        let grab = cx.listener(|this, event: &MouseDownEvent, _, _| {
            this.drag = Some((event.position, this.rect, true));
        });
        let copy = cx.listener(|this, _: &MouseDownEvent, _, _| {
            if let Some(controls) = &this.controls {
                controls.finish(); // Finish ≡ finish+copy (panel default)
            }
        });
        let save = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if let Some(panel) = this.panel.clone()
                && let Some(panel) = panel.upgrade()
            {
                panel.update(cx, |panel, cx| panel.request_save(cx));
            }
        });
        let cancel = cx.listener(|this, _: &MouseDownEvent, _, _| {
            if let Some(controls) = &this.controls {
                controls.cancel();
            }
        });
        div()
            .size_full()
            // FIRST child: the sink canvas must sit under the toolbar
            // in paint order so the buttons win the element hit-test.
            .child(pointer_sink(cx.entity().downgrade(), self.output))
            .children(strokes.iter().map(|s| {
                div()
                    .absolute()
                    .left(s.origin.x)
                    .top(s.origin.y)
                    .w(s.size.width)
                    .h(s.size.height)
                    .bg(rgba(theme::c().accent))
            }))
            .child(
                div()
                    .id("frame-toolbar")
                    .absolute()
                    .left(toolbar.origin.x)
                    .top(toolbar.origin.y)
                    .w(toolbar.size.width)
                    .h(toolbar.size.height)
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded(px(8.))
                    .bg(rgba(theme::c().toolbar_bg))
                    .border_1()
                    .border_color(rgba(theme::c().pin_border))
                    .child(hold_button("frame-tb-grab", "⇕", 34., grab))
                    .child(hold_button("frame-tb-copy", "Copy", 52., copy))
                    .child(hold_button("frame-tb-save", "Save", 52., save))
                    .child(hold_button("frame-tb-cancel", "✕", 26., cancel)),
            )
    }
}

/// One toolbar button. Press semantics for every entry: the press
/// starts the effect (the grab button's drag ends on mouse-up via the
/// window listeners).
fn hold_button<F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>(
    id: &'static str,
    label: &'static str,
    w: f32,
    on_press: F,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(w))
        .flex_1()
        .h_full()
        .flex()
        .items_center()
        .justify_around()
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(rgba(theme::c().btn_text))
        .hover(|s| s.bg(rgba(theme::c().btn_hover_bg)))
        .on_mouse_down(MouseButton::Left, on_press)
        .child(label)
}

/// The invisible canvas that owns the frame window's pointer listeners.
///
/// `window.on_mouse_event` registrations live for ONE frame's event
/// dispatch — they must be re-attached during every paint (the
/// overlay's `pointer_event_sink` lesson; the v1.1 frame wired them
/// once in `new` and every registration was dead by the first event,
/// so neither strip drags nor the ⇕ button ever moved the frame).
/// Window-level, not element handlers: the implicit grab delivers the
/// whole gesture — including out-of-bounds releases — to the press
/// window, and element hit-testing would drop exactly those.
fn pointer_sink(weak: gpui_kit::WeakEntity<FrameView>, output: Size<Pixels>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let sink = weak.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = sink.update(cx, |this, _| {
                    // A press on a grab BAND starts a free drag. The ⇕
                    // button starts its own vertical-only drag in its
                    // element handler — its press position is over the
                    // toolbar, never a band, so the two cannot collide.
                    if frame_grab_bands(this.rect)
                        .iter()
                        .any(|band| band.contains(&event.position))
                    {
                        this.drag = Some((event.position, this.rect, false));
                    }
                });
            });
            let sink = weak.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let _ = sink.update(cx, |this, cx| {
                    let Some((start, origin, vertical_only)) = this.drag else {
                        return;
                    };
                    let (dx, dy) = if vertical_only {
                        (0., f32::from(event.position.y - start.y))
                    } else {
                        (
                            f32::from(event.position.x - start.x),
                            f32::from(event.position.y - start.y),
                        )
                    };
                    let next = clamp_moved_rect(
                        origin,
                        dx,
                        dy,
                        f32::from(output.width),
                        f32::from(output.height),
                    );
                    this.rect = next;
                    *this
                        .shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
                    cx.notify();
                });
            });
            let sink = weak.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = sink.update(cx, |this, cx| {
                    if this.drag.take().is_some() {
                        cx.notify();
                    }
                });
            });
        },
    )
    .absolute()
    .size_full()
}

impl Render for FrameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The input region must track the moving strokes and toolbar —
        // re-set every frame (cheap), same-source geometry as the
        // visuals.
        let mut hit: Vec<Bounds<Pixels>> = frame_grab_bands(self.rect).into();
        hit.push(frame_toolbar(
            self.rect,
            f32::from(self.output.width),
            f32::from(self.output.height),
        ));
        window.set_input_region(Some(&hit));
        FrameView::render(self, cx)
    }
}

// ── the preview panel ────────────────────────────────────────────────

pub(crate) struct PreviewPanel {
    focus: FocusHandle,
    controls: Option<ScrollControls>,
    state: PanelState,
    /// Ctrl+S was pressed: the finished canvas goes to the save flow
    /// instead of the clipboard.
    saving: bool,
    /// The region-frame window, closed on every terminal path.
    sibling: Option<AnyWindowHandle>,
    image: Option<Arc<RenderImage>>,
    /// Viewport highlight in PREVIEW-row coordinates (already mapped by
    /// the engine): (top, height).
    viewport: (u32, u32),
    preview_rows: u32,
    /// The preview bitmap's width in px (for the aspect-derived display
    /// height — see [`PREVIEW_IMG_W`]).
    preview_cols: u32,
    /// The host output's logical height — the preview column's visible
    /// window derives from it (the panel is anchored TOP|BOTTOM with
    /// 10 px margins).
    output_h: f32,
}

enum PanelState {
    /// Engine thread is connecting / capturing the first frame.
    Starting,
    /// Capturing.
    Running,
    /// Finishing on request; the canvas is on its way.
    Finishing,
    /// A terminal event fired; the window is going away.
    Done,
}

impl PreviewPanel {
    fn new(
        spec: ScrollSpec,
        options: StitchOptions,
        output_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::ui::theme::follow_system(window.appearance());
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // A spawn failure (not a compositor failure — those arrive as
        // Failed events) still yields a live event stream carrying the
        // error, so the lifecycle code stays single-shaped.
        let (controls, events) = match scroll_capture::start(spec, options) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[shotori] scroll engine failed to start: {e:#}");
                let (tx, rx) = async_channel::bounded(1);
                let _ = tx.try_send(ScrollEvent::Failed {
                    reason: format!("could not start the scroll engine: {e:#}"),
                });
                (ScrollControls::dead(), rx)
            }
        };

        let this = Self {
            focus: focus_handle,
            controls: Some(controls),
            state: PanelState::Starting,
            saving: false,
            sibling: None,
            image: None,
            viewport: (0, 0),
            preview_rows: 0,
            preview_cols: 1,
            output_h,
        };

        // The event pump: engine → entity updates; ends with the channel
        // (the engine closes its sender on exit).
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            while let Ok(event) = events.recv().await {
                if weak
                    .update(cx, |panel, cx| panel.on_event(event, &handle, cx))
                    .is_err()
                {
                    break; // window gone; nothing left to update
                }
            }
        })
        .detach();
        this
    }

    /// The engine handle, for the frame toolbar's late binding.
    pub(crate) fn controls(&self) -> ScrollControls {
        self.controls.clone().unwrap_or_else(ScrollControls::dead)
    }

    /// Finish into the SAVE flow (the frame toolbar's Save button —
    /// no action dispatch across windows, a direct entity call).
    pub(crate) fn request_save(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            self.saving = true;
            self.state = PanelState::Finishing;
            controls.finish();
            cx.notify();
        }
    }

    fn close_sibling(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.sibling.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn on_event(&mut self, event: ScrollEvent, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        match event {
            ScrollEvent::Started { .. } => {
                self.state = PanelState::Running;
                cx.notify();
            }
            ScrollEvent::Progress { .. } => {
                // Row counters were panel status copy once; the panel is
                // a pure preview now and progress rides the image stream.
            }
            ScrollEvent::Viewport { top, height } => {
                // The pixel-free twin of Preview's viewport fields: the
                // highlight follows frame drags without image resend.
                self.viewport = (top, height);
                cx.notify();
            }
            ScrollEvent::Preview {
                width,
                height,
                rgba,
                tail_start: _,
                viewport_top,
                viewport_height,
            } => {
                self.image = Some(image_util::rgba_to_render_image(
                    (*rgba).clone(),
                    width,
                    height,
                ));
                self.preview_rows = height;
                self.preview_cols = width.max(1);
                // The engine already mapped the span into PREVIEW rows
                // (top clamped inside the tail); the panel only turns
                // rows into fractions of the preview height.
                self.viewport = (viewport_top, viewport_height);
                cx.notify();
            }
            ScrollEvent::Finished {
                width,
                height,
                rgba,
            } => {
                if matches!(self.state, PanelState::Done) {
                    return;
                }
                self.state = PanelState::Done;
                cx.notify();
                self.controls = None;
                let rgba = rgba.clone();
                if self.saving {
                    self.save_and_quit(&rgba, width, height, window, cx);
                } else {
                    self.copy_and_close(&rgba, width, height, window, cx);
                }
            }
            ScrollEvent::Failed { reason } => {
                eprintln!("[shotori] scroll failed: {reason}");
                crate::notify::send("Long screenshot failed", &reason);
                self.close(window, cx);
            }
            ScrollEvent::Cancelled => {
                println!("[shotori] scroll cancelled");
                self.close(window, cx);
            }
        }
    }

    /// The default exit: PNG → clipboard (background, never blocking
    /// the panel) → notification → the windows close LAST, so the async
    /// copy always completes while the app loop still runs.
    fn copy_and_close(
        &mut self,
        rgba: &Arc<Vec<u8>>,
        width: u32,
        height: u32,
        window: &AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let rgba = rgba.clone();
        let window = *window;
        self.close_sibling(cx);
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let png = crate::model::export::encode_png_fast(width, height, &rgba)?;
                    crate::clipboard::copy_image(width, height, &rgba, &png)?;
                    Ok::<Vec<u8>, anyhow::Error>(png)
                })
                .await;
            match result {
                Ok(png) => {
                    println!("[shotori] long screenshot {width}x{height} copied");
                    crate::notify::copied(&png);
                }
                Err(e) => {
                    eprintln!("[shotori] long screenshot copy failed: {e:#}");
                    crate::notify::send(
                        "Couldn’t copy the long screenshot",
                        "The clipboard refused the image.",
                    );
                }
            }
            let _ = window.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    }

    /// The Ctrl+S exit — mirrors the overlay save flow exactly: stash,
    /// unmap, quit from a timer so the connection flushes; the portal
    /// dialog then opens from `save_dialog::complete_pending` in main.
    fn save_and_quit(
        &mut self,
        rgba: &Arc<Vec<u8>>,
        width: u32,
        height: u32,
        window: &AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        crate::save_dialog::stash(width, height, rgba.to_vec());
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        self.close_sibling(cx);
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    fn close(&mut self, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        self.state = PanelState::Done;
        self.close_sibling(cx);
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.notify();
    }

    /// Enter / Ctrl+C / [Copy]: stop the engine now; whatever is
    /// stitched is the deliverable.
    fn finish(&mut self, _: &ScrollFinish, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls
            && !matches!(self.state, PanelState::Done)
        {
            self.state = PanelState::Finishing;
            controls.finish();
            cx.notify();
        }
    }

    /// Esc / [Cancel]: discard everything.
    fn cancel(&mut self, _: &ScrollCancel, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.take() {
            controls.cancel();
            cx.notify();
        }
    }

    /// Ctrl+S: finish, then hand the canvas to the save flow.
    fn save(&mut self, _: &ScrollSave, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            self.saving = true;
            self.state = PanelState::Finishing;
            controls.finish();
            cx.notify();
        }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.state; // Starting/Finishing/Done differ only in logic now
        // The panel is a PURE PREVIEW (user-designed): the frame's
        // toolbar carries the buttons, the keyboard shortcuts stay
        // bound here invisibly. When the long image is TALLER than the
        // panel, the visible window FOLLOWS the viewport highlight —
        // a bottom-anchored image let the highlight scroll out of view
        // (clipped away entirely at v_top = 0: the "no upward sync"
        // report, where the highlight was pinned to an off-screen
        // image top). Fitting images still anchor to the bottom (the
        // growing edge stays put).
        let mut preview_area = div().flex_1().relative().overflow_hidden();
        if let Some(image) = self.image.clone() {
            let rows = self.preview_rows.max(1) as f32;
            let (v_top, v_h) = self.viewport;
            // Viewport events reference the CURRENT canvas while this
            // image is up to one preview-interval stale — v_top can
            // exceed `rows` mid-scroll. Cap below 1.0 so the height
            // clamp's min ≤ max always holds (an uncapped pair once
            // panicked the render pass: the "scrolling crashes" bug).
            let top_frac = (v_top as f32 / rows).min(0.99);
            let h_frac = (v_h as f32 / rows).clamp(0.01, 1. - top_frac);
            // EXPLICIT height from the known aspect — intrinsic sizing
            // is the HiDPI trap (see PREVIEW_IMG_W). The wrapper then
            // hugs exactly this box and the fractions below are
            // fractions of the IMAGE.
            let img_h = (PREVIEW_IMG_W * self.preview_rows as f32 / self.preview_cols as f32)
                .clamp(1., 8000.);
            // The visible column: panel window height (output minus the
            // 10 px layer-shell margins) minus padding and border.
            let area_h = (self.output_h - 20. - 2. * (PANEL_PAD + PANEL_BORDER)).max(60.);
            let y_off = if img_h <= area_h {
                area_h - img_h // bottom-anchored, as before
            } else {
                // Scroll the image so the highlight sits centered in
                // the visible window, clamped to the image's extent.
                let hl_center = (top_frac + h_frac / 2.) * img_h;
                (area_h / 2. - hl_center).clamp(area_h - img_h, 0.)
            };
            preview_area = preview_area.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(y_off))
                    .w_full()
                    .child(img(image).w_full().h(px(img_h)))
                    // viewport highlight — where the capture frame sits
                    // in the long image (accent outline, image-relative)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .w_full()
                            .top(relative(top_frac))
                            .h(relative(h_frac))
                            .border_1()
                            .border_color(rgba(theme::c().accent))
                            .rounded_sm(),
                    ),
            );
        }

        div()
            .id("shotori-scroll-panel")
            .key_context("ShotoriScroll")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .bg(rgba(theme::c().toolbar_bg))
            // Deliberately NOT pin_border: that is accent-orange at 60%
            // alpha and reads as a giant "highlight" around the whole
            // panel — the user kept mistaking it for the viewport box.
            .border_1()
            .border_color(rgba(0xFFFFFF20))
            .on_action(cx.listener(Self::finish))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save))
            .child(preview_area)
    }
}
