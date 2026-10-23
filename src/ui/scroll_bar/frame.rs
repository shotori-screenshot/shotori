//! The region frame window: ONE full-output transparent layer surface
//! drawing four accent strokes around the shared capture rect, with
//! the ⇕ move handle (its only input region), the chrome-free HUD
//! variant and the window-level pointer sink. Wheel events over the
//! captured content are NOT taken — manual scrolling keeps working
//! under the frame.

use super::panel::{PreviewPanel, scroll_copy_exit, scroll_save_exit};
use super::*;
use crate::ui::theme::alpha;

use std::sync::Arc;

use crate::model::placement::{
    ScrollPanelDock, clamp_moved_rect, frame_grab_chip, frame_strokes, frame_toolbar,
};
use crate::model::session::ScrollRect;
use crate::platform::scroll_capture::{ScrollControls, ScrollEvent};
use crate::ui::theme;
pub(crate) struct FrameView {
    rect: ScrollRect,
    shared: Arc<std::sync::Mutex<ScrollRect>>,
    output: Size<Pixels>,
    /// The session's chrome plan (see `launch`): drives the toolbar's
    /// placement ladder and, when `Hidden`, marks this window as the
    /// session's ONLY chrome — it hosts the engine lifecycle and the
    /// keyboard.
    dock: ScrollPanelDock,
    /// The keyboard focus for chrome-free sessions (the keybindings are
    /// scoped to the `ShotoriScroll` context, bound on the root).
    pub(super) focus: FocusHandle,
    /// Chrome-free only: the engine lifecycle this window hosts when no
    /// panel exists (saving flag + terminal-event guard).
    host: Option<FrameHost>,
    /// (press point, rect at press) — the ⇕ handle's drag: the frame
    /// follows the pointer's VERTICAL position only. Horizontal motion
    /// would desync the stitch (folded columns stop matching) and could
    /// slide the region under the frozen preview panel.
    drag: Option<(Point<Pixels>, ScrollRect)>,
    /// Late-bound by `launch` (the panel starts the engine after the
    /// frame exists): toolbar exits without cross-window dispatch.
    controls: Option<ScrollControls>,
    panel: Option<gpui_kit::WeakEntity<PreviewPanel>>,
    /// Chrome-free pause state (the deck tracks its own).
    paused: bool,
}

/// The chrome-free engine host's state (a miniature of what the panel
/// carries: where a finished canvas goes, and whether a terminal event
/// already fired).
struct FrameHost {
    saving: bool,
    done: bool,
}

impl FrameView {
    /// `shared` is the VERY Arc the engine reads per capture — drags
    /// must write through it, not a lookalike.
    pub(super) fn new(
        shared: Arc<std::sync::Mutex<ScrollRect>>,
        rect: ScrollRect,
        output_w: f32,
        output_h: f32,
        dock: ScrollPanelDock,
        focus: FocusHandle,
    ) -> Self {
        Self {
            rect,
            shared,
            output: size(px(output_w), px(output_h)),
            dock,
            focus,
            host: None,
            drag: None,
            controls: None,
            panel: None,
            paused: false,
        }
    }

