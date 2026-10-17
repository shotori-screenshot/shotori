//! # Scroll capture chrome: region frame + preview panel
//!
//! The overlays show FROZEN pixels — for the live content to scroll they
//! must unmap, and surfaces replace them for the duration of a long
//! screenshot:
//!
//! - the **region frame** — ONE full-output transparent layer surface
//!   drawing four accent strokes around the selection. Its input region
//!   covers ONLY the move handle (plus the chrome-free toolbar), so
//!   wheel events over the captured content pass through to the app —
//!   manual scrolling keeps working. The ⇕ handle slides the region
//!   VERTICALLY: the shared capture rect updates live and the engine
//!   follows the frame to content that never scrolled into view (for
//!   the sheet, "frame moves down" is indistinguishable from "content
//!   scrolls up"). Horizontal motion is deliberately NOT offered — a
//!   sideways jump desyncs the stitch (the folded columns stop
//!   matching), and free frame movement shipped broken long screenshots.
//! - the **capture HUD** — a floating action bar in the selection
//!   toolbar's own vocabulary (grip-free, 30 px icon tiles, accent
//!   primary) above a strip of the growing long image. Status and live
//!   size ride as small chips ON the strip, the same language as the
//!   selection's size label. No panel header, no footer, no slab: with
//!   the HUD up, the frame is just the selection outline plus a move
//!   chip — no duplicated toolbar.
//!
//! ## What the strip is, relative to the selection
//!
//! The strip is a **viewfinder on the sheet**, not a second copy of the
//! screen: one sheet row is always the same number of panel pixels
//! ([`crate::model::scroll_sheet::preview_fit_rows`]), the window is
//! exactly as tall as the panel's well, and the camera keeps the
//! selection's bottom edge on the window's bottom edge. So the strip
//! moves at the rate the page scrolls — new rows enter at its bottom as
//! the capture takes them, old ones leave at the top — and the tinted
//! band is the part of the sheet the selection is showing right now.
//! While the selection sits at the growing edge (the normal case) the
//! window is simply the last well-height of the sheet; scrolling back up
//! re-aims the camera and the band rises through the strip instead of
//! the whole image sliding.
//!
//! ## Chrome must never paint inside the capture rect
//!
//! wlr-screencopy captures the fully-COMPOSITED output — our layer
//! surfaces included. Anything drawn over the selection burns into every
//! captured frame (the "full-screen long screenshot contains the layer
//! chrome" bug): the panel floats only on a side with a full free strip
//! ([`crate::model::placement::plan_scroll_panel`]) and hangs BESIDE the
//! region — its launch-time position is frozen and the frame's x is
//! frozen with it, so the region can never slide under the panel. The
//! frame toolbar
//! climbs a placement ladder that never enters the region
//! ([`crate::model::placement::frame_toolbar`]) and hides when nowhere
//! fits, and on a single monitor with a full-screen selection the
//! session goes CHROME-FREE: no panel, no toolbar — the frame window
//! hosts the engine lifecycle alone (strokes stay strictly outside the
//! rect; the keyboard carries the exits) and a notification announces
//! the shortcuts. With a second monitor the panel docks there instead.
//!
//! Keyboard lives on the panel (or, chrome-free, on the frame) and
//! mirrors the selection flow exactly: Enter/Ctrl+C finish+copy,
//! Ctrl+S finish+save, Space pauses/resumes, Esc cancels.

use std::sync::Arc;

use gpui_kit::assets::IconName;
#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::model::placement::{
    SCROLL_PANEL_MARGIN, ScrollPanelDock, clamp_moved_rect, frame_grab_chip, frame_strokes,
    frame_toolbar, legacy_scroll_panel, plan_scroll_panel, scroll_panel_margins,
};
use crate::model::scroll_sheet::{Axis, Phase, SheetLimits};
use crate::model::session::ScrollRect;
use crate::platform::scroll_capture::{self, ScrollControls, ScrollEvent, ScrollSpec};
use crate::ui::image_util;
use crate::ui::theme;

actions!(
    scroll,
    [ScrollFinish, ScrollCancel, ScrollSave, ScrollPause]
);

