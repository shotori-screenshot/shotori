//! # Scroll capture chrome: control bar + region frame + live preview
//!
//! The overlays show FROZEN pixels — for the live content to scroll they
//! must unmap, and three surfaces replace them for the duration of a
//! long screenshot:
//!
//! - the **control bar** — a slim strip at the top edge: progress
//!   readout, [Finish] / [Cancel] buttons;
//! - the **region frame** — four 2-px layer-shell strips boxing the
//!   selection so the user keeps seeing WHAT is being captured (they
//!   sit strictly OUTSIDE the captured rect — screencopy photographs
//!   the whole output, chrome included);
//! - the **preview panel** — a dock on the freer side of the screen
//!   showing the stitched image growing (a downscaled canvas tail the
//!   engine streams).
//!
//! All three are mouse-only (keyboard_interactivity None): niri (and
//! sway) deliver wheel events to the KEYBOARD-FOCUSED surface — any
//! keyboard grab here would swallow the user's manual scrolling, which
//! is the whole point of the flow.

use std::sync::Arc;

use gpui_kit::base::Button;
#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::model::scroll_stitch::StitchOptions;
use crate::platform::scroll_capture::{self, ScrollControls, ScrollEvent, ScrollSpec};
use crate::ui::image_util;
use crate::ui::theme;

actions!(scroll, [ScrollFinish, ScrollCancel, ScrollSave]);

/// Bar height in logical px — a status strip, not a dialog.
const BAR_H: f32 = 44.;
/// Region frame stroke (logical px). Drawn OUTSIDE the selection rect.
const FRAME: f32 = 2.;
/// Preview panel width (logical px).
const PANEL_W: f32 = 264.;

/// Everything `launch` needs about the target output, in logical px
/// (the strip geometry is computed from the spec's output-local rect).
pub(crate) struct ScrollChrome {
    pub display_id: Option<DisplayId>,
    pub output_width: f32,
    pub output_height: f32,
}

pub(crate) struct ScrollBar {
    focus: FocusHandle,
    controls: Option<ScrollControls>,
    state: BarState,
    /// Ctrl+S was pressed: the finished canvas goes to the save flow
    /// instead of the clipboard.
    saving: bool,
    /// Sibling windows this chrome owns (frame strips + preview) —
    /// closed on every terminal path.
    siblings: Vec<AnyWindowHandle>,
    /// The preview panel's entity, for streaming canvas tails.
    preview: Option<gpui_kit::WeakEntity<PreviewPanel>>,
}

enum BarState {
    /// Engine thread is connecting / capturing the first frame.
    Starting,
    /// Scrolling; `stitched` rows captured beyond the initial viewport.
    Running { stitched: u32 },
    /// Finishing on request; the canvas is on its way.
    Finishing,
    /// A terminal event fired; the window is going away.
    Done,
}

/// Launch the full scroll-capture chrome and start the engine. Must be
/// called BEFORE the overlays unmap — closing every window would end
/// the app loop with the capture half-started.
pub(crate) fn launch(spec: ScrollSpec, chrome: ScrollChrome, cx: &mut App) -> anyhow::Result<()> {
    let options = StitchOptions::default();
    let mut siblings = Vec::new();

    // Region frame: four strips boxing the selection (strictly outside
    // the captured rect). All anchored LEFT|TOP; margins position them.
    for (i, (x, y, w, h)) in frame_strips(spec.rect).into_iter().enumerate() {
        let window_options = strip_options(
            &chrome,
            x,
            y,
            size(px(w), px(h)),
            &format!("shotori-scroll-frame-{i}"),
        );
        let handle = cx
            .open_window(window_options, |_, cx| {
                cx.new(|_| FrameStrip {
                    accent: theme::c().accent,
                })
            })
            .map_err(|e| anyhow::anyhow!("frame strip: {e}"))?;
        siblings.push(handle.into());
    }

    // Preview panel on the freer side of the selection.
    let side = if (spec.rect.x + spec.rect.width / 2) as f32 > chrome.output_width / 2. {
        Side::Left
    } else {
        Side::Right
    };
    // The panel entity is created up front so the bar can hold its
    // weak handle from birth (the open_window closure just hands the
    // pre-made entity to the new window).
    let panel: Entity<PreviewPanel> = cx.new(PreviewPanel::new);
    let weak_panel = panel.downgrade();
    let preview_handle = cx.open_window(preview_options(&chrome, side), |_, _| panel.clone());

    let bar_cell: std::rc::Rc<std::cell::RefCell<Option<WeakEntity<ScrollBar>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let _bar_handle = {
        let bounds = WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(chrome.output_width), px(BAR_H)),
        });
        let window_options = WindowOptions {
            app_id: Some(crate::APP_ID.into()),
            titlebar: None,
            window_background: WindowBackgroundAppearance::Opaque,
            focus: true,
            display_id: chrome.display_id,
            window_bounds: Some(bounds),
            #[cfg(target_os = "linux")]
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "shotori-scroll-bar".into(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                // Overlay that reserves no space: an exclusive zone would
                // resize the scrolled window mid-capture and wreck the
                // stitching.
                exclusive_zone: Some(px(-1.)),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            #[cfg(not(target_os = "linux"))]
            kind: WindowKind::PopUp,
            ..Default::default()
        };
        let cell = bar_cell.clone();
        let weak_panel = weak_panel.clone();
        cx.open_window(window_options, move |window, cx| {
            cx.new(|cx| {
                let bar = ScrollBar::new(spec, options, Some(weak_panel), window, cx);
                *cell.borrow_mut() = Some(cx.entity().downgrade());
                bar
            })
        })
    }?;

    if let Ok(handle) = &preview_handle {
        siblings.push((*handle).into());
    }
    if let Some(weak) = bar_cell.borrow().as_ref()
        && let Some(bar) = weak.upgrade()
    {
        // the bar closes the siblings on its terminal paths
        bar.update(cx, |bar, _| {
            bar.siblings = siblings;
        });
    }
    Ok(())
}

