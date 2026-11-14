//! The preview panel: floating action toolbar, dedicated non-occluding
//! status & dimensions bar, top-aligned aspect-preserving preview well,
//! and proportional global scrollbar/minimap rail.

use std::sync::Arc;

use super::exits::{ExitTarget, PinContext, WARNING_AMBER, dispatch_exit, is_warning_phase};
use super::frame::FrameView;
use super::*;
use crate::model::scroll_sheet::{Axis, Phase, PreviewGeometry};
use crate::platform::scroll_capture::{ScrollControls, ScrollEvent};
use crate::ui::image_util;
use crate::ui::theme;
use crate::ui::theme::alpha;

pub(crate) struct PreviewPanel {
    focus: FocusHandle,
    controls: Option<ScrollControls>,
    state: PanelState,
    exit: ExitTarget,
    pin_ctx: PinContext,
    /// The region-frame window, closed on every terminal path.
    pub(super) sibling: Option<AnyWindowHandle>,
    /// Weak handle to `FrameView` so live `Phase`/`paused` changes tint
    /// the peripheral region outline in real time.
    pub(super) frame_view: Option<gpui_kit::WeakEntity<FrameView>>,
    image: Option<Arc<RenderImage>>,
    /// Every preview bitmap the window has been handed; retired textures
    /// are evicted on the next paint so long sessions never leak VRAM.
    preview_images: image_util::CanvasImages,
    retired: Vec<Arc<RenderImage>>,
    /// Viewport highlight in preview pixels along the growth axis; `None`
    /// when the viewport lies outside the currently shown window.
    viewport: Option<(u32, u32)>,
    preview_rows: u32,
    preview_cols: u32,
    /// Global sheet metrics along the scroll axis (physical sheet rows)
    /// for the proportional scrollbar/minimap rail.
    sheet_len: u32,
    window_top: u32,
    window_rows: u32,
    sheet_viewport_top: u32,
    sheet_viewport_len: u32,
    preview_geom: PreviewGeometry,
    panel_scale: f32,
    panel_w: f32,
    output_h: f32,
    capture: Option<(u32, u32)>,
    total_height: u32,
    phase: Phase,
    paused: bool,
    axis: Axis,
    axis_locked: bool,
    peeking: bool,
    rail_drag: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelState {
    Starting,
    Running,
    Finishing,
    Done,
}

/// Aspect-preserving strip placement `(left, top, width, height)` inside
/// an inner preview box `(well_w, well_h)`.
///
/// - Vertical captures anchor at `top = 0` so newly captured rows unroll
///   naturally downward instead of floating in the middle of an empty box;
///   narrow-tall captures (`viewport_h > window_rows`) scale BOTH width
///   and height uniformly instead of clamping height alone (preventing
///   vertical squash) and center horizontally.
/// - Horizontal captures anchor at `left = 0` (unrolling rightward) and
///   fit uniformly within `(well_w, well_h)`, centered vertically.
fn fit_strip_box(
    well_w: f32,
    well_h: f32,
    img_cols: u32,
    img_rows: u32,
    axis: Axis,
) -> (f32, f32, f32, f32) {
    let cols = img_cols.max(1) as f32;
    let rows = img_rows.max(1) as f32;
    let well_w = well_w.max(1.0);
    let well_h = well_h.max(1.0);
    match axis {
        Axis::Vertical => {
            let nat_h = well_w * rows / cols;
            if nat_h <= well_h {
                (0.0, 0.0, well_w, nat_h.max(1.0))
            } else {
                let scale = well_h / nat_h;
                let w = (well_w * scale).clamp(1.0, well_w);
                ((well_w - w) * 0.5, 0.0, w, well_h)
            }
        }
        Axis::Horizontal => {
            let cross_cap = well_h.min(well_w);
            let scale = (well_w / cols).min(cross_cap / rows).max(0.001);
            let w = (cols * scale).clamp(1.0, well_w);
            let h = (rows * scale).clamp(1.0, well_h);
            (0.0, (well_h - h) * 0.5, w, h)
        }
    }
}

impl PreviewPanel {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        controls: ScrollControls,
        events: async_channel::Receiver<ScrollEvent>,
        panel_w: f32,
        output_h: f32,
        panel_scale: f32,
        preview_geom: PreviewGeometry,
        pin_ctx: PinContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let this = Self {
            focus: focus_handle,
            controls: Some(controls),
            state: PanelState::Starting,
            exit: ExitTarget::Copy,
            pin_ctx,
            sibling: None,
            frame_view: None,
            image: None,
            preview_images: image_util::CanvasImages::default(),
            retired: Vec::new(),
            viewport: None,
            preview_rows: 0,
            preview_cols: 1,
            sheet_len: 0,
            window_top: 0,
            window_rows: 1,
            sheet_viewport_top: 0,
            sheet_viewport_len: 1,
            preview_geom,
            panel_scale: panel_scale.max(1.0),
            panel_w,
            output_h,
            capture: None,
            total_height: 0,
            phase: Phase::Waiting,
            paused: false,
            axis: Axis::Vertical,
            axis_locked: false,
            peeking: false,
            rail_drag: false,
        };
        this.publish_preview_geometry();

        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            while let Ok(event) = events.recv().await {
                if weak
                    .update(cx, |panel, cx| panel.on_event(event, &handle, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this
    }

    pub(crate) fn controls(&self) -> ScrollControls {
        self.controls.clone().unwrap_or_else(ScrollControls::dead)
    }

    pub(crate) fn request_save(&mut self, cx: &mut Context<Self>) {
        self.request_exit(ExitTarget::Save, cx);
    }

    pub(crate) fn request_pin(&mut self, cx: &mut Context<Self>) {
        self.request_exit(ExitTarget::Pin, cx);
    }

    fn request_exit(&mut self, target: ExitTarget, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls
            && !matches!(self.state, PanelState::Done)
        {
            self.exit = target;
            self.state = PanelState::Finishing;
            controls.finish();
            cx.notify();
        }
    }

    fn request_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.take() {
            controls.cancel();
            cx.notify();
        }
    }

    /// Outer well height (`area_h`) and inner image box `(well_w, well_h)`
    /// in logical px, reserving [`RAIL_SLOT_W`] on the right for the
    /// global scrollbar rail.
    fn well_box(&self) -> (f32, f32, f32) {
        let content_h = self.output_h.max(160.);
        let chrome_h = TOOLBAR_H + INFO_BAR_H + 2. * TOOLBAR_GAP + 2. * PREVIEW_BORDER;
        let area_h = (content_h - chrome_h).max(60.);
        let well_w =
            (self.panel_w - 2. * PREVIEW_BORDER - 2. * PREVIEW_GUTTER - RAIL_SLOT_W).max(80.);
        let well_h = (area_h - 2. * PREVIEW_GUTTER).max(40.);
        (area_h, well_w, well_h)
    }

    /// Top Y coordinate and height of the scrollbar rail track in
    /// window-local logical pixels (same geometry source for paint and
    /// pointer scrubbing).
    fn rail_track_y(&self) -> (f32, f32) {
        let (_, _, well_h) = self.well_box();
        let well_top = TOOLBAR_H + INFO_BAR_H + 2. * TOOLBAR_GAP + PREVIEW_BORDER + PREVIEW_GUTTER;
        (well_top, well_h)
    }

    /// Publish logical `(well_along, well_cross)` and physical
    /// `sample_cross` so the engine's window sizing and area-box filter
    /// match both adaptive panel widths and growth orientation.
    fn publish_preview_geometry(&self) {
        let (_, well_w, well_h) = self.well_box();
        let (along_f, cross_f) = match self.axis {
            Axis::Vertical => (well_h, well_w),
            Axis::Horizontal => (well_w, well_h.min(well_w)),
        };
        let along = along_f.round().max(1.) as u32;
        let cross = cross_f.round().max(1.) as u32;
        let sample_w = PreviewGeometry::sample_width_for(cross_f, self.panel_scale);
        self.preview_geom.set_box(along, cross, sample_w);
    }

    fn sync_frame_status(&self, cx: &mut Context<Self>) {
        if let Some(frame) = self.frame_view.as_ref().and_then(|w| w.upgrade()) {
            let phase = self.phase;
            let paused = self.paused;
            frame.update(cx, |f, cx| f.set_status(phase, paused, cx));
        }
    }

    fn scroll_preview(
        &mut self,
        event: &gpui_kit::ScrollEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => {
                if lines.y != 0. {
                    lines.y
                } else {
                    lines.x
                }
            }
            ScrollDelta::Pixels(pixels) => {
                let py = f32::from(pixels.y);
                let px_ = f32::from(pixels.x);
                (if py.abs() >= px_.abs() { py } else { px_ }) / 40.
            }
        };
        if lines == 0. || self.image.is_none() {
            return;
        }
        let step = (lines * 18.).round() as i32;
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

    /// Scrub the global sheet window from a window-local pointer Y along
    /// the scrollbar rail.
    fn scrub_rail_at(&mut self, window_y: f32, cx: &mut Context<Self>) {
        if self.sheet_len == 0 || self.window_rows >= self.sheet_len {
            return;
        }
        let (track_top, track_h) = self.rail_track_y();
        let win_frac = (self.window_rows as f32 / self.sheet_len as f32).clamp(0.04, 1.0);
        let thumb_h = (track_h * win_frac).clamp(12.0, track_h);
        let travel = (track_h - thumb_h).max(1.0);
        let ratio = ((window_y - track_top - thumb_h * 0.5) / travel).clamp(0.0, 1.0);
        if let Some(controls) = &self.controls {
            self.peeking = true;
            controls.peek_ratio(ratio);
            cx.notify();
        }
    }

    fn status_label(&self) -> (String, bool) {
        if self.paused {
            return (crate::i18n::t!("scroll.status.paused").to_string(), false);
        }
        match self.state {
            PanelState::Starting => (crate::i18n::t!("scroll.status.waiting").to_string(), false),
            PanelState::Finishing | PanelState::Done => (
                crate::i18n::t!("scroll.status.finishing").to_string(),
                false,
            ),
            PanelState::Running => {
                let warn = is_warning_phase(self.phase);
                let text = match self.phase {
                    Phase::Following => crate::i18n::t!("scroll.status.following").to_string(),
                    Phase::Waiting => crate::i18n::t!("scroll.status.waiting").to_string(),
                    Phase::NoOverlap => crate::i18n::t!("scroll.status.no_overlap").to_string(),
                    Phase::Unstable => crate::i18n::t!("scroll.status.unstable").to_string(),
                    Phase::Stalled => crate::i18n::t!("scroll.status.stalled").to_string(),
                    Phase::MaxLength => crate::i18n::t!("scroll.status.max_length").to_string(),
                    Phase::LayoutShift => crate::i18n::t!("scroll.status.layout_shift").to_string(),
                };
                (text, warn)
            }
        }
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            self.paused = !self.paused;
            controls.set_paused(self.paused);
            self.sync_frame_status(cx);
            cx.notify();
        }
    }

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
            drop(handle.update(cx, |_, window, _| window.remove_window()));
        }
    }

    fn on_event(&mut self, event: ScrollEvent, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        match event {
            ScrollEvent::Axis { axis, locked } => {
                self.axis = axis;
                self.axis_locked = locked;
                self.peeking = false;
                self.publish_preview_geometry();
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
                self.total_height = height;
                cx.notify();
            }
            ScrollEvent::Progress { total, .. } => {
                self.total_height = self.total_height.max(total);
                cx.notify();
            }
            ScrollEvent::Phase(phase) => {
                self.phase = phase;
                self.sync_frame_status(cx);
                cx.notify();
            }
            ScrollEvent::Viewport {
                top,
                height,
                sheet_len,
                window_top,
                window_rows,
                sheet_viewport_top,
                sheet_viewport_len,
                peeking,
            } => {
                self.viewport = Some((top, height));
                self.sheet_len = sheet_len;
                self.window_top = window_top;
                self.window_rows = window_rows.max(1);
                self.sheet_viewport_top = sheet_viewport_top;
                self.sheet_viewport_len = sheet_viewport_len.max(1);
                self.peeking = peeking;
                cx.notify();
            }
            ScrollEvent::Preview {
                width,
                height,
                rgba,
                viewport_top,
                viewport_height,
                sheet_len,
                window_top,
                window_rows,
                sheet_viewport_top,
                sheet_viewport_len,
                peeking,
            } => {
                let image = image_util::rgba_to_render_image((*rgba).clone(), width, height);
                self.retired
                    .extend(self.preview_images.replace(vec![image.clone()]));
                self.image = Some(image);
                self.preview_rows = height.max(1);
                self.preview_cols = width.max(1);
                self.viewport = viewport_top.map(|top| (top, viewport_height));
                self.sheet_len = sheet_len;
                self.window_top = window_top;
                self.window_rows = window_rows.max(1);
                self.sheet_viewport_top = sheet_viewport_top;
                self.sheet_viewport_len = sheet_viewport_len.max(1);
                self.peeking = peeking;
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
                self.controls = None;
                cx.notify();
                let sibling = self.sibling.take();
                dispatch_exit(
                    self.exit,
                    (width, height, &rgba),
                    &self.pin_ctx,
                    *window,
                    sibling,
                    cx,
                );
            }
            ScrollEvent::Failed { reason, partial } => {
                if matches!(self.state, PanelState::Done) {
                    return;
                }
                self.state = PanelState::Done;
                self.controls = None;
                cx.notify();
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
                    dispatch_exit(
                        self.exit,
                        (w, h, &rgba),
                        &self.pin_ctx,
                        *window,
                        sibling,
                        cx,
                    );
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

    fn close(&mut self, window: &AnyWindowHandle, cx: &mut Context<Self>) {
        self.state = PanelState::Done;
        self.close_sibling(cx);
        drop(window.update(cx, |_, window, _| window.remove_window()));
        cx.notify();
    }

    fn finish(&mut self, _: &ScrollFinish, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_exit(ExitTarget::Copy, cx);
    }

    fn cancel(&mut self, _: &ScrollCancel, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_cancel(cx);
    }

    fn pause(&mut self, _: &ScrollPause, _window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_pause(cx);
    }

    fn save(&mut self, _: &ScrollSave, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_exit(ExitTarget::Save, cx);
    }

    fn pin(&mut self, _: &ScrollPin, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_exit(ExitTarget::Pin, cx);
    }

    fn render_info_bar(&self, size_label: Option<String>, cx: &mut Context<Self>) -> Div {
        let (status_text, warn) = self.status_label();
        let dot_color = if self.paused {
            alpha(theme::c().hint_text, 0xCC)
        } else if warn {
            WARNING_AMBER
        } else {
            theme::c().accent
        };
        let border_col = if warn {
            alpha(WARNING_AMBER, 0x70)
        } else {
            alpha(theme::c().toolbar_border, 0xD0)
        };
        let bg_col = if warn {
            alpha(WARNING_AMBER, 0x1E)
        } else {
            theme::c().toolbar_bg
        };
        let status_fg = if warn {
            WARNING_AMBER
        } else if self.paused {
            theme::c().hint_text
        } else {
            theme::c().toolbar_text
        };

        let mut right_cluster = div().flex().items_center().gap(px(4.)).flex_shrink_0();
        if self.peeking {
            right_cluster = right_cluster.child(
                div()
                    .id("scroll-back-to-live")
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(rgba(alpha(theme::c().accent, 0x22)))
                    .border_1()
                    .border_color(rgba(alpha(theme::c().accent, 0x66)))
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
                            .text_color(rgba(theme::c().toolbar_text))
                            .child(crate::i18n::t!("scroll.action.follow").to_string()),
                    ),
            );
        }
        if let Some(size) = size_label {
            right_cluster = right_cluster.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgba(theme::c().hint_text))
                    .child(size),
            );
        }

        div()
            .w_full()
            .h(px(INFO_BAR_H))
            .px(px(7.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .rounded(px(7.))
            .bg(rgba(bg_col))
            .border_1()
            .border_color(rgba(border_col))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(rgba(dot_color)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgba(status_fg))
                            .truncate()
                            .child(status_text),
                    ),
            )
            .child(right_cluster)
    }

    fn render_scrollbar_rail(&self, well_h: f32, cx: &mut Context<Self>) -> AnyElement {
        let total = self.sheet_len.max(1) as f32;
        let win_top_frac = (self.window_top as f32 / total).clamp(0.0, 0.98);
        let win_len_frac = (self.window_rows as f32 / total).clamp(0.02, 1.0 - win_top_frac);
        let thumb_h = (well_h * win_len_frac).clamp(12.0, well_h.max(12.0));
        let thumb_top = if self.sheet_len > self.window_rows {
            let max_top = (self.sheet_len - self.window_rows) as f32;
            (self.window_top as f32 / max_top).clamp(0.0, 1.0) * (well_h - thumb_h).max(0.0)
        } else {
            0.0
        };

        let live_top_frac = (self.sheet_viewport_top as f32 / total).clamp(0.0, 0.99);
        let live_len_frac =
            (self.sheet_viewport_len as f32 / total).clamp(0.01, 1.0 - live_top_frac);
        let live_top = well_h * live_top_frac;
        let live_h = (well_h * live_len_frac).clamp(3.0, well_h.max(3.0));

        let scrollable = self.sheet_len > self.window_rows;
        let thumb_color = if self.peeking || self.rail_drag {
            theme::c().accent
        } else {
            alpha(theme::c().accent, 0xA8)
        };
        let scrub_tip = crate::i18n::t!("scroll.action.scrub").to_string();

        let mut rail = div()
            .id("scroll-preview-rail")
            .absolute()
            .top(px(PREVIEW_GUTTER))
            .right(px(2.))
            .w(px(RAIL_SLOT_W - 2.))
            .h(px(well_h))
            .flex()
            .justify_center()
            .tooltip(move |_, cx| {
                cx.new(|_| crate::ui::toolbar::ToolbarTooltip(scrub_tip.clone().into()))
                    .into()
            });

        if scrollable {
            rail = rail.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.rail_drag = true;
                    this.scrub_rail_at(f32::from(event.position.y), cx);
                }),
            );
        }

        rail.child(
            div()
                .relative()
                .w(px(4.))
                .h_full()
                .rounded_full()
                .bg(rgba(alpha(theme::c().toolbar_border, 0x55)))
                .child(
                    // Live viewport indicator along the full sheet
                    div()
                        .absolute()
                        .left_0()
                        .w_full()
                        .top(px(live_top))
                        .h(px(live_h))
                        .rounded_full()
                        .bg(rgba(alpha(theme::c().accent, 0x30))),
                )
                .child(
                    // Current preview camera window thumb
                    div()
                        .absolute()
                        .left_0()
                        .w_full()
                        .top(px(thumb_top))
                        .h(px(thumb_h))
                        .rounded_full()
                        .bg(rgba(thumb_color)),
                ),
        )
        .into_any_element()
    }
}

