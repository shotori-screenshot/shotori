//! The preview panel: the deck window showing the growing long image
//! (a viewfinder on the sheet), the action bar above it and the
//! finish/cancel/save exits. The engine's camera, phase and axis all
//! render here as chips on the strip; the wheel can peek at history
//! and returns to the live edge by chip.

use super::*;

use std::sync::Arc;

use crate::model::scroll_sheet::{Axis, Phase};
use crate::platform::scroll_capture::{ScrollControls, ScrollEvent};
use crate::ui::image_util;
use crate::ui::theme;

pub(crate) struct PreviewPanel {
    focus: FocusHandle,
    controls: Option<ScrollControls>,
    state: PanelState,
    /// Ctrl+S was pressed: the finished canvas goes to the save flow
    /// instead of the clipboard.
    saving: bool,
    /// The region-frame window, closed on every terminal path.
    pub(super) sibling: Option<AnyWindowHandle>,
    image: Option<Arc<RenderImage>>,
    /// Every preview bitmap the window has been handed. gpui keeps a GPU
    /// texture per `RenderImage`; a 16 fps preview would otherwise leak
    /// one ~1 MB texture per frame (the "it freezes after a minute of
    /// scrolling" report). Retired ones are dropped during the next
    /// paint — `CanvasImages` is the crate's existing eviction dance.
    preview_images: image_util::CanvasImages,
    /// Textures to evict on the next paint (see above).
    retired: Vec<Arc<RenderImage>>,
    /// Viewport highlight in PREVIEW-row coordinates (already mapped by
    /// the engine); `None` when the panel is peeking at history, i.e.
    /// the viewport is not on the window it shows.
    viewport: Option<(u32, u32)>,
    preview_rows: u32,
    /// The preview bitmap's width in px (for the aspect-derived display
    /// height — see [`PREVIEW_IMG_W`]).
    preview_cols: u32,
    /// The engine's camera writes its preview height here (0 = none).
    preview_height: Arc<std::sync::atomic::AtomicU32>,
    /// The deck window's logical height — the preview well's visible
    /// area derives from it (the card is top-anchored, not full-height).
    output_h: f32,
    /// Physical capture size from `Started` (width, height).
    capture: Option<(u32, u32)>,
    /// Full canvas height from `Progress` — the header's "W × H".
    total_height: u32,
    /// The session's own reading of the last few frames (waiting /
    /// following / no overlap / stalled) — a refusal is a state here,
    /// never an error.
    phase: Phase,
    /// Sampling paused by the user (Space / the pause button).
    paused: bool,
    /// Current growth direction (confirmed by the engine).
    axis: Axis,
    /// The engine accepted a frame: the axis can no longer be swapped.
    axis_locked: bool,
    /// The wheel in the strip aimed the camera at history; the chip
    /// returns it to the live edge.
    peeking: bool,
}