/// Which screen edge the preview panel docks to.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

/// The four frame strips as (x, y, w, h) in output-local logical px,
/// each strictly outside the captured rect.
fn frame_strips(rect: crate::model::session::ScrollRect) -> Vec<(f32, f32, f32, f32)> {
    let (x, y) = (rect.x as f32, rect.y as f32);
    let (w, h) = (rect.width as f32, rect.height as f32);
    vec![
        (x - FRAME, y - FRAME, w + 2. * FRAME, FRAME), // top
        (x - FRAME, y + h, w + 2. * FRAME, FRAME),     // bottom
        (x - FRAME, y, FRAME, h),                      // left
        (x + w, y, FRAME, h),                          // right
    ]
}

#[cfg(target_os = "linux")]
fn strip_options(
    chrome: &ScrollChrome,
    x: f32,
    y: f32,
    size: Size<Pixels>,
    namespace: &str,
) -> WindowOptions {
    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Opaque,
        focus: false,
        display_id: chrome.display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size,
        })),
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: namespace.into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::LEFT,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: KeyboardInteractivity::None,
            margin: Some((
                px(y),
                px(chrome.output_width - x - f32::from(size.width)),
                px(0.),
                px(x),
            )),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn preview_options(chrome: &ScrollChrome, side: Side) -> WindowOptions {
    let top = BAR_H + 10.;
    let bottom = 10.;
    let height = (chrome.output_height - top - bottom).max(120.);
    let (anchor, margin) = match side {
        Side::Right => (
            Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT,
            (px(top), px(0.), px(bottom), px(0.)),
        ),
        Side::Left => (
            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT,
            (px(top), px(0.), px(bottom), px(0.)),
        ),
    };
    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Opaque,
        focus: false,
        display_id: chrome.display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(PANEL_W), px(height)),
        })),
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-preview".into(),
            layer: Layer::Overlay,
            anchor,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: KeyboardInteractivity::None,
            margin: Some(margin),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// One solid frame strip.
struct FrameStrip {
    accent: u32,
}

impl Render for FrameStrip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgba(self.accent))
    }
}

/// The live preview: the stitched image's growing edge, streamed by the
/// engine as downscaled tails.
pub(crate) struct PreviewPanel {
    focus: FocusHandle,
    image: Option<Arc<RenderImage>>,
    label: String,
}

impl PreviewPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            image: None,
            label: "waiting for frames…".into(),
        }
    }

    fn on_preview(&mut self, width: u32, height: u32, rgba: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.image = Some(image_util::rgba_to_render_image(
            (*rgba).clone(),
            width,
            height,
        ));
        self.label = format!("{width}×{height}");
        cx.notify();
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Panel chrome: a dark card; the image scales to the panel width
        // and anchors to the bottom (the growing edge stays put).
        let mut card = div()
            .id("shotori-scroll-preview")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .bg(rgba(theme::c().toolbar_bg))
            .border_1()
            .border_color(rgba(theme::c().pin_border))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(theme::c().btn_text))
                    .child(format!("Long screenshot · {}", self.label)),
            )
            .child(div().flex_1());
        if let Some(image) = self.image.clone() {
            card = card.child(
                div()
                    .flex()
                    .justify_end()
                    .child(img(image).w_full().object_fit(ObjectFit::ScaleDown)),
            );
        }
        card
    }
}