/// Window-level pointer sink so dragging the global scrollbar rail
/// continues smoothly even when the pointer drifts outside the 8 px track.
fn rail_drag_sink(weak: gpui_kit::WeakEntity<PreviewPanel>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let move_sink = weak.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                drop(move_sink.update(cx, |this, cx| {
                    if this.rail_drag {
                        this.scrub_rail_at(f32::from(event.position.y), cx);
                    }
                }));
            });
            let up_sink = weak.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                drop(up_sink.update(cx, |this, cx| {
                    if this.rail_drag {
                        this.rail_drag = false;
                        cx.notify();
                    }
                }));
            });
        },
    )
    .absolute()
    .size_full()
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            if let Err(error) = window.drop_image(image) {
                eprintln!("[shotori] preview cleanup failed: {error}");
            }
        }
        let size_label = self.capture.map(|(w, h)| match self.axis {
            Axis::Vertical => format!("{w} × {}", self.total_height.max(h)),
            Axis::Horizontal => format!("{} × {h}", self.total_height.max(w)),
        });

        let compact = self.panel_w < 224.0;
        let btn_size = if compact { 24.0 } else { 28.0 };
        let btn_gap = if compact { 2.0 } else { 3.0 };
        let bar_px = if compact { 4.0 } else { 6.0 };

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
        let pin_label = crate::i18n::t!("common.pin").to_string();
        let cancel_label = crate::i18n::t!("scroll.action.cancel").to_string();

        let bar = div()
            .flex()
            .items_center()
            .gap(px(btn_gap))
            .h(px(TOOLBAR_H))
            .px(px(bar_px))
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
                btn_size,
                true,
                cx.listener(|this, _, _, cx| this.toggle_pause(cx)),
            ))
            .child(deck_button(
                "scroll-deck-axis",
                axis_label.clone(),
                axis_tip,
                DeckIcon::Own("icons/axis.svg"),
                btn_size,
                !self.axis_locked,
                cx.listener(|this, _, _, cx| this.toggle_axis(cx)),
            ))
            .child(
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .bg(rgba(theme::c().toolbar_border)),
            )
            .child(deck_button(
                "scroll-deck-copy",
                copy_label.clone(),
                format!("{copy_label} · Enter"),
                DeckIcon::Kit(IconName::Copy),
                btn_size,
                true,
                cx.listener(|this, _, _, cx| this.request_exit(ExitTarget::Copy, cx)),
            ))
            .child(deck_button(
                "scroll-deck-save",
                save_label.clone(),
                format!("{save_label} · Ctrl+S"),
                DeckIcon::Kit(IconName::Save),
                btn_size,
                true,
                cx.listener(|this, _, _, cx| this.request_save(cx)),
            ))
            .child(deck_button(
                "scroll-deck-pin",
                pin_label.clone(),
                format!("{pin_label} · Ctrl+P"),
                DeckIcon::Kit(IconName::Pin),
                btn_size,
                true,
                cx.listener(|this, _, _, cx| this.request_pin(cx)),
            ))
            .child(deck_button(
                "scroll-deck-cancel",
                cancel_label.clone(),
                format!("{cancel_label} · Esc"),
                DeckIcon::Kit(IconName::X),
                btn_size,
                true,
                cx.listener(|this, _, _, cx| this.request_cancel(cx)),
            ));
        let toolbar = div().w_full().flex().justify_center().child(bar);
        let info_bar = self.render_info_bar(size_label, cx);

        let (_, well_w, well_h) = self.well_box();
        let mut preview = div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .rounded(px(10.))
            .bg(rgba(theme::c().toolbar_hover))
            .border_1()
            .border_color(rgba(alpha(theme::c().toolbar_border, 0xD0)))
            .on_scroll(cx.listener(Self::scroll_preview));

        if let Some(image) = self.image.clone() {
            let (x_off, y_off, strip_w, strip_h) = fit_strip_box(
                well_w,
                well_h,
                self.preview_cols,
                self.preview_rows,
                self.axis,
            );
            let along_px = match self.axis {
                Axis::Vertical => self.preview_rows.max(1) as f32,
                Axis::Horizontal => self.preview_cols.max(1) as f32,
            };
            let (wash, marker) = match self.viewport {
                Some((v_top, v_len)) => {
                    let top_frac = (v_top as f32 / along_px).min(0.99);
                    let len_frac = (v_len as f32 / along_px).clamp(0.01, 1. - top_frac);
                    let wash = if top_frac > 0.01 || len_frac < 0.98 {
                        alpha(theme::c().accent, 0x14)
                    } else {
                        0
                    };
                    (wash, Some((top_frac, len_frac)))
                }
                None => (0, None),
            };
            let mut strip = div()
                .absolute()
                .left(px(PREVIEW_GUTTER + x_off))
                .top(px(PREVIEW_GUTTER + y_off))
                .w(px(strip_w))
                .h(px(strip_h))
                .child(img(image).w(px(strip_w)).h(px(strip_h)));
            if let Some((top_frac, len_frac)) = marker {
                let band = match self.axis {
                    Axis::Vertical => div()
                        .absolute()
                        .left_0()
                        .w_full()
                        .top(relative(top_frac))
                        .h(relative(len_frac)),
                    Axis::Horizontal => div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .left(relative(top_frac))
                        .w(relative(len_frac)),
                };
                strip = strip.child(
                    band.bg(rgba(wash))
                        .border_1()
                        .border_color(rgba(theme::c().accent))
                        .rounded_sm(),
                );
            }
            preview = preview
                .child(strip)
                .child(self.render_scrollbar_rail(well_h, cx));
        } else {
            let hint = if self.paused {
                crate::i18n::t!("scroll.status.paused")
            } else {
                crate::i18n::t!("scroll.hint.start")
            };
            preview = preview.child(
                div()
                    .size_full()
                    .px(px(8.))
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

        div()
            .id("shotori-scroll-panel")
            .key_context("ShotoriScroll")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::finish))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::pin))
            .on_action(cx.listener(Self::pause))
            .child(rail_drag_sink(cx.entity().downgrade()))
            .child(toolbar)
            .child(div().h(px(TOOLBAR_GAP)))
            .child(info_bar)
            .child(div().h(px(TOOLBAR_GAP)))
            .child(preview)
    }
}