/// Preview panel width (logical px) — owned by `model::placement` (the
/// docking planner sizes its side strips from it); the panel's layout
/// math uses the same constant.
use crate::model::placement::SCROLL_PANEL_W as PANEL_W;
/// Preview well border (the only frame around the page strip).
const PREVIEW_BORDER: f32 = 1.;
/// Gap between the floating action bar and the preview strip.
const TOOLBAR_GAP: f32 = 6.;
/// The action bar's height — a compact version of the selection toolbar.
const TOOLBAR_H: f32 = 36.;
/// Gutter between the preview well's border and the page strip.
const PREVIEW_GUTTER: f32 = 4.;
/// The preview image's display width: window minus the well's border
/// and the inner gutter. The img element's height is derived from this
/// and the image's own aspect — NEVER from the element's intrinsic
/// sizing: gpui leaks the BITMAP's pixel height into the layout as
/// logical px, so on a 2× screen the box rendered 2× taller than
/// width × aspect, throwing the highlight's relative() fractions
/// (fractions of that box) off with it — the whole "preview doesn't
/// match my frame" saga (measured: 1164 physical px of box where 590
/// was correct).
const PREVIEW_IMG_W: f32 = PANEL_W - 2. * PREVIEW_BORDER - 2. * PREVIEW_GUTTER;

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
    let options = SheetLimits::default();
    let shared_rect = spec.rect.clone();
    let rect_now = *shared_rect
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Where may chrome live? Nothing painted over the capture rect can
    // be excluded from wlr-screencopy's composite — plan first, open
    // windows second. Auto/debug mode keeps the legacy center heuristic:
    // a chrome-free frame holding the keyboard eats injected wheel on
    // compositors that route virtual pointers by keyboard focus (niri).
    let other_output = crate::ui::pin::other_output_than(chrome.display_id);
    let dock = match spec.mode {
        scroll_capture::ScrollMode::Manual => {
            plan_scroll_panel(rect_now, chrome.output_width, other_output.is_some())
        }
        scroll_capture::ScrollMode::Auto => legacy_scroll_panel(rect_now, chrome.output_width),
    };
    let chrome_free = dock == ScrollPanelDock::Hidden;

    // The engine starts HERE (not inside the panel): whichever window
    // ends up hosting the lifecycle receives the pair. A spawn failure
    // (not a compositor failure — those arrive as Failed events) still
    // yields a live event stream carrying the error, so the lifecycle
    // code stays single-shaped.
    // The engine renders preview pixels only when a panel exists; the
    // panel writes its well height here once it knows it.
    let preview_height = spec.preview_rows.clone();
    let (controls, events) = match scroll_capture::start(spec, options) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[shotori] scroll engine failed to start: {e:#}");
            let (tx, rx) = async_channel::bounded(1);
            let _ = tx.try_send(ScrollEvent::Failed {
                reason: format!("could not start the scroll engine: {e:#}"),
                partial: None,
            });
            (ScrollControls::dead(), rx)
        }
    };

    // Region frame first: the user must never lose sight of WHAT is
    // being captured, not even for the frames before the panel maps.
    // Chrome-free sessions take the keyboard here (Exclusive + focus):
    // manual wheel follows the pointer, so real scrolling survives —
    // only injected (virtual-pointer) wheel is keyboard-routed, and
    // auto mode never goes chrome-free (see the dock planning above).
    let frame_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: chrome_free,
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
            keyboard_interactivity: if chrome_free {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            },
            ..Default::default()
        }),
        #[cfg(not(target_os = "linux"))]
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    let frame_cell: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::WeakEntity<FrameView>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let cell = frame_cell.clone();
    let frame = cx.open_window(frame_options, |window, cx| {
        let view = cx.new(|cx| {
            let view = FrameView::new(
                shared_rect.clone(),
                rect_now,
                chrome.output_width,
                chrome.output_height,
                dock,
                cx.focus_handle(),
            );
            *cell.borrow_mut() = Some(cx.entity().downgrade());
            view
        });
        // Chrome-free: this window IS the keyboard — focus its handle
        // (WindowOptions::focus only activates the window).
        if chrome_free && let Some(entity) = cell.borrow().as_ref().and_then(|w| w.upgrade()) {
            let handle = entity.read(cx).focus.clone();
            window.focus(&handle, cx);
        }
        view
    });

    // Chrome-free: no second window — the frame hosts the engine
    // lifecycle and the keyboard alone (strokes outside the region are
    // the only pixels; the notification carries the shortcuts).
    if chrome_free {
        let host_window = frame.as_ref().ok().map(|f| (*f).into());
        let host_entity = frame_cell.borrow().as_ref().and_then(|w| w.upgrade());
        if let (Some(entity), Some(host_window)) = (host_entity, host_window) {
            entity.update(cx, |f, cx| f.host_engine(controls, events, host_window, cx));
        }
        crate::notify::send(
            crate::i18n::t!("scroll.notify.running.title").as_ref(),
            crate::i18n::t!("scroll.notify.running.body").as_ref(),
        );
        return Ok(());
    }

    // The panel docks on its planned side of the capture output — or,
    // when no side fits, on the OTHER output (display + logical size
    // come from the pin registry; the preview column math needs that
    // output's height, not the capture one's).
    let (panel_display, panel_out_h) = match dock {
        ScrollPanelDock::OtherOutput => {
            // `other_output` is Some by construction of the plan; fall
            // back to the capture display rather than unwrapping.
            other_output
                .map(|(id, bounds)| (Some(id), f32::from(bounds.size.height)))
                .unwrap_or((chrome.display_id, chrome.output_height))
        }
        _ => (chrome.display_id, chrome.output_height),
    };
    // A floating card, not a full-height sidebar: the deck reads as one
    // object instead of a screen-wide slab, and keeps the output's
    // vertical rhythm. Tall enough for a useful preview on a 1080p
    // screen, capped so 4K users do not get a 1500px column.
    let panel_h = (panel_out_h * 0.40)
        .clamp(320., 500.)
        .min((panel_out_h - 2. * SCROLL_PANEL_MARGIN).max(120.));
    // The panel hangs BESIDE the capture rect (user-reported: pinned to
    // the output edge it read as disconnected from the region it
    // previews) — vertically centered on the rect, horizontally hugging
    // its planned side across SCROLL_PANEL_GAP. Layer surfaces ignore
    // `window_bounds` origins, so the position rides the TOP|LEFT
    // margins. It is frozen for the session, and the frame's x is
    // frozen with it (vertical-only drags), so the region can never
    // slide under the panel.
    let (panel_top, panel_left) = scroll_panel_margins(
        rect_now,
        chrome.output_width,
        chrome.output_height,
        panel_h,
        dock,
    );
    let panel_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        // Only the action bar and the preview strip paint — the window
        // itself is a floating HUD, not a slab over the desktop.
        window_background: WindowBackgroundAppearance::Transparent,
        focus: true,
        display_id: panel_display,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(PANEL_W), px(panel_h)),
        })),
        #[cfg(target_os = "linux")]
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-preview".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::LEFT,
            exclusive_zone: Some(px(-1.)),
            // The panel owns the keyboard so the shortcuts match the
            // selection flow. NOTE: if the compositor routes wheel by
            // keyboard focus (niri does for virtual pointers), scrolling
            // degrades to frame-dragging — deliberately shipped both.
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            // CSS order: top, right, bottom, left — (top, left) carry
            // the beside-the-rect position.
            margin: Some((px(panel_top), px(0.), px(0.), px(panel_left))),
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
            let panel = PreviewPanel::new(controls, events, panel_h, preview_height, window, cx);
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