#[derive(Clone, Copy, PartialEq)]
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
    /// `controls`/`events` arrive from `launch` (the engine starts
    /// before any window so either host — panel or chrome-free frame —
    /// can take the pair).
    pub(super) fn new(
        controls: ScrollControls,
        events: async_channel::Receiver<ScrollEvent>,
        output_h: f32,
        preview_height: Arc<std::sync::atomic::AtomicU32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::ui::theme::follow_system(window.appearance());
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let this = Self {
            focus: focus_handle,
            controls: Some(controls),
            state: PanelState::Starting,
            saving: false,
            sibling: None,
            image: None,
            preview_images: image_util::CanvasImages::default(),
            retired: Vec::new(),
            viewport: None,
            preview_rows: 0,
            preview_cols: 1,
            preview_height,
            output_h,
            capture: None,
            total_height: 0,
            phase: Phase::Waiting,
            paused: false,
            axis: Axis::Vertical,
            axis_locked: false,
            peeking: false,
        };
        this.publish_preview_height();

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

    /// Stop sampling and deliver the canvas (the deck's Copy button).
    fn request_finish(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls
            && !matches!(self.state, PanelState::Done)
        {
            self.state = PanelState::Finishing;
            controls.finish();
            cx.notify();
        }
    }

    /// Discard the session (the deck's Cancel button).
    fn request_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.take() {
            controls.cancel();
            cx.notify();
        }
    }

    /// The preview well's visible height and the strip's rendered height,
    /// from the same constants the renderer uses.
    fn preview_metrics(&self) -> (f32, f32) {
        let content_h = self.output_h.max(120.);
        let area_h = (content_h - TOOLBAR_H - TOOLBAR_GAP - 2. * PREVIEW_BORDER).max(60.);
        let img_h = (PREVIEW_IMG_W * self.preview_rows as f32 / self.preview_cols.max(1) as f32)
            .clamp(1., 8000.);
        (area_h, img_h)
    }

    /// Tell the engine how tall the strip is. The engine sizes the slice
    /// from it (same `k`, same fit), which is what keeps one sheet row a
    /// constant number of screen pixels — the old preview resized its own
    /// image as the sheet grew and moved at a rate nobody could predict.
    fn publish_preview_height(&self) {
        let (area_h, _) = self.preview_metrics();
        self.preview_height.store(
            area_h.round().max(0.) as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Wheel over the strip: aim the engine's camera at history instead
    /// of the live edge. The panel never scrolls a bitmap — the strip is
    /// a viewfinder, so a step is a new slice from the engine and the
    /// "Back to live" chip restores following.
    fn scroll_preview(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 40.,
        };
        if lines == 0. || self.image.is_none() {
            return;
        }
        let step = (lines * 3.).round() as i32;
        if step == 0 {
            return;
        }
        if let Some(controls) = &self.controls {
            self.peeking = true;
            controls.peek(Some(step));
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The contextual chip that appears over the preview only when the
    /// session is NOT simply following: waiting, no overlap, stalled,
    /// paused or wrapping up. A healthy capture needs no word — the
    /// moving strip says it.
    fn state_chip(&self) -> Option<String> {
        if self.paused {
            return Some(crate::i18n::t!("scroll.status.paused").to_string());
        }
        match self.state {
            PanelState::Starting => None,
            PanelState::Finishing | PanelState::Done => {
                Some(crate::i18n::t!("scroll.status.finishing").to_string())
            }
            PanelState::Running => match self.phase {
                Phase::Waiting => Some(crate::i18n::t!("scroll.status.waiting").to_string()),
                Phase::NoOverlap => Some(crate::i18n::t!("scroll.status.no_overlap").to_string()),
                Phase::Unstable => Some(crate::i18n::t!("scroll.status.unstable").to_string()),
                Phase::Stalled => Some(crate::i18n::t!("scroll.status.stalled").to_string()),
                Phase::Following => None,
            },
        }
    }

    /// Space / the pause button: freeze sampling without ending the
    /// session. The engine keeps the canvas; resuming re-anchors on
    /// whatever the page shows then.
    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            self.paused = !self.paused;
            controls.set_paused(self.paused);
            cx.notify();
        }
    }

    /// Swap vertical/horizontal growth. The engine confirms with a
    /// `ScrollEvent::Axis` (and reports `locked` once it is too late).
    fn toggle_axis(&mut self, cx: &mut Context<Self>) {
        if self.axis_locked {
            return;
        }
        let next = match self.axis {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        };
        if let Some(controls) = &self.controls {
            controls.set_axis(next);
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
            ScrollEvent::Axis { axis, locked } => {
                self.axis = axis;
                self.axis_locked = locked;
                self.peeking = false;
                self.publish_preview_height();
                // The initial long dimension follows the axis: the
                // region's height for vertical, its width for
                // horizontal. Progress overwrites this on the first
                // accepted frame (after which the axis is locked).
                if let Some((w, h)) = self.capture {
                    self.total_height = match axis {
                        Axis::Vertical => h,
                        Axis::Horizontal => w,
                    };
                }
                cx.notify();
            }
            ScrollEvent::Started { width, height } => {
                self.state = PanelState::Running;
                self.capture = Some((width, height));
                // Vertical is the initial direction; the Axis event
                // that follows corrects the long dimension.
                self.total_height = height;
                cx.notify();
            }
            ScrollEvent::Progress { total, .. } => {
                // The header shows the growing canvas size; `extent`
                // stays available for a future progress bar.
                self.total_height = self.total_height.max(total);
                cx.notify();
            }
            ScrollEvent::Phase(phase) => {
                self.phase = phase;
                cx.notify();
            }
            ScrollEvent::Viewport { top, height } => {
                // The pixel-free twin of Preview's viewport fields: the
                // highlight follows frame drags without image resend.
                self.viewport = Some((top, height));
                cx.notify();
            }
            ScrollEvent::Preview {
                width,
                height,
                rgba,
                viewport_top,
                viewport_height,
            } => {
                // A new bitmap every frame buys a GPU texture every
                // frame; retire the one it replaces (painted-dropped,
                // see `retired`).
                let image = image_util::rgba_to_render_image((*rgba).clone(), width, height);
                self.retired
                    .extend(self.preview_images.replace(vec![image.clone()]));
                self.image = Some(image);
                self.preview_rows = height;
                self.preview_cols = width.max(1);
                // The engine mapped the span into PREVIEW rows already;
                // `None` = the viewport is not on the window it sent
                // (the camera is looking at history).
                self.viewport = viewport_top.map(|top| (top, viewport_height));
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
                let sibling = self.sibling.take();
                if self.saving {
                    scroll_save_exit(&rgba, width, height, *window, sibling, cx);
                } else {
                    scroll_copy_exit(&rgba, width, height, *window, sibling, cx);
                }
            }
            ScrollEvent::Failed { reason, partial } => {
                if matches!(self.state, PanelState::Done) {
                    return;
                }
                self.state = PanelState::Done;
                self.controls = None;
                cx.notify();
                // Something was captured before the failure: deliver it
                // instead of discarding — a broken tail must not eat the
                // good rows above it.
                let sibling = self.sibling.take();
                if let Some((w, h, rgba)) = partial {
                    crate::notify::send(
                        crate::i18n::t!("scroll.notify.stopped_early.title").as_ref(),
                        crate::i18n::t!(
                            "scroll.notify.stopped_early.partial",
                            reason = reason.clone()
                        )
                        .as_ref(),
                    );
                    if self.saving {
                        scroll_save_exit(&rgba, w, h, *window, sibling, cx);
                    } else {
                        scroll_copy_exit(&rgba, w, h, *window, sibling, cx);
                    }
                } else {
                    eprintln!("[shotori] scroll failed: {reason}");
                    crate::notify::send(
                        crate::i18n::t!("scroll.notify.failed.title").as_ref(),
                        &reason,
                    );
                    self.close(window, cx);
                }
            }
            ScrollEvent::Cancelled => {
                println!("[shotori] scroll cancelled");
                self.close(window, cx);
            }
        }
    }

    // The default exit and the Ctrl+S exit run through the shared
    // `scroll_copy_exit`/`scroll_save_exit` (see `on_event`) — the
    // panel is one of two possible hosts, the chrome-free frame is
    // the other.
    fn close(&mut self, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        self.state = PanelState::Done;
        self.close_sibling(cx);
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.notify();
    }

    /// Enter / Ctrl+C / [Copy]: stop the engine now; whatever is
    /// folded image is the deliverable.
    fn finish(&mut self, _: &ScrollFinish, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_finish(cx);
    }

    /// Esc / [Cancel]: discard everything.
    fn cancel(&mut self, _: &ScrollCancel, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_cancel(cx);
    }

    /// Space / [Pause]: freeze or resume sampling.
    fn pause(&mut self, _: &ScrollPause, _window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_pause(cx);
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

/// The copy exit, shared by every scroll-session host (panel and
/// chrome-free frame): PNG → clipboard (background, never blocking the
/// UI) → notification → the host window closes LAST, so the async copy
/// always completes while the app loop still runs.
pub(super) fn scroll_copy_exit(
    rgba: &Arc<Vec<u8>>,
    width: u32,
    height: u32,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    let rgba = rgba.clone();
    if let Some(sibling) = sibling {
        let _ = sibling.update(cx, |_, window, _| window.remove_window());
    }
    cx.spawn(async move |cx| {
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
                    crate::i18n::t!("scroll.notify.copy_failed.title").as_ref(),
                    crate::i18n::t!("scroll.notify.copy_failed.body").as_ref(),
                );
            }
        }
        let _ = host.update(cx, |_, window, _| window.remove_window());
    })
    .detach();
}