    /// Chrome-free hosting: this window owns the engine. The event pump
    /// mirrors the panel's — terminal events run the exits, everything
    /// else is preview traffic with no preview to feed.
    pub(crate) fn host_engine(
        &mut self,
        controls: ScrollControls,
        events: async_channel::Receiver<ScrollEvent>,
        host_window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.controls = Some(controls);
        self.host = Some(FrameHost {
            saving: false,
            done: false,
        });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            while let Ok(event) = events.recv().await {
                if weak
                    .update(cx, |frame, cx| frame.on_host_event(event, &host_window, cx))
                    .is_err()
                {
                    break; // window gone; nothing left to update
                }
            }
        })
        .detach();
    }

    /// The chrome-free event pump's sink: only the terminals matter.
    fn on_host_event(
        &mut self,
        event: ScrollEvent,
        window: &AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let Some((done, saving)) = self.host.as_ref().map(|h| (h.done, h.saving)) else {
            return;
        };
        match event {
            ScrollEvent::Finished {
                width,
                height,
                rgba,
            } => {
                if done {
                    return;
                }
                self.mark_host_done();
                self.controls = None;
                cx.notify();
                if saving {
                    scroll_save_exit(&rgba, width, height, *window, None, cx);
                } else {
                    scroll_copy_exit(&rgba, width, height, *window, None, cx);
                }
            }
            ScrollEvent::Failed { reason, partial } => {
                if done {
                    return;
                }
                self.mark_host_done();
                self.controls = None;
                cx.notify();
                // Something was captured before the failure: deliver it
                // instead of discarding — a broken tail must not eat
                // the good rows above it.
                if let Some((w, h, rgba)) = partial {
                    crate::notify::send(
                        crate::i18n::t!("scroll.notify.stopped_early.title").as_ref(),
                        crate::i18n::t!(
                            "scroll.notify.stopped_early.partial",
                            reason = reason.clone()
                        )
                        .as_ref(),
                    );
                    if saving {
                        scroll_save_exit(&rgba, w, h, *window, None, cx);
                    } else {
                        scroll_copy_exit(&rgba, w, h, *window, None, cx);
                    }
                } else {
                    eprintln!("[shotori] scroll failed: {reason}");
                    crate::notify::send(
                        crate::i18n::t!("scroll.notify.failed.title").as_ref(),
                        &reason,
                    );
                    let _ = window.update(cx, |_, window, _| window.remove_window());
                }
            }
            ScrollEvent::Cancelled => {
                if done {
                    return;
                }
                self.mark_host_done();
                let _ = window.update(cx, |_, window, _| window.remove_window());
                cx.notify();
            }
            // Preview traffic has no preview UI in chrome-free mode;
            // Started/Progress/Viewport/Idle carry nothing actionable.
            _ => {}
        }
    }

    fn mark_host_done(&mut self) {
        if let Some(host) = &mut self.host {
            host.done = true;
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

    /// Enter/Ctrl+C (chrome-free keyboard): deliver the canvas as-is.
    fn finish(&mut self, _: &ScrollFinish, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls
            && self.host.as_ref().is_none_or(|h| !h.done)
        {
            controls.finish();
            cx.notify();
        }
    }

    /// Esc: discard everything.
    fn cancel(&mut self, _: &ScrollCancel, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.take() {
            controls.cancel();
            cx.notify();
        }
    }

    /// Space: freeze or resume sampling in a chrome-free session.
    fn pause_key(&mut self, _: &ScrollPause, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            self.paused = !self.paused;
            controls.set_paused(self.paused);
            cx.notify();
        }
    }

    /// Ctrl+S: finish, then hand the canvas to the save flow.
    fn save(&mut self, _: &ScrollSave, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(host) = &mut self.host {
            host.saving = true;
        }
        if let Some(panel) = self.panel.clone()
            && let Some(panel) = panel.upgrade()
        {
            panel.update(cx, |panel, cx| panel.request_save(cx));
        } else if let Some(controls) = &self.controls {
            controls.finish();
            cx.notify();
        }
    }

    /// The chrome painter: four strokes + the toolbar (when the
    /// placement ladder found room outside the capture region).
    /// `&mut self` with listeners (buttons need entity access).
    ///
    /// The ⇕ handles here and on the chip start a press-and-hold drag:
    /// the frame follows the mouse's VERTICAL position until release
    /// (window-level listeners carry the gesture, implicit grab
    /// included). Horizontal motion is not offered — the stitch cannot
    /// follow a sideways jump.
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // Transparent full-output surface; the strokes and toolbar are
        // the only painted pixels. The input region (set in Render)
        // covers only their rects — the captured content stays
        // interactive for the app beneath.
        let strokes = frame_strokes(self.rect);
        // A docked deck carries every action, so the frame is just the
        // selection outline plus a small move chip. Only the chrome-free
        // session (no deck at all) needs a toolbar on the frame.
        let chip = (self.dock != ScrollPanelDock::Hidden)
            .then(|| {
                frame_grab_chip(
                    self.rect,
                    f32::from(self.output.width),
                    f32::from(self.output.height),
                )
            })
            .flatten();
        let toolbar = (self.dock == ScrollPanelDock::Hidden)
            .then(|| {
                frame_toolbar(
                    self.rect,
                    f32::from(self.output.width),
                    f32::from(self.output.height),
                    self.dock,
                )
            })
            .flatten();
        let grab = cx.listener(|this, event: &MouseDownEvent, _, _| {
            this.drag = Some((event.position, this.rect));
        });
        let copy = cx.listener(|this, _: &MouseDownEvent, _, _| {
            if let Some(controls) = &this.controls {
                controls.finish(); // Finish ≡ finish+copy (panel default)
            }
        });
        let save = cx.listener(|this, _: &MouseDownEvent, window, cx| {
            this.save(&ScrollSave, window, cx);
        });
        let cancel = cx.listener(|this, _: &MouseDownEvent, _, _| {
            if let Some(controls) = this.controls.take() {
                controls.cancel();
            }
        });
        let chip_grab = cx.listener(|this, event: &MouseDownEvent, _, _| {
            this.drag = Some((event.position, this.rect));
        });
        let pause = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if let Some(controls) = &this.controls {
                this.paused = !this.paused;
                controls.set_paused(this.paused);
                cx.notify();
            }
        });
        div()
            .id("shotori-scroll-frame")
            // The ShotoriScroll context + focus are only live when this
            // window holds the keyboard (chrome-free); inert otherwise.
            .key_context("ShotoriScroll")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(Self::finish))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::pause_key))
            // FIRST child: the sink canvas must sit under the toolbar
            // in paint order so the buttons win the element hit-test.
            .child(pointer_sink(cx.entity().downgrade(), self.output))
            .children(chip.map(|chip| {
                div()
                    .id("frame-grab-chip")
                    .absolute()
                    .left(chip.origin.x)
                    .top(chip.origin.y)
                    .w(chip.size.width)
                    .h(chip.size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    // Wears the frame's own accent instead of the
                    // toolbar's neutral slab: the strokes, this handle
                    // and the selection's size chip are one family, so
                    // the handle reads as "grab here" rather than as a
                    // stray widget over the page.
                    .bg(rgba(alpha(theme::c().accent, 0x2E)))
                    .border_1()
                    .border_color(rgba(alpha(theme::c().accent, 0x66)))
                    .cursor(CursorStyle::ResizeUpDown)
                    .hover(|style| style.bg(rgba(alpha(theme::c().accent, 0x4A))))
                    .active(|style| style.bg(rgba(alpha(theme::c().accent, 0x66))))
                    .tooltip(|_, cx| {
                        cx.new(|_| {
                            crate::ui::toolbar::ToolbarTooltip(
                                crate::i18n::t!("scroll.action.drag").to_string().into(),
                            )
                        })
                        .into()
                    })
                    .on_mouse_down(MouseButton::Left, chip_grab)
                    .child(crate::ui::toolbar::chip_grip())
            }))
            .children(strokes.iter().map(|s| {
                div()
                    .absolute()
                    .left(s.origin.x)
                    .top(s.origin.y)
                    .w(s.size.width)
                    .h(s.size.height)
                    .bg(rgba(theme::c().accent))
            }))
            .children(toolbar.map(|toolbar| {
                let copy_label = crate::i18n::t!("common.copy").to_string();
                let save_label = crate::i18n::t!("common.save").to_string();
                let cancel_label = crate::i18n::t!("scroll.action.cancel").to_string();
                let pause_label = if self.paused {
                    crate::i18n::t!("scroll.action.resume").to_string()
                } else {
                    crate::i18n::t!("scroll.action.pause").to_string()
                };
                div()
                    .id("frame-toolbar")
                    .absolute()
                    .left(toolbar.origin.x)
                    .top(toolbar.origin.y)
                    .w(toolbar.size.width)
                    .h(toolbar.size.height)
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .rounded(px(10.))
                    .bg(rgba(theme::c().toolbar_bg))
                    .border_1()
                    .border_color(rgba(theme::c().toolbar_border))
                    .shadow_md()
                    .child(frame_grip(grab))
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(rgba(theme::c().toolbar_border)),
                    )
                    .child(frame_button(
                        "frame-tb-pause",
                        pause_label.clone(),
                        format!("{pause_label} · Space"),
                        if self.paused {
                            IconName::Play
                        } else {
                            IconName::Pause
                        },
                        false,
                        pause,
                    ))
                    .child(frame_button(
                        "frame-tb-copy",
                        copy_label.clone(),
                        format!("{copy_label} · Enter"),
                        IconName::Copy,
                        true,
                        copy,
                    ))
                    .child(frame_button(
                        "frame-tb-save",
                        save_label.clone(),
                        format!("{save_label} · Ctrl+S"),
                        IconName::Save,
                        false,
                        save,
                    ))
                    .child(frame_button(
                        "frame-tb-cancel",
                        cancel_label.clone(),
                        format!("{cancel_label} · Esc"),
                        IconName::X,
                        false,
                        cancel,
                    ))
            }))
    }
}