fn deck_button(
    id: &'static str,
    label: String,
    tooltip: String,
    icon: DeckIcon,
    tile_px: f32,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let icon_px = if tile_px <= 24.0 { 14.0 } else { 16.0 };
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(tile_px))
        .rounded(px(6.))
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
                .size(px(icon_px))
                .text_color(rgba(theme::c().toolbar_text)),
        )
        .into_any_element()
}

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

#[cfg(test)]
mod tests {
    use super::{Axis, fit_strip_box};

    #[test]
    fn short_vertical_strip_anchors_at_top_without_rescaling() {
        let (x, y, w, h) = fit_strip_box(240.0, 500.0, 240, 180, Axis::Vertical);
        assert_eq!((x, y), (0.0, 0.0));
        assert!((w - 240.0).abs() < 0.01);
        assert!((h - 180.0).abs() < 0.01);
    }

    #[test]
    fn narrow_tall_vertical_strip_scales_uniformly_instead_of_squashing() {
        // 500 × 900 viewport in a 240 × 380 well: natural height at 240w
        // is 432 > 380. Both dimensions must scale uniformly so w/h stays
        // 500/900 instead of clamping height alone.
        let (x, y, w, h) = fit_strip_box(240.0, 380.0, 500, 900, Axis::Vertical);
        assert_eq!(y, 0.0);
        assert!((h - 380.0).abs() < 0.01);
        let aspect_in = 500.0 / 900.0;
        let aspect_out = w / h;
        assert!(
            (aspect_out - aspect_in).abs() < 1e-4,
            "aspect squashed: expected {aspect_in}, got {aspect_out}"
        );
        assert!((x - (240.0 - w) * 0.5).abs() < 0.01);
    }

    #[test]
    fn horizontal_strip_anchors_left_and_preserves_screen_aspect() {
        let (x, y, w, h) = fit_strip_box(240.0, 500.0, 300, 100, Axis::Horizontal);
        assert_eq!(x, 0.0);
        assert!((w - 240.0).abs() < 0.01);
        assert!((h - 80.0).abs() < 0.01);
        assert!((y - (500.0 - 80.0) * 0.5).abs() < 0.01);
    }
}