// ── the region frame ─────────────────────────────────────────────────

/// One full-output transparent surface drawing the region frame and
/// owning its drag interaction.
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
    focus: FocusHandle,
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
    fn new(
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
    fn new(
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
fn scroll_copy_exit(
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
fn scroll_save_exit(
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
                DeckTone::Neutral,
                true,
                cx.listener(|this, _, _, cx| this.toggle_pause(cx)),
            ))
            .child(deck_button(
                "scroll-deck-axis",
                axis_label.clone(),
                axis_tip,
                DeckIcon::Own("icons/axis.svg"),
                DeckTone::Neutral,
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
                DeckTone::Primary,
                true,
                cx.listener(|this, _, _, cx| this.request_finish(cx)),
            ))
            .child(deck_button(
                "scroll-deck-save",
                save_label.clone(),
                format!("{save_label} · Ctrl+S"),
                DeckIcon::Kit(IconName::Save),
                DeckTone::Neutral,
                true,
                cx.listener(|this, _, _, cx| this.request_save(cx)),
            ))
            .child(deck_button(
                "scroll-deck-cancel",
                cancel_label.clone(),
                format!("{cancel_label} · Esc"),
                DeckIcon::Kit(IconName::X),
                DeckTone::Neutral,
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

/// The floating bar's icon look: the selection toolbar's 30px tile,
/// with one accent tile for the primary exit. Disabled tiles fade but
/// keep their tooltip (the axis explains why it locked).
fn deck_button(
    id: &'static str,
    label: String,
    tooltip: String,
    icon: DeckIcon,
    tone: DeckTone,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (bg, hover, ink) = match tone {
        DeckTone::Neutral => (0, theme::c().toolbar_hover, theme::c().toolbar_text),
        DeckTone::Primary => (
            alpha(theme::c().accent, 0x2E),
            alpha(theme::c().accent, 0x4A),
            theme::c().accent,
        ),
    };
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(7.))
        .bg(rgba(bg))
        .aria_label(label)
        .tooltip(move |_, cx| {
            cx.new(|_| crate::ui::toolbar::ToolbarTooltip(tooltip.clone().into()))
                .into()
        });
    let button = if enabled {
        button
            .hover(move |style| style.bg(rgba(hover)))
            .cursor(CursorStyle::PointingHand)
            .on_click(on_click)
    } else {
        button.opacity(0.45)
    };
    button
        .child(icon.svg().size(px(16.)).text_color(rgba(ink)))
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

#[derive(Clone, Copy)]
enum DeckTone {
    Neutral,
    Primary,
}

/// `0xRRGGBBAA` with a replacement alpha — theme colors are opaque.
fn alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xFFFFFF00) | u32::from(alpha)
}