impl ScrollBar {
    fn new(
        spec: ScrollSpec,
        options: StitchOptions,
        preview: Option<WeakEntity<PreviewPanel>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::ui::theme::follow_system(window.appearance());
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // A spawn failure (not a compositor failure — those arrive as
        // Failed events) still yields a live event stream carrying the
        // error, so the bar's lifecycle code stays single-shaped.
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
            state: BarState::Starting,
            saving: false,
            siblings: Vec::new(),
            preview,
        };

        // The event pump: engine → entity updates; ends with the channel
        // (the engine closes its sender on exit).
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            while let Ok(event) = events.recv().await {
                if weak
                    .update(cx, |bar, cx| bar.on_event(event, &handle, cx))
                    .is_err()
                {
                    break; // window gone; nothing left to update
                }
            }
        })
        .detach();
        this
    }

    fn close_siblings(&mut self, cx: &mut Context<Self>) {
        for handle in std::mem::take(&mut self.siblings) {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn on_event(&mut self, event: ScrollEvent, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        match event {
            ScrollEvent::Started { .. } => {
                self.state = BarState::Running { stitched: 0 };
                cx.notify();
            }
            ScrollEvent::Progress { stitched } => {
                if let BarState::Running { stitched: rows } = &mut self.state {
                    *rows = stitched;
                    cx.notify();
                }
            }
            ScrollEvent::Preview {
                width,
                height,
                rgba,
            } => {
                if let Some(preview) = self.preview.clone()
                    && let Some(panel) = preview.upgrade()
                {
                    panel.update(cx, |panel, cx| panel.on_preview(width, height, rgba, cx));
                }
            }
            ScrollEvent::Finished {
                width,
                height,
                rgba,
            } => {
                if matches!(self.state, BarState::Done) {
                    return;
                }
                self.state = BarState::Done;
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
    /// the bar) → notification → the windows close LAST, so the async
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
        self.close_siblings(cx);
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
        self.close_siblings(cx);
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
        self.state = BarState::Done;
        self.close_siblings(cx);
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.notify();
    }

    /// [Finish]: stop the engine now; whatever is stitched is the
    /// deliverable.
    fn finish(&mut self, _: &ScrollFinish, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls
            && !matches!(self.state, BarState::Done)
        {
            self.state = BarState::Finishing;
            controls.finish();
            cx.notify();
        }
    }

    /// [Cancel]: discard everything.
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
            self.state = BarState::Finishing;
            controls.finish();
            cx.notify();
        }
    }
}

impl Render for ScrollBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, running) = match &self.state {
            BarState::Starting => ("Starting scroll capture…".to_string(), false),
            BarState::Running { stitched } => {
                (format!("Scroll the content — {stitched} px captured"), true)
            }
            BarState::Finishing => ("Finishing…".to_string(), false),
            BarState::Done => ("Done".to_string(), false),
        };

        div()
            .id("shotori-scroll-bar")
            .key_context("ShotoriScroll")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .bg(rgba(theme::c().toolbar_bg))
            .border_b_1()
            .border_color(rgba(theme::c().pin_border))
            .on_action(cx.listener(Self::finish))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().size(px(8.)).rounded_full().bg(rgba(if running {
                        theme::c().accent
                    } else {
                        theme::c().pin_border
                    })))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgba(theme::c().btn_text))
                            .child(label),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(bar_button("scroll-finish", "Finish ⏎", ScrollFinish))
                    .child(bar_button("scroll-cancel", "Cancel Esc", ScrollCancel)),
            )
    }
}

fn bar_button<A: Action + Clone + 'static>(
    id: &'static str,
    label: &'static str,
    action: A,
) -> Button {
    Button::new(id)
        .accessibility_label(label)
        .px_3()
        .py_1()
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(rgba(theme::c().btn_text))
        .border_1()
        .border_color(rgba(theme::c().pin_border))
        .hover(|s| s.bg(rgba(theme::c().btn_hover_bg)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            window.dispatch_action(Box::new(action.clone()), cx);
        })
        .child(label)
}