/// Chrome-free deck substitute: a 26×30 grip that drags the frame
/// vertically. The tooltip is the only label it needs.
fn frame_grip(
    on_press: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("frame-tb-grab")
        .flex()
        .items_center()
        .justify_center()
        .w(px(26.))
        .h(px(30.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(rgba(theme::c().hint_text))
        .cursor(CursorStyle::ResizeUpDown)
        .hover(|style| style.bg(rgba(theme::c().toolbar_hover)))
        .tooltip(|_, cx| {
            cx.new(|_| {
                crate::ui::toolbar::ToolbarTooltip(
                    crate::i18n::t!("scroll.action.drag").to_string().into(),
                )
            })
            .into()
        })
        .on_mouse_down(MouseButton::Left, on_press)
        .child("⇕")
}

/// The chrome-free toolbar's icon button — the deck's 34px tile,
/// shrunk to 30px to keep the floating bar compact.
fn frame_button(
    id: &'static str,
    label: String,
    tooltip: String,
    icon: IconName,
    primary: bool,
    on_press: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (bg, hover, ink) = if primary {
        (
            alpha(theme::c().accent, 0x2E),
            alpha(theme::c().accent, 0x4A),
            theme::c().accent,
        )
    } else {
        (0, theme::c().toolbar_hover, theme::c().toolbar_text)
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(6.))
        .bg(rgba(bg))
        .hover(move |style| style.bg(rgba(hover)))
        .cursor(CursorStyle::PointingHand)
        .aria_label(label)
        .tooltip(move |_, cx| {
            cx.new(|_| crate::ui::toolbar::ToolbarTooltip(tooltip.clone().into()))
                .into()
        })
        .on_mouse_down(MouseButton::Left, on_press)
        .child(svg().path(icon.path()).size(px(16.)).text_color(rgba(ink)))
}

/// The invisible canvas that owns the frame window's pointer listeners.
///
/// `window.on_mouse_event` registrations live for ONE frame's event
/// dispatch — they must be re-attached during every paint (the
/// overlay's `pointer_event_sink` lesson; the v1.1 frame wired them
/// once in `new` and every registration was dead by the first event,
/// so neither strip drags nor the ⇕ button ever moved the frame).
/// Window-level, not element handlers: the implicit grab delivers the
/// whole gesture — including releases outside the handle — to the
/// press window, and element hit-testing would drop exactly those.
///
/// The gesture is VERTICAL-only (see [`clamp_moved_rect`]): the press
/// lands on a ⇕ handle (the only interactive pixels, via the input
/// region) and the frame follows the pointer's y until release.
fn pointer_sink(weak: gpui_kit::WeakEntity<FrameView>, output: Size<Pixels>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let sink = weak.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let _ = sink.update(cx, |this, cx| {
                    let Some((start, origin)) = this.drag else {
                        return;
                    };
                    let next = clamp_moved_rect(
                        origin,
                        f32::from(event.position.y - start.y),
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
        // The input region must track the moving handle — re-set every
        // frame (cheap), same-source geometry as the visuals. Strokes
        // are paint-only: the ⇕ handle (docked) or the toolbar
        // (chrome-free) is the ONLY interactive area, so wheel and
        // clicks anywhere else pass through to the app beneath and
        // manual scrolling keeps working.
        let mut hit: Vec<Bounds<Pixels>> = Vec::new();
        if self.dock != ScrollPanelDock::Hidden
            && let Some(chip) = frame_grab_chip(
                self.rect,
                f32::from(self.output.width),
                f32::from(self.output.height),
            )
        {
            hit.push(chip);
        }
        if self.dock == ScrollPanelDock::Hidden {
            hit.extend(frame_toolbar(
                self.rect,
                f32::from(self.output.width),
                f32::from(self.output.height),
                self.dock,
            ));
        }
        window.set_input_region(Some(&hit));
        FrameView::render(self, cx)
    }
}

// ── the preview panel ────────────────────────────────────────────────