/// The save exit, shared by every scroll-session host: stash, unmap,
/// quit from a timer so the connection flushes; the portal dialog then
/// opens from `save_dialog::complete_pending` in main.
pub(super) fn scroll_save_exit(
    rgba: &Arc<Vec<u8>>,
    width: u32,
    height: u32,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    crate::save_dialog::stash(width, height, rgba.to_vec());
    cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
    if let Some(sibling) = sibling {
        let _ = sibling.update(cx, |_, window, _| window.remove_window());
    }
    let _ = host.update(cx, |_, window, _| window.remove_window());
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Retire the previous frame's texture. gpui keeps a texture per
        // `RenderImage` per window; at capture cadence an un-evicted
        // preview leaks one bitmap-sized texture every ~60 ms, which is
        // what froze (then killed) long sessions.
        for image in self.retired.drain(..) {
            if let Err(error) = window.drop_image(image) {
                eprintln!("[shotori] preview cleanup failed: {error}");
            }
        }
        let size = self.capture.map(|(w, h)| match self.axis {
            Axis::Vertical => format!("{w} × {}", self.total_height.max(h)),
            Axis::Horizontal => format!("{} × {h}", self.total_height.max(w)),
        });
        let state = self.state_chip();

        // ── action bar: the selection toolbar's vocabulary, floating
        // over the capture instead of becoming a panel header. ────────
        let pause_label = if self.paused {
            crate::i18n::t!("scroll.action.resume").to_string()
        } else {
            crate::i18n::t!("scroll.action.pause").to_string()
        };
        let pause_icon = if self.paused {
            IconName::Play
        } else {
            IconName::Pause
        };
        let axis_label = match self.axis {
            Axis::Vertical => crate::i18n::t!("scroll.action.axis_horizontal").to_string(),
            Axis::Horizontal => crate::i18n::t!("scroll.action.axis_vertical").to_string(),
        };
        let axis_tip = if self.axis_locked {
            crate::i18n::t!("scroll.action.axis_locked").to_string()
        } else {
            axis_label.clone()
        };
        let copy_label = crate::i18n::t!("common.copy").to_string();
        let save_label = crate::i18n::t!("common.save").to_string();
        let cancel_label = crate::i18n::t!("scroll.action.cancel").to_string();
        let bar = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(TOOLBAR_H))
            .px(px(6.))
            .rounded(px(10.))
            .bg(rgba(theme::c().toolbar_bg))
            .border_1()
            .border_color(rgba(theme::c().toolbar_border))
            .shadow_md()
            .child(deck_button(
                "scroll-deck-pause",
                pause_label.clone(),
                format!("{pause_label} · Space"),
                DeckIcon::Kit(pause_icon),
                true,
                cx.listener(|this, _, _, cx| this.toggle_pause(cx)),
            ))
            .child(deck_button(
                "scroll-deck-axis",
                axis_label.clone(),
                axis_tip,
                DeckIcon::Own("icons/axis.svg"),
                !self.axis_locked,
                cx.listener(|this, _, _, cx| this.toggle_axis(cx)),
            ))
            .child(
                div()
                    .w(px(1.))
                    .h(px(18.))
                    .bg(rgba(theme::c().toolbar_border)),
            )
            .child(deck_button(
                "scroll-deck-copy",
                copy_label.clone(),
                format!("{copy_label} · Enter"),
                DeckIcon::Kit(IconName::Copy),
                true,
                cx.listener(|this, _, _, cx| this.request_finish(cx)),
            ))
            .child(deck_button(
                "scroll-deck-save",
                save_label.clone(),
                format!("{save_label} · Ctrl+S"),
                DeckIcon::Kit(IconName::Save),
                true,
                cx.listener(|this, _, _, cx| this.request_save(cx)),
            ))
            .child(deck_button(
                "scroll-deck-cancel",
                cancel_label.clone(),
                format!("{cancel_label} · Esc"),
                DeckIcon::Kit(IconName::X),
                true,
                cx.listener(|this, _, _, cx| this.request_cancel(cx)),
            ));
        let toolbar = div().w_full().flex().justify_center().child(bar);

        // ── preview strip: the long image itself, no extra panel chrome.
        let (area_h, _) = self.preview_metrics();
        let mut preview = div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .rounded(px(10.))
            .bg(rgba(theme::c().toolbar_hover))
            .border_1()
            .border_color(rgba(alpha(theme::c().toolbar_border, 0xD0)))
            .on_scroll_wheel(cx.listener(Self::scroll_preview));

        if let Some(image) = self.image.clone() {
            let rows = self.preview_rows.max(1) as f32;
            let (_, img_h) = self.preview_metrics();
            // The engine cut the window to this well's height, so the
            // strip is painted 1:1 and never scaled to fit: at most a
            // rounding pixel of slack, centered.
            let strip_h = img_h.min(area_h);
            let y_off = (area_h - strip_h) / 2.;
            // Suppress the accent wash when the viewport IS the whole
            // strip (the first screen): tinting every pixel adds nothing
            // but a color cast.
            let (wash, marker) = match self.viewport {
                Some((v_top, v_h)) => {
                    // Fractions of the STRIP (not the well): the engine's
                    // mapping is already in preview rows. Cap below 1.0 so
                    // the height clamp's min ≤ max always holds (an
                    // uncapped pair once panicked the render pass — the
                    // "scrolling crashes" bug).
                    let top_frac = (v_top as f32 / rows).min(0.99);
                    let h_frac = (v_h as f32 / rows).clamp(0.01, 1. - top_frac);
                    let wash = if top_frac > 0.01 || h_frac < 0.98 {
                        alpha(theme::c().accent, 0x12)
                    } else {
                        0
                    };
                    (wash, Some((top_frac, h_frac)))
                }
                // Peeking at history: no "you are here" band to draw.
                None => (alpha(theme::c().hint_text, 0x08), None),
            };
            let mut strip = div()
                .absolute()
                .left(px(PREVIEW_GUTTER))
                .top(px(y_off))
                .w(px(PREVIEW_IMG_W))
                .child(img(image).w_full().h(px(strip_h)));
            if let Some((top_frac, h_frac)) = marker {
                // The viewport marker: a translucent accent wash plus a
                // crisp accent rule. The old 1px border read as a random
                // rectangle; the wash makes "this is what the frame
                // currently shows" legible at a glance, including at a
                // 1–2 px band.
                strip = strip.child(
                    div()
                        .absolute()
                        .left_0()
                        .w_full()
                        .top(relative(top_frac))
                        .h(relative(h_frac))
                        .bg(rgba(wash))
                        .border_1()
                        .border_color(rgba(theme::c().accent))
                        .rounded_sm(),
                );
            }
            preview = preview.child(strip);
        } else {
            let hint = if self.paused {
                crate::i18n::t!("scroll.status.paused")
            } else {
                crate::i18n::t!("scroll.hint.start")
            };
            preview = preview.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(
                        svg()
                            .path(IconName::ArrowDown.path())
                            .size(px(20.))
                            .text_color(rgba(theme::c().hint_text)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(theme::c().toolbar_text))
                            .child(hint),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgba(theme::c().hint_text))
                            .child(crate::i18n::t!("scroll.hint.shortcuts").to_string()),
                    ),
            );
        }

        // Chips float over the image, exactly like the selection's size
        // label: status on the left (only when not simply following the
        // scroll), live size on the right.
        if let Some(state) = state {
            preview = preview.child(preview_chip(state).left(px(8.)));
        }
        if let Some(size) = size {
            preview = preview.child(preview_chip(size).right(px(8.)));
        }
        if self.peeking {
            preview = preview.child(
                div()
                    .id("scroll-back-to-live")
                    .absolute()
                    .bottom(px(8.))
                    .right(px(8.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .bg(rgba(theme::c().chip_bg))
                    .border_1()
                    .border_color(rgba(alpha(theme::c().toolbar_border, 0xB0)))
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.peeking = false;
                        if let Some(controls) = &this.controls {
                            controls.peek(None);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgba(theme::c().hint_text))
                            .child(crate::i18n::t!("scroll.action.follow").to_string()),
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
            .on_action(cx.listener(Self::finish))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::pause))
            .child(toolbar)
            .child(div().h(px(TOOLBAR_GAP)))
            .child(preview)
    }
}

/// A small floating label over the preview, in the selection size-chip
/// language (`chip_bg` + hint text). Absolute top is set by the caller.
fn preview_chip(text: String) -> Div {
    div()
        .absolute()
        .top(px(8.))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(5.))
        .bg(rgba(theme::c().chip_bg))
        .border_1()
        .border_color(rgba(alpha(theme::c().toolbar_border, 0xB0)))
        .text_size(px(10.))
        .text_color(rgba(theme::c().hint_text))
        .child(text)
}

/// Neutral icon tiles for the floating toolbar. Disabled tiles fade
/// but keep their tooltip explaining why the action is unavailable.
fn deck_button(
    id: &'static str,
    label: String,
    tooltip: String,
    icon: DeckIcon,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(7.))
        .aria_label(label)
        .tooltip(move |_, cx| {
            cx.new(|_| crate::ui::toolbar::ToolbarTooltip(tooltip.clone().into()))
                .into()
        });
    let button = if enabled {
        button
            .hover(|style| style.bg(rgba(theme::c().toolbar_hover)))
            .cursor(CursorStyle::PointingHand)
            .on_click(on_click)
    } else {
        button.opacity(0.45)
    };
    button
        .child(
            icon.svg()
                .size(px(16.))
                .text_color(rgba(theme::c().toolbar_text)),
        )
        .into_any_element()
}

/// Icon source for a HUD tile: a gpui-kit asset or one of Shotori's own
/// SVGs (the axis swap glyph is ours — Lucide has no neat twin-arrow).
#[derive(Clone, Copy)]
enum DeckIcon {
    Kit(IconName),
    Own(&'static str),
}

impl DeckIcon {
    fn svg(self) -> Svg {
        match self {
            Self::Kit(name) => svg().path(name.path()),
            Self::Own(path) => svg().path(path),
        }
    }
}

/// `0xRRGGBBAA` with a replacement alpha — theme colors are opaque.
pub(super) fn alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xFFFFFF00) | u32::from(alpha)
}
