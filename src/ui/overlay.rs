//! # Screenshot overlay: assembly layer for frozen-screen + selection interaction
//!
//! Flow: frozen frame as the base (img) → drag a selection (the selection
//! "sees through", everything around it dims) → Enter copies to clipboard /
//! Ctrl+S saves PNG / Ctrl+O OCR / Esc exits.
//!
//! Division of labor: pure logic lives in [`crate::model::selection`] (state
//! machine) and [`crate::model::export`] (crop/encode), visuals in [`crate::ui::hud`]
//! and [`crate::ui::toolbar`] — this file only does gpui assembly:
//! window, events → state-machine calls, state → rendering.

use std::sync::Arc;

#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::actions::{
    CancelText, ClearAnnotations, CopySelection, DeleteAnnotation, FinishPolyline, OcrSelection,
    PinSelection, QuitOverlay, RedoAnnotation, SaveSelection, SelectScreen, ToggleArrow,
    ToggleEllipse, ToggleEraser, ToggleHighlighter, ToggleLine, ToggleMosaic, ToggleNumber,
    TogglePencil, TogglePolyline, ToggleRectangle, ToggleSelect, ToggleText, UndoAnnotation,
};
use crate::model::placement::round_px;
use crate::model::selection::{PressTarget, Selection};
use crate::platform::capture::Capture;
use crate::ui::hud::{
    annotation_chrome, eraser_chrome, handle_cursor, hover_outline, magnifier_loupe,
    selection_backdrop, selection_handles, selection_label,
};
use crate::ui::image_util;
use crate::ui::toolbar::selection_toolbar;

pub struct Overlay {
    focus_handle: FocusHandle,
    /// The frozen screen image (displayed by the img element)
    frozen: Arc<RenderImage>,
    highlighter_cache: std::rc::Rc<std::cell::RefCell<crate::annotation::HighlighterCache>>,
    canvas_images: std::rc::Rc<std::cell::RefCell<image_util::CanvasImages>>,
    number_cache: std::rc::Rc<std::cell::RefCell<crate::annotation::NumberCache>>,
    /// Raw pixels (for cropping)
    capture: Arc<Capture>,
    session: Entity<crate::model::session::ScreenshotSession>,
    _subscriptions: Vec<Subscription>,
    /// First-run OCR setup (confirm → download progress), open while active
    ocr_setup: Option<Box<crate::ui::ocr_setup::OcrSetup>>,
    setup_focus: crate::ui::ocr_setup::SetupFocus,
    /// OCR inference in flight → show the busy badge (spinner)
    ocr_busy: bool,
    text_editing: Option<crate::ui::text_editor::TextEditor>,
    text_subscription: Option<Subscription>,
    /// Double-click value editor for a number badge: the shape index
    /// and its pre-edit snapshot. `text_editing` carries the editor
    /// itself, so every gate it implies (blocked canvas, Esc cancel,
    /// Enter commit, click-away commit) applies unchanged.
    number_edit: Option<(usize, crate::annotation::Shape)>,
    /// The window cursor for the current pointer position/state
    /// (crosshair / open hand / resize), refreshed by the pointer-move
    /// path and pushed during paint by the handles canvas.
    cursor: std::rc::Rc<std::cell::Cell<CursorStyle>>,
    /// Scroll accumulator for wheel-stepping the active tool's S/M/L
    /// preset: touchpads emit sub-notch deltas, so they pile up here
    /// until a full notch (±1.0) is reached
    size_scroll_acc: f32,
    /// boot trace: this instance's first render not yet reported
    /// (per-instance — the warm-window suite opens several overlays)
    perf_first_render_pending: bool,
    /// The toolbar's size slider, rebuilt when the active tool (its
    /// spec) changes; `SliderEvent::Change` writes through to the
    /// session. UI state, never crosses to the model.
    slider: Option<(
        crate::annotation::ShapeKind,
        Entity<gpui_kit::base::slider::SliderState>,
    )>,
    slider_sub: Option<Subscription>,
}

impl Overlay {
    pub fn new(
        capture: Arc<Capture>,
        session: Entity<crate::model::session::ScreenshotSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::boot_mark("overlay constructed (window created)");
        crate::ui::theme::follow_system(window.appearance());
        let frozen =
            image_util::rgba_to_render_image(capture.rgba.clone(), capture.width, capture.height);

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let debug_targeted = crate::ui::e2e::debug_targeted(&capture.output_name);
        if debug_targeted {
            crate::ui::e2e::spawn_debug_action(window, cx);
        }

        session.update(cx, |session, cx| {
            session.set_size(&capture.output_name, window.bounds().size);
            if let Selection::Selected { bounds } = crate::ui::e2e::debug_selection(debug_targeted)
            {
                session.begin(&capture.output_name, bounds.origin);
                session.end(&capture.output_name, bounds.bottom_right());
            }
            cx.notify();
        });
        let mut overlay = Self {
            focus_handle,
            frozen,
            canvas_images: Default::default(),
            number_cache: Default::default(),
            highlighter_cache: Default::default(),
            capture,
            session,
            _subscriptions: Vec::new(),
            ocr_setup: None,
            setup_focus: crate::ui::ocr_setup::SetupFocus::new(cx),
            ocr_busy: false,
            text_editing: None,
            text_subscription: None,
            number_edit: None,
            cursor: std::rc::Rc::new(std::cell::Cell::new(CursorStyle::Crosshair)),
            perf_first_render_pending: true,
            slider: None,
            slider_sub: None,
            size_scroll_acc: 0.0,
        };
        overlay.attach_observers(window, cx);
        overlay
    }

    fn attach_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions = vec![
            window.observe_window_appearance(|window, cx| {
                crate::ui::theme::follow_system(window.appearance());
                cx.refresh_windows();
            }),
            cx.observe_in(&self.session, window, |this, _, window, cx| {
                // A change from another output can remove this toolbar while
                // one of its controls still owns the local keyboard focus.
                if let Some(editor) = &mut this.text_editing {
                    let a = this.session.read(cx).annotations();
                    if editor.sync_style(a.text_size(), a.color().0, cx) {
                        let accepted_size = editor.font_size;
                        this.session.update(cx, |s, cx| {
                            s.edit_annotation_settings(|a| a.apply_size(accepted_size));
                            cx.notify();
                        });
                        this.refresh_text(cx);
                    }
                }
                if this.ocr_setup.is_none() && this.text_editing.is_none() {
                    window.focus(&this.focus_handle, cx);
                }
                // The repaint below re-derives the window cursor in
                // render() — no cursor plumbing needed here.
                cx.notify();
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.session.update(cx, |session, cx| {
                    if session.set_size(&this.capture.output_name, window.bounds().size) {
                        cx.notify();
                    }
                });
            }),
        ];
    }

    /// The window cursor for the current interaction state, hit-tested
    /// against the selection at the last known pointer position:
    /// crosshair for (new) selection drawing and annotation tools, the
    /// resize arrows on the handles, a pointing hand over an unselected
    /// shape (click-to-pick), an open hand over the selected shape's
    /// body, the selection interior and the toolbar grips, a closed
    /// hand while moving — or dragging the toolbar by a grip.
    fn cursor_style(&self, cx: &App) -> CursorStyle {
        let session = self.session.read(cx);
        if session.blocked() {
            return CursorStyle::Arrow;
        }
        if session.toolbar_drag_active() {
            return CursorStyle::ClosedHand;
        }
        let selection = session.selection();
        if selection.is_dragging() {
            return CursorStyle::Crosshair;
        }
        match selection {
            Selection::Moving { .. } => CursorStyle::ClosedHand,
            Selection::Resizing { handle, .. } => handle_cursor(handle),
            _ => {
                // Chrome first, selection affordances second: the grips
                // (open hand — draggable), then the toolbar body (arrow —
                // the toolbar can sit INSIDE the box and must steal the
                // cursor from the move affordance beneath it). Geometry
                // comes from the session — the same rects the render side
                // draws — so the two cannot drift apart. The position
                // comes from the session too: the GLOBAL pointer mapped
                // into this window, so the affordance is right even on a
                // window the pointer never moved over (cross-screen
                // release under Wayland's implicit grab — the press
                // window received every event, this one none).
                let p = session.pointer_in(&self.capture.output_name);
                if let Some((lg, rg)) = session.toolbar_grips(&self.capture.output_name) {
                    if p.is_some_and(|p| lg.contains(&p) || rg.contains(&p)) {
                        return CursorStyle::OpenHand;
                    }
                    if session
                        .toolbar_bounds(&self.capture.output_name)
                        .is_some_and(|tb| p.is_some_and(|p| tb.contains(&p)))
                    {
                        return CursorStyle::Arrow;
                    }
                }
                if session.annotations().enabled()
                    || session.is_body_moving()
                    || session.handle_drag_anchor().is_some()
                    || session.annotation_handle_hover().is_some()
                    || session.annotation_hover().is_some()
                {
                    // body move: grabbed; handle drag keeps its own
                    // affordance (plain arrow for point handles, the
                    // diagonal for corners); a handle under the pointer
                    // promises the same; a shape body selects on press
                    // (pointing hand — open hand once selected, when
                    // the press becomes a move); blank canvas keeps
                    // the crosshair
                    if session.is_body_moving() {
                        return CursorStyle::ClosedHand;
                    }
                    if let Some((kind, anchor)) = session.handle_drag_anchor() {
                        return annotation_handle_cursor(kind, anchor);
                    }
                    if let Some((kind, anchor)) = session.annotation_handle_hover() {
                        return annotation_handle_cursor(kind, anchor);
                    }
                    // The pick/grab split (issue #17): a hand means
                    // "holding something", so before anything is
                    // grabbed the affordance over a shape is "click to
                    // pick"; the already-selected shape is a move
                    // affordance (open hand) and the drag itself is
                    // the closed hand above.
                    match session.annotation_hover() {
                        Some(crate::annotation::ShapeHover::Move) => {
                            return CursorStyle::OpenHand;
                        }
                        Some(crate::annotation::ShapeHover::Pick) => {
                            return CursorStyle::PointingHand;
                        }
                        None => {}
                    }
                    if session.annotations().enabled() {
                        return CursorStyle::Crosshair;
                    }
                }
                if let (Some(bounds), Some(global)) = (selection.bounds(), session.pointer_global())
                {
                    return match crate::model::selection::press_target(bounds, global) {
                        PressTarget::Handle(h) => handle_cursor(h),
                        PressTarget::Interior => CursorStyle::OpenHand,
                        PressTarget::Outside => CursorStyle::Crosshair,
                    };
                }
                CursorStyle::Crosshair
            }
        }
    }

    /// Recompute and store the cursor; returns whether it changed. The
    /// PRIMARY derivation point is the top of `render` — this method
    /// exists for the one case where the cursor changes while nothing
    /// else does: a pointer move across an affordance boundary needs a
    /// repaint to push the new style, so the move listener must know.
    fn refresh_cursor(&self, cx: &App) -> bool {
        let style = self.cursor_style(cx);
        if self.cursor.get() == style {
            return false;
        }
        self.cursor.set(style);
        true
    }
    fn subscribe_text(
        &mut self,
        input: &Entity<crate::ui::text_input::TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text_subscription =
            Some(
                cx.subscribe_in(input, window, |this, _, event, window, cx| {
                    if matches!(event, gpui_kit::base::input::InputEvent::Change) {
                        this.refresh_text(cx);
                    }
                    if matches!(
                        event,
                        gpui_kit::base::input::InputEvent::PressEnter { shift: false, .. }
                    ) {
                        this.finish_text(true, window, cx);
                    }
                }),
            );
    }

    fn start_text(&mut self, local: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let font_size = self.session.read(cx).annotations().text_size();
        let Some(bounds) =
            self.session
                .read(cx)
                .text_bounds(&self.capture.output_name, local, font_size)
        else {
            return;
        };
        let color = self.session.read(cx).annotations().color().0;
        let local = bounds.origin
            - self
                .session
                .read(cx)
                .screen_origin(&self.capture.output_name);
        let editor = crate::ui::text_editor::TextEditor::new(bounds, local, font_size, color, cx);
        self.subscribe_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.refresh_text(cx);
        self.session.update(cx, |s, cx| {
            s.set_blocked(true);
            cx.notify();
        });
        cx.defer_in(window, |this, window, cx| {
            if let Some(editor) = &this.text_editing {
                editor.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn start_number_edit(
        &mut self,
        ix: usize,
        badge_local: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shape) = self
            .session
            .read(cx)
            .annotations()
            .committed()
            .get(ix)
            .cloned()
        else {
            return;
        };
        // The editor box floats on the badge; text matches the digit
        // scale the badge itself renders at (diameter × 0.55, the
        // layout factor in annotation::number).
        let diameter = f32::from(shape.bounds.size.width);
        let editor = crate::ui::text_editor::TextEditor::new(
            shape.bounds,
            badge_local,
            diameter * 0.55,
            shape.color,
            cx,
        );
        editor.input().update(cx, |input, cx| {
            input.set_value(&shape.number.unwrap_or(1).to_string(), cx)
        });
        self.subscribe_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.number_edit = Some((ix, shape));
        self.session.update(cx, |s, cx| {
            s.set_blocked(true);
            cx.notify();
        });
        cx.defer_in(window, |this, window, cx| {
            if let Some(editor) = &this.text_editing {
                editor.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn refresh_text(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.text_editing else {
            return;
        };
        editor.refresh(cx);
        let value = editor.value(cx);
        if let Some((ix, _)) = &self.number_edit {
            // parse-or-hold: a half-typed or non-numeric buffer
            // previews nothing — the badge keeps its last value until
            // the buffer parses again
            if let Ok(v) = value.trim().parse::<u32>() {
                self.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.preview_number(*ix, v));
                    cx.notify();
                });
            }
        } else {
            let bounds = editor.actual_bounds();
            self.session.update(cx, |s, cx| {
                s.preview_text(bounds, value);
                cx.notify();
            });
        }
        cx.notify();
    }

    fn start_edit_text(
        &mut self,
        ix: usize,
        click_local: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (shape, local_origin, available) = {
            let s = self.session.read(cx);
            let screen_origin = s.screen_origin(&self.capture.output_name);
            let Some(shape) = s.annotations().shape(ix).cloned() else {
                return;
            };
            // Occupied bounds are for selection; editing can grow to the
            // screenshot edge, just like a newly placed text annotation.
            let local_origin = shape.bounds.origin - screen_origin;
            let Some(available) =
                s.text_bounds(&self.capture.output_name, local_origin, shape.width)
            else {
                return;
            };
            (shape, available.origin - screen_origin, available)
        };
        let text = shape.text.as_deref().unwrap_or("");
        let font_size = shape.width;
        let color = shape.color;
        let editor = crate::ui::text_editor::TextEditor::new_with_text(
            available,
            local_origin,
            font_size,
            color,
            text,
            Some(click_local),
            cx,
        );
        let started = self.session.update(cx, |s, cx| {
            let started = s.begin_text_edit(ix);
            if started {
                cx.notify();
            }
            started
        });
        if !started {
            return;
        }
        self.subscribe_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.refresh_text(cx);
        cx.defer_in(window, |this, window, cx| {
            if let Some(editor) = &this.text_editing {
                editor.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn finish_text(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.text_editing.take() else {
            return;
        };
        self.text_subscription.take();
        if let Some((ix, before)) = self.number_edit.take() {
            // commit only a full integer; anything else (Esc, empty,
            // trailing junk) restores the pre-edit value
            let value = editor.value(cx);
            let parsed = commit.then(|| value.trim().parse::<u32>().ok()).flatten();
            self.session.update(cx, |s, cx| {
                s.set_blocked(false);
                s.edit_annotations(|a| match parsed {
                    Some(v) => {
                        a.preview_number(ix, v);
                        a.commit_move(ix, before);
                    }
                    None => a.preview_number(ix, before.number.unwrap_or(1)),
                });
                cx.notify();
            });
        } else {
            self.session.update(cx, |s, cx| {
                s.preview_text(editor.actual_bounds(), editor.value(cx));
                s.finish_text_edit(commit);
                cx.notify();
            });
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Shared body of every annotation-tool toggle action: commit any
    /// pending text edit, take the keyboard focus back from the toolbar
    /// button, and flip the tool in the session. One place instead of a
    /// dozen keybindings' worth of identical plumbing.
    fn toggle_tool(
        &mut self,
        kind: crate::annotation::ShapeKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_text(true, window, cx);
        window.focus(&self.focus_handle, cx);
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.toggle(kind));
            cx.notify();
        });
    }

    /// WindowOptions for the overlay window: a layer-shell surface
    /// anchored on all four edges with Exclusive keyboard. display_id:
    /// pin to the output the capture came from (without it the
    /// compositor placement picks — multi-monitor = lottery).
    ///
    /// `logical_size` (the output's TRUE size, see
    /// [`Capture::logical_size_f32`]) rides along as window_bounds: the
    /// wayland backend forwards it as the layer surface's `set_size`.
    /// niri ignores client sizes on fully-anchored surfaces, but Hyprland
    /// honors them — and the backend's own default bounds (computed from
    /// the INTEGER wl_output scale, no transform) requested 960×540 for a
    /// 720×1280 1.5x-rotated output (measured live). Passing the correct
    /// size makes both behaviors coincide. A 0×0 "compositor, you decide"
    /// was tried and rejected: the surface never maps on Hyprland (no
    /// configure, no first commit — dead loop).
    ///
    /// The overlay window options: a full-output layer-shell surface.
    /// `logical_size` is the output's true logical size (fractional scale
    /// + transform aware).
    pub fn window_options(
        display_id: Option<DisplayId>,
        logical_size: Size<Pixels>,
    ) -> WindowOptions {
        let kind = WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-overlay".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            ..Default::default()
        });

        WindowOptions {
            app_id: Some(crate::APP_ID.into()),
            titlebar: None,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: true,
            display_id,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: logical_size,
            })),
            kind,
            ..Default::default()
        }
    }

    fn crop(&self, cx: &App) -> Option<(u32, u32, Vec<u8>)> {
        self.session.read(cx).crop(&self.capture.output_name)
    }

    /// Enter / Ctrl+C / toolbar [Copy]: crop → PNG (fast, background) →
    /// clipboard (resident daemon) → exit. The primary exit of daily use.
    /// The encode + clipboard handoff run on the background executor so the
    /// overlay never blocks on compression (the balanced-tier 4K encode cost
    /// ~1.4 s; the fast tier is ~45 ms — ROADMAP perf pass).
    fn copy_selection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some((w, h, rgba)) = self.crop(cx) else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let name = self.capture.output_name.clone();
        let entity = cx.entity();
        cx.spawn(async move |_, cx| {
            copy_to_clipboard(w, h, rgba, name, entity, cx).await;
        })
        .detach();
    }

    /// Ctrl+S / toolbar [Save]: crop → stash pixels → quit the overlay.
    /// The overlay is a layer-shell surface that would cover the native file
    /// dialog, so it exits first; the dialog itself (xdg-desktop-portal
    /// SaveFile), the write and the notification run on the main thread
    /// afterwards — see [`crate::save_dialog`]
    fn save_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some((w, h, rgba)) = self.crop(cx) else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        println!(
            "[shotori] save: handing {w}x{h} (from {}) to the file dialog",
            self.capture.output_name
        );
        crate::save_dialog::stash(w, h, rgba);
        // Unmap all overlays — they would cover the save dialog (layer-shell
        // Overlay layer + exclusive keyboard). Quit is delayed a beat: the
        // run loop needs a few iterations to flush the surface-destroy
        // requests to the compositor — quitting immediately leaves frozen
        // frames mapped on screen (measured)
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        crate::save_dialog::close_overlays(window, cx);
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    /// Ctrl+P / toolbar [Pin]: crop → floating pinned layer surfaces →
    /// overlays down. The pin is one shared state plus a transparent
    /// Top-layer surface per output (see `ui::pin`) — first frame at
    /// the selection's exact spot, draggable across outputs, no
    /// compositor IPC. The app stays alive on pins and exits when the
    /// last one closes — no `cx.quit()` here, unlike copy/save.
    fn pin_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some(crop) = self
            .session
            .read(cx)
            .crop_for_pin(&self.capture.output_name)
        else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let (w, h) = (crop.width, crop.height);
        let spec = crate::ui::pin::PinSpec {
            w,
            h,
            rgba: crop.rgba,
            rect: crop.bounds,
        };
        self.session.update(cx, |session, cx| {
            session.set_blocked(true);
            cx.notify();
        });
        let prepared = cx.background_spawn(async move { crate::ui::pin::prepare(spec) });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepared.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.session.update(cx, |session, cx| {
                    session.set_blocked(false);
                    cx.notify();
                });
                match prepared.and_then(|prepared| crate::ui::pin::open(prepared, cx)) {
                    Ok(()) => {
                        println!("[shotori] pinned {w}x{h}");
                        crate::save_dialog::close_overlays(window, cx);
                    }
                    Err(error) => {
                        eprintln!("[shotori] pin failed: {error:#}");
                        crate::notify::send(
                            "Couldn’t pin image",
                            "Try pinning the selection again.",
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// Ctrl+O / toolbar [OCR]. With cached models this runs immediately; on
    /// the very first use it opens the setup dialog (confirm → progress →
    /// cancel) instead — [`crate::ui::ocr_setup`].
    fn ocr_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).blocked() {
            return; // dialog open or an OCR already running
        }
        let Some((w, h, rgba)) = self
            .session
            .read(cx)
            .crop_original(&self.capture.output_name)
        else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let snapshot = crate::ui::ocr_setup::Snapshot { w, h, rgba };

        self.session.update(cx, |s, cx| {
            s.set_blocked(true);
            cx.notify();
        });
        if crate::ocr::models_missing() {
            self.ocr_setup = Some(Box::new(crate::ui::ocr_setup::OcrSetup::new(snapshot)));
            self.setup_focus.focus_confirm(window, cx);
            cx.notify();
        } else {
            self.spawn_ocr(snapshot, window, cx);
        }
    }

    /// Run OCR on a snapshot (the Ctrl+O fast path, and the tail of the
    /// first-run setup once models are in): background inference → text to
    /// clipboard → quit. Free of `self` so both the action handler and the
    /// download poll loop can call it.
    fn spawn_ocr(
        &mut self,
        snapshot: crate::ui::ocr_setup::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let crate::ui::ocr_setup::Snapshot { w, h, rgba } = snapshot;
        let window_handle = window.window_handle();
        let entity = cx.entity();
        cx.spawn(async move |_, cx| {
            ocr_to_clipboard(w, h, rgba, window_handle, entity, cx).await;
        })
        .detach();
    }

    /// Setup dialog [Download]/[Retry]: switch to Downloading, spawn the
    /// download thread and start the poll loop that drives the progress bar
    /// and the completion transition. The loop holds an Arc clone of the
    /// progress and the entity handle — no shared state mutation races with
    /// the UI thread.
    fn ocr_setup_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Retry after a failure needs a fresh progress arc
        let fresh = std::sync::Arc::new(crate::ocr::DownloadProgress::default());
        if let Some(setup) = self.ocr_setup.as_mut() {
            if !matches!(setup.stage, crate::ui::ocr_setup::Stage::Confirm)
                && !matches!(setup.stage, crate::ui::ocr_setup::Stage::Failed(_))
            {
                return; // already downloading
            }
            setup.progress = fresh;
            setup.stage = crate::ui::ocr_setup::Stage::Downloading;
        } else {
            return; // nothing to confirm (no dialog open)
        }
        let Some(setup) = self.ocr_setup.as_ref() else {
            return;
        };
        let progress = setup.progress.clone();
        self.setup_focus.focus_cancel(window, cx);
        crate::ocr::spawn_download(progress.clone());
        cx.notify();

        let entity = cx.entity();
        let window_handle = window.window_handle();

        cx.spawn(async move |_, cx| {
            let mut last = (0u64, 0u64, 0u8);
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                if !entity.update(cx, |this, _| {
                    this.ocr_setup
                        .as_ref()
                        .is_some_and(|s| s.owns_download(&progress))
                }) {
                    return; // cancelled, closed, or replaced by a newer attempt
                }
                if !progress.is_running() {
                    break;
                }
                let readout = (
                    progress.bytes.load(std::sync::atomic::Ordering::Relaxed),
                    progress.total.load(std::sync::atomic::Ordering::Relaxed),
                    progress.file_idx.load(std::sync::atomic::Ordering::Relaxed),
                );
                if readout != last {
                    last = readout;
                    // re-render: the card reads the atomics at draw time
                    entity.update(cx, |_, cx| cx.notify());
                }
            }
            if progress.finished_ok() {
                println!("[shotori] model download complete");
                // hand the frozen snapshot over to OCR
                let snap = entity.update(cx, |this, _| {
                    if this
                        .ocr_setup
                        .as_ref()
                        .is_some_and(|s| s.owns_download(&progress))
                    {
                        this.ocr_setup.take().map(|s| s.snapshot)
                    } else {
                        None
                    }
                });
                if let Some(crate::ui::ocr_setup::Snapshot { w, h, rgba }) = snap {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        let focus = entity.read(cx).focus_handle.clone();
                        window.focus(&focus, cx);
                    });
                    ocr_to_clipboard(w, h, rgba, window_handle, entity, cx).await;
                }
            } else {
                let err = progress.error();
                let _ = window_handle.update(cx, |_, window, cx| {
                    entity.update(cx, |this, cx| {
                        if let Some(setup) = this.ocr_setup.as_mut()
                            && setup.owns_download(&progress)
                        {
                            setup.stage = crate::ui::ocr_setup::Stage::Failed(err);
                            this.setup_focus.focus_confirm(window, cx);
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }

    /// Setup dialog [Cancel]/[Close] and Esc: abort the download, clean up,
    /// back to plain selection mode.
    fn ocr_setup_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = self.ocr_setup.take() else {
            return;
        };
        setup
            .progress
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.session.update(cx, |s, cx| {
            s.set_blocked(false);
            cx.notify();
        });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
}

/// Enter/Ctrl+C exit path: PNG-encode (fast tier) + clipboard handoff run
/// on the background executor so the frozen overlay never blocks on
/// compression (the balanced-tier 4K encode cost ~1.4 s; the fast tier is
/// ~45 ms — ROADMAP perf pass). The daemon spawn (Linux) is blocking I/O
/// and rides the same background task.
async fn copy_to_clipboard(
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    name: String,
    entity: gpui_kit::Entity<Overlay>,
    cx: &mut gpui_kit::AsyncApp,
) {
    let result = cx
        .background_executor()
        .spawn(async move {
            let png = crate::model::export::encode_png_fast(w, h, &rgba)?;
            crate::clipboard::copy_image(w, h, &rgba, &png)?;
            Ok::<Vec<u8>, anyhow::Error>(png)
        })
        .await;
    match result {
        Ok(png) => {
            println!("[shotori] copied {w}x{h} (from {name}) to clipboard");
            // The thumbnail renders inside the detached notify child —
            // the parent does no pixel work (see notify::copied)
            crate::notify::copied(&png);
            cx.update(|cx| cx.quit());
        }
        Err(e) => {
            // Stay in the overlay on failure: the user can still Ctrl+S
            eprintln!("[shotori] copy failed: {e:#}");
            entity.update(cx, |_, cx| cx.notify());
            crate::notify::send(
                "Couldn’t copy screenshot",
                "Try again, or save the image to a file.",
            );
        }
    }
}

/// Background OCR → text to clipboard → quit. Shared by the Ctrl+O action
/// path and the post-download handover. Flips `ocr_busy` on the view for the
/// duration (spinner badge).
async fn ocr_to_clipboard(
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    window_handle: gpui_kit::AnyWindowHandle,
    entity: gpui_kit::Entity<Overlay>,
    cx: &mut gpui_kit::AsyncApp,
) {
    entity.update(cx, |this, cx| {
        this.ocr_busy = true;
        cx.notify();
    });
    let result: anyhow::Result<String> = cx
        .background_executor()
        .spawn(async move { crate::ocr::run_ocr(&rgba, w, h) })
        .await;
    let _ = window_handle.update(cx, |_, _, cx| match &result {
        Ok(t) if t.trim().is_empty() => {
            crate::notify::send(
                "No text found",
                "Try selecting a clearer area containing text.",
            );
        }
        Ok(t) => {
            if let Err(e) = crate::clipboard::copy_text(t.clone()) {
                eprintln!("[shotori] OCR copy failed: {e:#}");
                crate::notify::send(
                    "Couldn’t copy text",
                    "The clipboard is unavailable. Try again.",
                );
                return;
            }
            let lines = t.lines().count();
            let preview: String = t
                .replace('\n', " ")
                .chars()
                .filter(|c| !c.is_control())
                .take(60)
                .collect();
            println!(
                "[shotori] OCR done {w}x{h} → {lines} line(s) → clipboard (preview: {preview})"
            );
            crate::notify::send("Text copied", &preview);
            cx.quit();
        }
        Err(e) => {
            eprintln!("[shotori] OCR failed: {e:#}");
            // The overlay stays open with no in-UI error display yet —
            // without this notification a keybinding user sees nothing
            crate::notify::send(
                "Couldn’t recognize text",
                "Try again or select a clearer area.",
            );
        }
    });
    // Clear the busy flag on both paths (failure stays on-screen; a stuck
    // spinner would spin forever otherwise)
    entity.update(cx, |this, cx| {
        this.session.update(cx, |s, cx| {
            s.set_blocked(false);
            cx.notify();
        });
        this.ocr_busy = false;
        cx.notify();
    });
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Closest observable proxy for "layer mapped & selectable": the
        // first scene paint commits right after this returns (~1 frame).
        // Per instance so the perf warm-window suite sees every overlay.
        if self.perf_first_render_pending {
            self.perf_first_render_pending = false;
            crate::boot_mark("overlay first render");
        }
        // THE cursor derivation point: every repaint stores the style the
        // handles canvas pushes during paint. Deriving here — instead of
        // at every event and action site — means any state flip lands
        // with a correct cursor on EVERY window, pointer motion or not
        // (e.g. chrome re-hosted by another window's release event).
        self.cursor.set(self.cursor_style(cx));
        // The size slider FIRST: building/rebuilding it needs &mut cx
        // (cx.new / cx.subscribe), which the `shared` read borrow below
        // would block for the rest of render.
        let size_slider = {
            let a = self.session.read(cx).annotations();
            // the edit target is the SELECTED shape when one is live
            // (select-on-place), else the active tool's preset — the
            // slider position, readout and writes all follow it
            let edit = a.edit_kind().map(|kind| {
                (
                    kind,
                    crate::annotation::size_spec(kind),
                    a.current_edit_size(),
                )
            });
            edit.map(|(kind, spec, current)| {
                let state = if self.slider.as_ref().is_none_or(|(k, _)| *k != kind) {
                    // a new tool family means a new range — the state is
                    // baked at build time, so rebuild the entity
                    let state = cx.new(|_| {
                        gpui_kit::base::slider::SliderState::new()
                            .min(spec.min)
                            .max(spec.max)
                            .step(1.)
                            .default_value(current)
                    });
                    let sub = cx.subscribe(
                        &state,
                        |this, _, event: &gpui_kit::base::slider::SliderEvent, cx| {
                            match event {
                                // value writes go to the edit target
                                // (selected shape first) and repaint the
                                // overlay so the thumb follows the pointer
                                gpui_kit::base::slider::SliderEvent::Change(v) => {
                                    this.session.update(cx, |s, cx| {
                                        s.edit_annotation_settings(|a| a.apply_size(v.start()));
                                        cx.notify();
                                    });
                                    cx.notify();
                                }
                                // one history entry per drag
                                gpui_kit::base::slider::SliderEvent::Release(_) => {
                                    this.session.update(cx, |s, _| {
                                        s.edit_annotation_settings(|a| a.end_size_drag())
                                    });
                                }
                            }
                        },
                    );
                    self.slider_sub = Some(sub);
                    self.slider = Some((kind, state.clone()));
                    state
                } else {
                    let (_, state) = self.slider.as_ref().expect("checked above");
                    // sync external changes (wheel) into the thumb;
                    // skip when equal or every frame would re-notify
                    // itself into a loop
                    if (state.read(cx).value().start() - current).abs() > f32::EPSILON {
                        state.update(cx, |s, cx| s.set_value(current, window, cx));
                    }
                    state.clone()
                };
                // single-value sliders carry the thumb position in
                // percentage().END (start stays 0)
                let percentage = state.read(cx).percentage().end;
                crate::ui::toolbar::SizeSlider {
                    state,
                    percentage,
                    current,
                }
            })
        };
        // Keep display geometry stable while dragging. The backdrop paints
        // shared edges directly so fractional DPI cannot open layout seams.
        let filtered = self.session.update(cx, |session, cx| {
            session.request_filtered_preview(&self.capture.output_name, cx)
        });
        let shared = self.session.read(cx);
        let selection = shared.selection();
        let sel = shared.local_bounds(&self.capture.output_name).map(round_px);
        let backdrop = shared
            .backdrop_bounds(&self.capture.output_name)
            .map(round_px);
        let hover = shared.hover_bounds(&self.capture.output_name).map(round_px);
        let selected_shape = shared.selected_shape_local(&self.capture.output_name);
        let shapes = if shared.uses_raster_preview() {
            Vec::new()
        } else {
            shared.local_annotations(&self.capture.output_name)
        };
        let canvas_images = self.canvas_images.clone();
        let filtered_image = filtered.as_ref().map(|(_, image)| image.clone());
        let number_cache = self.number_cache.clone();
        let highlighter_cache = self.highlighter_cache.clone();
        let drawing_polyline = shared.annotations().is_drawing_polyline();
        let active = shared.active_on(&self.capture.output_name);
        let loupe = shared.local_loupe(&self.capture.output_name);
        let eraser = shared.eraser_chrome(&self.capture.output_name);
        let input_view = cx.entity().downgrade();
        let ws = window.bounds().size; // window logical size (= output logical size)

        // Bind the base chain, then attach feature-gated handlers via
        // shadowing — cfg attributes are illegal in the middle of a method
        // chain (see ROADMAP, gpui pitfalls: "#[cfg] cannot hang
        // mid-method-chain")
        let base = div()
            .id("shotori-overlay")
            .key_context(if self.ocr_setup.is_some() {
                "ShotoriOcrSetup"
            } else if self.text_editing.is_some() {
                "ShotoriTextEditing"
            } else if self.session.read(cx).blocked() {
                "ShotoriBlocked"
            } else if drawing_polyline {
                "ShotoriOverlay PolylineDrawing"
            } else {
                "ShotoriOverlay"
            })
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleSelect, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Select, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRectangle, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Rectangle, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEllipse, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Ellipse, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleLine, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Line, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleArrow, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Arrow, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleText, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Text, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &CancelText, window, cx| this.finish_text(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Enter, _, cx| {
                    if this.text_editing.is_some() {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Escape, window, cx| {
                    if this.text_editing.is_some() {
                        this.finish_text(false, window, cx);
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleNumber, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Number, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePencil, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Pencil, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHighlighter, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Highlighter, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEraser, window, cx| {
                this.finish_text(true, window, cx);
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        let kind = if a.tool() == Some(crate::annotation::ShapeKind::EraserRect) {
                            crate::annotation::ShapeKind::EraserRect
                        } else {
                            crate::annotation::ShapeKind::Eraser
                        };
                        a.toggle(kind);
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleMosaic, window, cx| {
                // The toolbar's mosaic slot re-toggles BLUR when that is
                // the active filter variant (they share one button).
                let blur = this.session.read(cx).annotations().tool()
                    == Some(crate::annotation::ShapeKind::Blur);
                let kind = if blur {
                    crate::annotation::ShapeKind::Blur
                } else {
                    crate::annotation::ShapeKind::Mosaic
                };
                this.toggle_tool(kind, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePolyline, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Polyline, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FinishPolyline, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.finish_polyline());
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &UndoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.undo());
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &DeleteAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        a.delete_selected();
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &ClearAnnotations, window, cx| {
                // An in-flight text/number edit would keep its editor
                // open over an emptied canvas; cancel it — committing
                // first is pointless when the result is about to be
                // cleared as part of the same user step.
                this.finish_text(false, window, cx);
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        a.clear_all();
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &RedoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.redo());
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &CopySelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.copy_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectScreen, _, cx| {
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                let name = this.capture.output_name.clone();
                this.session.update(cx, |s, cx| {
                    if s.cycle_select_all(&name) {
                        cx.notify();
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &SaveSelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.save_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PinSelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.pin_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OcrSelection, window, cx| {
                this.finish_text(true, window, cx);
                this.ocr_selection(window, cx);
            }))
            .on_action(cx.listener(
                |this, _: &crate::ui::ocr_setup::OcrSetupConfirm, window, cx| {
                    this.ocr_setup_confirm(window, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::ui::ocr_setup::OcrSetupCancel, window, cx| {
                    this.ocr_setup_cancel(window, cx);
                    cx.stop_propagation();
                },
            ));

        // ⑥ First-run OCR setup dialog (confirm / progress), topmost
        let setup_el: Option<AnyElement> = self
            .ocr_setup
            .as_ref()
            .map(|s| crate::ui::ocr_setup::setup_card(s, &self.setup_focus).into_any_element());

        // OCR-in-flight spinner badge, centered on the selection
        let busy_el: Option<AnyElement> = self
            .ocr_busy
            .then(|| crate::ui::hud::ocr_busy_badge(sel, ws).into_any_element());

        base
            // Two-stage Esc (handled in place, no reliance on bubbling):
            // measured: dispatch_action inside a gpui window stops at the
            // focus path and never reaches App::on_action — exiting must
            // happen here. With the setup dialog open, Esc cancels the
            // dialog instead (aborting any download).
            .on_action(cx.listener(|this, _: &QuitOverlay, window, cx| {
                if this.ocr_setup.is_some() {
                    this.ocr_setup_cancel(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.session.update(cx, |s, cx| {
                    let cancelled = s.cancel_annotation();
                    if cancelled {
                        cx.notify();
                    }
                    cancelled
                }) {
                    cx.stop_propagation();
                    return;
                }
                if this.session.read(cx).selection().is_dragging()
                    || this.session.read(cx).selection().is_editing()
                    || this.session.read(cx).toolbar_drag_active()
                {
                    this.session.update(cx, |s, cx| {
                        s.cancel_drag(); // stage one: abandon this drag / revert the edit / put the toolbar back
                        cx.notify();
                    });
                } else {
                    cx.quit(); // stage two: exit
                }
                cx.stop_propagation();
                cx.notify();
            }))
            // ── Selection interaction (events → state machine) ─────────
            // (the LEFT down is handled window-level in `pointer_event_sink`:
            // a stale pointer focus after a cross-screen release delivers
            // it with out-of-bounds local coordinates, which element
            // hit-testing would drop)
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| {
                if this.session.read(cx).blocked() { return; }
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.finish_polyline());
                    cx.notify();
                });
            }))
            // Wheel steps the active tool's S/M/L size preset (issue #3,
            // phase 1: preset stepping with toolbar highlight sync). Only
            // consumed while an annotation tool is active and no text
            // editor owns the pointer; otherwise the event bubbles.
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let usable = this.text_editing.is_none()
                    && this.session.read(cx).annotations().edit_kind().is_some();
                if !usable {
                    cx.propagate();
                    return;
                }
                let lines = match event.delta {
                    ScrollDelta::Lines(l) => l.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.,
                };
                // One discrete notch must be one step, but a notch
                // arrives (on niri) as Pixels(40 × 3): gpui's wayland
                // backend hard-codes a ×3 amplification of the axis
                // value, and the per-notch value has no cross-
                // compositor standard (continuous pixels also outrank
                // the discrete event, so Lines never reaches us here).
                // Clamping each EVENT to ±1 line lands one notch = one
                // step on any compositor whose notch value reaches the
                // /40 divisor, while touchpads (many small events)
                // still accumulate to full steps below.
                let lines = lines.clamp(-1., 1.);
                // accumulate touchpad-scale deltas; one notch = one preset
                this.size_scroll_acc += lines;
                let mut changed = false;
                while this.size_scroll_acc >= 1.0 {
                    this.session.update(cx, |s, _| {
                        s.edit_annotations(|a| changed |= a.step_size(true));
                    });
                    this.size_scroll_acc -= 1.0;
                }
                while this.size_scroll_acc <= -1.0 {
                    this.session.update(cx, |s, _| {
                        s.edit_annotations(|a| changed |= a.step_size(false));
                    });
                    this.size_scroll_acc += 1.0;
                }
                if changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            // ── Layer stack (bottom to top) ─────────────────────────────
            // ① The frozen screen image (opaque, filling the window)
            .child(img(self.frozen.clone()).size_full())
            .child(
                canvas(
                    move |bounds, window, _| {
                        let highlights = highlighter_cache.borrow_mut().prepare(&shapes, window.scale_factor(), Bounds::new(point(px(0.), px(0.)), bounds.size));
                        let images = number_cache.borrow_mut().prepare(&shapes, window.scale_factor());
                        let next = filtered_image.into_iter()
                            .chain(images.iter().flatten().map(|image| image.image.clone()))
                            .chain(highlights.iter().flatten().map(|image| image.image.clone()))
                            .collect();
                        for image in canvas_images.borrow_mut().replace(next) {
                            // Each output has its own atlas. Do not evict another
                            // window's currently displayed shared session image.
                            if let Err(error) = window.drop_image(image) {
                                eprintln!("[shotori] preview cleanup failed: {error}");
                            }
                        }
                        shapes.into_iter().zip(images).zip(highlights).collect::<Vec<_>>()
                    },
                    move |viewport, shapes, window, _| {
                        if let Some(mut clip) = sel {
                            clip.origin += viewport.origin;
                            window.with_content_mask(
                                Some(ContentMask { bounds: clip }),
                                |window| {
                                    if let Some((mut bounds, image)) = filtered {
                                        bounds.origin += viewport.origin;
                                        if let Err(error) = window.paint_image(bounds, bounds, Corners::default(), image, 0, false) {
                                            eprintln!("[shotori] filter preview failed: {error}");
                                        }
                                    }
                                    for ((shape, number_image), highlight) in shapes {
                                        if shape.kind == crate::annotation::ShapeKind::Highlighter {
                                            if let Some(mut highlight) = highlight {
                                                highlight.bounds.origin += viewport.origin;
                                                if let Err(error) = window.paint_image(highlight.bounds, highlight.bounds, Corners::default(), highlight.image, 0, false) {
                                                    eprintln!("[shotori] highlighter preview failed: {error}");
                                                }
                                            }
                                            continue;
                                        }
                                        if let Some(mut number_image) = number_image {
                                            number_image.bounds.origin += viewport.origin;
                                            if let Err(error) = window.paint_image(number_image.bounds, number_image.bounds, Corners::default(), number_image.image, 0, false) {
                                                eprintln!("[shotori] number preview failed: {error}");
                                            }
                                            continue;
                                        }
                                        if matches!(shape.kind, crate::annotation::ShapeKind::Line | crate::annotation::ShapeKind::Arrow | crate::annotation::ShapeKind::Polyline | crate::annotation::ShapeKind::Pencil) {
                                            for path in shape.line_paths(viewport.origin) {
                                                window.paint_path(path, rgba(shape.color));
                                            }
                                            continue;
                                        }
                                        if shape.kind == crate::annotation::ShapeKind::Ellipse {
                                            if let Some(path) = shape.ellipse_path(viewport.origin)
                                            {
                                                window.paint_path(path, rgba(shape.color));
                                            }
                                            continue;
                                        }
                                        for mut stroke in shape.strokes() {
                                            stroke.origin += viewport.origin;
                                            window.paint_quad(fill(stroke, rgba(shape.color)));
                                        }
                                    }
                                },
                            );
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            // ①½ Annotation selection chrome (stroke + handles),
            // above the marks, below the selection border.
            .child(annotation_chrome(selected_shape))
            // ①¾ Eraser chrome (issue #14): the brush ring / the area
            // rect — pointer affordances, above the marks they erase.
            .children(eraser.map(eraser_chrome))
            // Paint the border above the export-backed preview as well as vector marks.
            .child(selection_backdrop(backdrop))
            // ②¼ Resize handles (above the border; the toolbar paints later
            // and stays on top). Drawn whenever the selection is finalized
            // or being edited — not during a fresh drag.
            .child(selection_handles(
                backdrop,
                selection.is_selected() || selection.is_editing(),
                self.cursor.clone(),
                self.text_editing.is_none() && self.ocr_setup.is_none(),
            ))
            // ②½ Window-snap hover outline (above the dim, below all
            // selection chrome: it is a hint, not a selection)
            .children(hover.map(hover_outline))
            // ②¾ Window-level pointer listeners (see `pointer_event_sink`)
            .child(pointer_event_sink(input_view))
            // ③ Size label stays independent so narrow selections cannot wrap it.
            .children(
                sel.filter(|_| active)
                    .map(|b| selection_label(b, ws, round_px(selection.bounds().unwrap()).size)),
            )
            .children(self.text_editing.as_ref().map(|editor| editor.render()))
            // ④ Toolbar: appears only after release (no flicker while dragging).
            // Its rect comes from the session — anchored, or wherever the
            // user dragged it (see session::toolbar_bounds).
            .children(
                if selection.is_selected()
                    && active
                    && self.ocr_setup.is_none()
                    && (!self.session.read(cx).blocked() || self.text_editing.is_some())
                    && let Some(rect) = shared.toolbar_bounds(&self.capture.output_name)
                {
                    Some(selection_toolbar(
                        rect,
                        self.capture.output_name.clone().into(),
                        self.session.read(cx).annotations(),
                        self.session.clone(),
                        self.text_editing.as_ref().map(|e|e.focus_handle(cx)).unwrap_or_else(||self.focus_handle.clone()),
                        size_slider,
                    ))
                } else {
                    None
                },
            )
            // ⑤ OCR busy badge (spinner on the selection)
            .children(busy_el)
            // ⑤½ Magnifier loupe (issue #19): the drag's precision
            // chrome, topmost below the setup dialog (which blocks
            // gestures anyway, so ⑥ stays above it). Renders only on
            // the output that owns the focus point.
            .children(
                loupe.map(|l| magnifier_loupe(self.frozen.clone(), l, ws).into_any_element()),
            )
            // ⑥ First-run OCR setup dialog (confirm / progress), topmost

            .children(setup_el)
    }
}

// ── Render helpers ──────────────────────────────────────────────────
// (display-pixel rounding now lives in `model::placement::round_px`)

/// The invisible canvas that owns the window-level pointer listeners.
///
/// `window.on_mouse_event` registrations live for one frame's event
/// dispatch, so they must be re-attached during every paint — a canvas
/// whose paint closure registers them is the sanctioned hook. Element-
/// level mouse handlers are not an alternative: under Wayland's
/// implicit grab a drag's events keep arriving at the PRESS window
/// even outside its bounds, and element hit-testing would drop exactly
/// those events.
/// The cursor affordance of an annotation handle, shared by its hover
/// and drag states: corner handles resize along their diagonal, point
/// handles (line endpoints, polyline vertices) reposition — plain
/// arrow, grabbing feedback would add nothing there.
fn annotation_handle_cursor(kind: crate::annotation::ShapeKind, anchor: usize) -> CursorStyle {
    if kind.is_region() {
        match anchor {
            0 | 2 => CursorStyle::ResizeUpLeftDownRight,
            _ => CursorStyle::ResizeUpRightDownLeft,
        }
    } else {
        CursorStyle::Arrow
    }
}

fn pointer_event_sink(input_view: WeakEntity<Overlay>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let view = input_view.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = view.update(cx, |this, cx| {
                    // Why window-level and not an element handler: after a
                    // cross-screen release the compositor may keep pointer
                    // focus on the PRESS window while the pointer sits over
                    // another output (niri re-focuses only on the next
                    // motion). The down then arrives with OUT-OF-BOUNDS
                    // local coordinates — element hit-testing drops it, and
                    // the press silently vanished. The session converts via
                    // the RECEIVING window's origin, so the position lands
                    // correctly no matter which window delivered it.
                    // Toolbar/buttons still own their presses: their element
                    // handlers stop propagation before this root listener.
                    if this.text_editing.is_some() {
                        this.finish_text(true, window, cx);
                        return;
                    }
                    if this.session.read(cx).blocked() {
                        return; // modal dialog: no new selections
                    }
                    let p = this
                        .session
                        .read(cx)
                        .to_global(&this.capture.output_name, event.position);
                    // Double-click on an existing Text shape → inline
                    // edit. Gated to the select and text tools since
                    // the select-tool flip: a draw tool's first click
                    // would ink over the shape, stranding a stray mark
                    // next to the editor. (The text tool's own press
                    // on an existing shape is a no-op — `begin`
                    // refuses Text — which is why it stays allowed.)
                    let tool = this.session.read(cx).annotations().tool();
                    if event.click_count >= 2
                        && matches!(
                            tool,
                            Some(
                                crate::annotation::ShapeKind::Select
                                    | crate::annotation::ShapeKind::Text
                            )
                        )
                    {
                        let hit_text =
                            this.session
                                .read(cx)
                                .annotations()
                                .hit_test(p)
                                .and_then(|ix| {
                                    (this.session.read(cx).annotations().shape_kind(ix)
                                        == Some(crate::annotation::ShapeKind::Text))
                                    .then_some(ix)
                                });
                        if let Some(ix) = hit_text {
                            this.start_edit_text(ix, event.position, window, cx);
                            return;
                        }
                    }
                    if this.session.read(cx).annotations().tool()
                        == Some(crate::annotation::ShapeKind::Text)
                        && this.session.read(cx).annotations().hit_test(p).is_none()
                    {
                        // The first click of a double-click must select the
                        // existing mark, not open an empty editor over it.
                        this.start_text(event.position, window, cx);
                        return;
                    }
                    // Double-click on a badge opens the value editor
                    // (issue #2); select-tool only — the gate lives in
                    // `number_at_double_click`, same reasoning as the
                    // text gate above.
                    if event.click_count == 2
                        && let Some((ix, badge_local)) = this
                            .session
                            .read(cx)
                            .number_at_double_click(&this.capture.output_name, event.position)
                    {
                        this.start_number_edit(ix, badge_local, window, cx);
                        return;
                    }
                    window.focus(&this.focus_handle, cx);
                    this.session.update(cx, |s, cx| {
                        s.pointer_down(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.alt,
                        );
                        cx.notify(); // repaint re-derives the cursor in render
                    });
                });
            });
            let view = input_view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let _ = view.update(cx, |this, cx| {
                    // Apply the event to the session FIRST — state and the
                    // tracked pointer position both current — THEN derive
                    // the cursor. Deriving first would compute the
                    // affordance from the previous position: one event of
                    // lag on every move.
                    this.session.update(cx, |s, cx| {
                        if s.toolbar_drag_active() {
                            // the toolbar follows the pointer; hover
                            // tracking stays off under it
                            if s.toolbar_drag_move(&this.capture.output_name, event.position) {
                                cx.notify();
                            }
                            return;
                        }
                        let changed = s.pointer_move(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.shift,
                        );
                        let hovered = s.hover_at(&this.capture.output_name, event.position);
                        // the eraser ring follows the pointer: moves
                        // repaint even when no state changed
                        if changed || hovered || s.eraser_ring_follows_pointer() {
                            cx.notify();
                        }
                    });
                    if this.refresh_cursor(cx) {
                        cx.notify();
                    }
                });
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = input_view.update(cx, |this, cx| {
                    this.session.update(cx, |s, cx| {
                        if s.toolbar_drag_active() {
                            s.toolbar_drag_end();
                            cx.notify();
                            return;
                        }
                        let finish = event.click_count >= 2 && s.annotations().is_pressed();
                        s.pointer_up(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.shift,
                        );
                        if finish {
                            s.edit_annotations(|a| a.finish_polyline());
                        }
                        cx.notify();
                    });
                });
            });
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod multi_output_tests {
    use super::Overlay;
    use crate::{model::session::ScreenshotSession, platform::capture::Capture};
    use gpui_kit::{AppContext, CursorStyle, MouseButton, TestAppContext, point, px, size};
    use std::sync::Arc;

    #[gpui_kit::test]
    fn all_image_annotation_tools_release_replaced_previews(cx: &mut TestAppContext) {
        use crate::annotation::ShapeKind;
        cx.update(gpui_kit::base::init);
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 200;
        capture.height = 200;
        capture.rgba = vec![255; 200 * 200 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (view, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(200.), px(200.)));
        let mut previous: Vec<Arc<gpui_kit::RenderImage>> = Vec::new();
        for kind in [
            ShapeKind::Highlighter,
            ShapeKind::Mosaic,
            ShapeKind::Blur,
            ShapeKind::Number,
            ShapeKind::Text,
            ShapeKind::Polyline,
        ] {
            cx.update(|_, cx| {
                session.update(cx, |s, cx| {
                    s.begin("screen", point(px(0.), px(0.)));
                    s.end("screen", point(px(190.), px(190.)));
                    s.edit_annotations(|a| a.toggle(kind));
                    s.pointer_down("screen", point(px(10.), px(10.)), false);
                    cx.notify();
                })
            });
            for i in 0..30 {
                cx.update(|window, cx| {
                    session.update(cx, |s, cx| {
                        if kind == ShapeKind::Text {
                            s.preview_text(
                                gpui_kit::Bounds::new(
                                    point(px(10.), px(10.)),
                                    size(px(150.), px(80.)),
                                ),
                                format!("Text {i}"),
                            );
                        } else {
                            s.pointer_move("screen", point(px(30. + i as f32), px(70.)), false);
                        }
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                    let current = view.read(cx).canvas_images.borrow().current().to_vec();
                    assert!(!current.is_empty(), "{kind:?}");
                    for image in &current {
                        assert!(window.has_image_atlas_entry(image), "{kind:?}");
                    }
                    for old in &previous {
                        if !current.iter().any(|image| image.id == old.id) {
                            assert!(!window.has_image_atlas_entry(old), "{kind:?}");
                        }
                    }
                    previous = current;
                });
            }
        }
    }

    #[gpui_kit::test]
    fn continuous_pencil_repaints_release_retired_atlas_images(cx: &mut TestAppContext) {
        cx.update(gpui_kit::base::init);
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 200;
        capture.height = 200;
        capture.rgba = vec![255; 200 * 200 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (_, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(200.), px(200.)));
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.select_all();
                s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Pencil));
                s.pointer_down("screen", point(px(10.), px(10.)), false);
                cx.notify();
            })
        });
        let mut previous: Option<Arc<gpui_kit::RenderImage>> = None;
        for i in 0..250 {
            cx.update(|window, cx| {
                session.update(cx, |s, cx| {
                    s.pointer_move(
                        "screen",
                        point(px(20. + (i % 150) as f32), px(30. + (i % 80) as f32)),
                        false,
                    );
                    if i % 25 == 24 {
                        s.pointer_up("screen", point(px(170.), px(100.)), false);
                        s.pointer_down("screen", point(px(10.), px(10.)), false);
                    }
                    cx.notify();
                });
                window.draw(cx).clear(cx);
                let (_, image) = session.read(cx).filtered_preview("screen").unwrap();
                assert!(window.has_image_atlas_entry(&image));
                if let Some(old) = previous.take() {
                    assert!(!window.has_image_atlas_entry(&old));
                }
                previous = Some(image);
            });
        }
        cx.update(|window, cx| {
            session.update(cx, |s, cx| {
                s.begin("screen", point(px(1.), px(1.)));
                cx.notify();
            });
            window.draw(cx).clear(cx);
            assert!(!window.has_image_atlas_entry(&previous.unwrap()));
        });
    }

    #[gpui_kit::test]
    fn pointer_events_share_selection_and_handle_release_outside_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::base::init);
        let make_capture = |name: &str, x| {
            let mut capture = Capture::for_test((x, 0), 1.);
            capture.output_name = name.into();
            capture.width = 400;
            capture.height = 400;
            capture.rgba = vec![255; 400 * 400 * 4];
            Arc::new(capture)
        };
        let left = make_capture("left", 0);
        let right = make_capture("right", 400);
        let session =
            cx.new(|_| ScreenshotSession::new(vec![left.clone(), right.clone()], Vec::new()));
        let mut second_context = cx.clone();
        let (_, left_cx) =
            cx.add_window_view(|window, cx| Overlay::new(left, session.clone(), window, cx));
        let (_, right_cx) = second_context
            .add_window_view(|window, cx| Overlay::new(right, session.clone(), window, cx));
        for context in [&mut *left_cx, &mut *right_cx] {
            context.simulate_resize(size(px(400.), px(400.)));
            context.update(|window, cx| window.draw(cx).clear(cx));
            context.run_until_parked();
        }
        left_cx.simulate_mouse_down(
            point(px(300.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        left_cx.simulate_mouse_move(
            point(px(450.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        left_cx.simulate_mouse_up(
            point(px(450.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        left_cx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.selection().is_selected());
            assert_eq!(
                shared.local_bounds("right").unwrap().size,
                size(px(50.), px(100.))
            );
            assert_eq!(shared.crop("right").unwrap().0, 150);
        });
        right_cx.run_until_parked();
        right_cx.simulate_mouse_down(
            point(px(100.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_move(
            point(px(200.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_up(
            point(px(200.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        left_cx.run_until_parked();
        left_cx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.local_bounds("left").is_none());
            assert!(shared.active_on("right"));
            assert_eq!(shared.crop("left").unwrap().0, 100);
        });
        // Some compositors transfer pointer events to the destination surface.
        // Convert its local coordinates using that output's desktop origin.
        left_cx.simulate_mouse_down(
            point(px(350.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_move(
            point(px(50.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_up(
            point(px(50.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.selection().is_selected());
            assert_eq!(
                shared.selection().bounds().unwrap().size,
                size(px(100.), px(100.))
            );
            assert_eq!(shared.crop("right").unwrap().0, 100);
        });
    }

    /// The user's cross-screen repro: select on one output, then MOVE the
    /// selection across the seam and release. Wayland's implicit grab
    /// delivers the entire gesture to the press window — without the
    /// rehost on release, the size label and toolbar rendered on NEITHER
    /// output until a fresh click happened to re-host the chrome.
    #[gpui_kit::test]
    fn chrome_follows_a_cross_output_move_through_the_event_pipeline(cx: &mut TestAppContext) {
        cx.update(gpui_kit::base::init);
        let make_capture = |name: &str, x| {
            let mut capture = Capture::for_test((x, 0), 1.);
            capture.output_name = name.into();
            capture.width = 400;
            capture.height = 400;
            capture.rgba = vec![255; 400 * 400 * 4];
            Arc::new(capture)
        };
        let left = make_capture("left", 0);
        let right = make_capture("right", 400);
        let session =
            cx.new(|_| ScreenshotSession::new(vec![left.clone(), right.clone()], Vec::new()));
        let mut second_context = cx.clone();
        let (left_view, left_cx) =
            cx.add_window_view(|window, cx| Overlay::new(left, session.clone(), window, cx));
        let (_, right_cx) = second_context
            .add_window_view(|window, cx| Overlay::new(right, session.clone(), window, cx));
        for context in [&mut *left_cx, &mut *right_cx] {
            context.simulate_resize(size(px(400.), px(400.)));
            context.update(|window, cx| window.draw(cx).clear(cx));
            context.run_until_parked();
        }

        // a selection fully on "right": global (500,80)-(700,280)
        right_cx.simulate_mouse_down(
            point(px(100.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_move(
            point(px(300.), px(280.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_up(
            point(px(300.), px(280.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.run_until_parked();
        right_cx.update(|_, cx| {
            assert!(session.read(cx).active_on("right"));
            assert!(session.read(cx).toolbar_bounds("right").is_some());
        });

        // grab the interior on right's window and drag across the seam:
        // every event keeps arriving through the press window
        // (right-local, far negative x = pointer over "left")
        right_cx.simulate_mouse_down(
            point(px(150.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_move(
            point(px(-300.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_up(
            point(px(-300.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.run_until_parked();
        left_cx.run_until_parked();
        right_cx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.selection().is_selected());
            // the render inputs for left's label + toolbar, none for right
            assert!(shared.active_on("left"), "chrome re-hosts on release");
            assert!(shared.toolbar_bounds("left").is_some());
            assert!(shared.local_bounds("left").is_some());
            assert!(shared.toolbar_bounds("right").is_none());
        });
        // The cursor must be right WITHOUT a wiggle: left's window never
        // received a single pointer event (implicit grab), yet the pointer
        // physically sits inside the selection on it. The affordance is
        // derived from the session-tracked GLOBAL pointer — "can't grab
        // until I slide the mouse" was a stale per-window position.
        left_cx.update(|_, cx| {
            assert_eq!(left_view.read(cx).cursor.get(), CursorStyle::OpenHand);
        });

        // And so must the CLICK. A stale pointer focus after the release
        // (niri re-focuses only on the next motion) delivers the down to
        // the PRESS window — right — with OUT-OF-BOUNDS local
        // coordinates. Element hit-testing drops exactly that; the
        // window-level down listener must still grab.
        right_cx.simulate_mouse_down(
            point(px(-300.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.run_until_parked();
        right_cx.update(|_, cx| {
            assert!(
                session.read(cx).selection().is_editing(),
                "stale-focus press still grabs"
            );
        });
        right_cx.simulate_mouse_move(
            point(px(-280.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.simulate_mouse_up(
            point(px(-280.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        right_cx.run_until_parked();
        left_cx.run_until_parked();
        right_cx.update(|_, cx| {
            // press global (100,150) → grab (50,70); drag to (120,150)
            // → origin (70,80), and the release re-hosts on left again
            let b = session.read(cx).selection().bounds().unwrap();
            assert_eq!(b.origin, point(px(70.), px(80.)));
            assert!(session.read(cx).active_on("left"));
        });
    }

    #[gpui_kit::test]
    fn selection_moves_and_resizes_after_release_through_the_event_pipeline(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::base::init);
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "main".into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (_, vcx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        vcx.simulate_resize(size(px(400.), px(400.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();

        // draw a selection (50,50)-(150,120)
        vcx.simulate_mouse_down(
            point(px(50.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(150.), px(120.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(150.), px(120.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();

        // press the interior and drag: the whole selection translates
        vcx.simulate_mouse_down(
            point(px(100.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(140.), px(110.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(140.), px(110.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.selection().is_selected());
            let b = shared.selection().bounds().unwrap();
            assert_eq!(b.origin, point(px(90.), px(80.)));
            assert_eq!(b.size, size(px(100.), px(70.)));
        });

        // press exactly the bottom-right corner and drag: resize only that
        // corner; the top-left stays pinned
        vcx.simulate_mouse_down(
            point(px(190.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(230.), px(190.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(230.), px(190.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let shared = session.read(cx);
            assert!(shared.selection().is_selected());
            let b = shared.selection().bounds().unwrap();
            assert_eq!(b.origin, point(px(90.), px(80.)));
            assert_eq!(b.size, size(px(140.), px(110.)));
            assert_eq!(shared.crop("main").unwrap().0, 140); // export follows
        });
    }

    #[gpui_kit::test]
    fn cursor_reflects_interior_handles_and_the_inset_toolbar(cx: &mut TestAppContext) {
        cx.update(gpui_kit::base::init);
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "main".into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (overlay, vcx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        vcx.simulate_resize(size(px(400.), px(400.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();

        // A selection that reaches the screen bottom parks the toolbar
        // INSIDE the box (bottom-left): anchor math puts it at (8,344) —
        // width clamped to 384 on this narrow window.
        vcx.simulate_mouse_down(
            point(px(50.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(350.), px(390.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(350.), px(390.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();

        // interior (above the toolbar): move affordance
        vcx.simulate_mouse_move(
            point(px(200.), px(200.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

        // the toolbar's left grip — on this narrow window it even sits
        // OUTSIDE the box horizontally (toolbar pinned to x=8): the grip
        // affordance wins regardless
        vcx.simulate_mouse_move(
            point(px(14.), px(363.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

        // over the inset toolbar: the toolbar owns the cursor, even though
        // the point is still inside the selection
        vcx.simulate_mouse_move(
            point(px(200.), px(363.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::Arrow));

        // the bottom-right corner handle: its resize arrow
        vcx.simulate_mouse_move(
            point(px(350.), px(390.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                overlay.read(cx).cursor.get(),
                CursorStyle::ResizeUpLeftDownRight
            )
        });
    }

    #[gpui_kit::test]
    fn annotation_hover_cursor_picks_then_grabs(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            crate::actions::bind_keys(cx);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "main".into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (overlay, vcx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        vcx.simulate_resize(size(px(400.), px(400.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();

        // a selection first: (50,50)-(150,150)
        vcx.simulate_mouse_down(
            point(px(50.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(150.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(150.), px(150.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();

        // a rectangle annotation inside it: (60,70)-(140,130). Placing
        // no longer selects (the select-tool flip) — while the draw
        // tool runs, hovering the ink stays a CROSSHAIR: the press
        // would draw, and no affordance may promise otherwise.
        vcx.simulate_keystrokes("r");
        vcx.run_until_parked();
        vcx.simulate_mouse_down(
            point(px(60.), px(70.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(140.), px(130.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(140.), px(130.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(session.read(cx).annotations().selected().is_none());
        });
        vcx.simulate_mouse_move(
            point(px(100.), px(70.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::Crosshair));

        // switch to the select tool: the same body now advertises
        // click-to-pick — a hand before anything is grabbed would read
        // as "already holding" (issue #17)
        vcx.simulate_keystrokes("v");
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::PointingHand));

        // press-release without a drag: the shape is selected and
        // the body becomes a move affordance (open hand)
        vcx.simulate_mouse_down(
            point(px(100.), px(70.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(100.), px(70.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(session.read(cx).annotations().selected().is_some());
            assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand);
        });

        // Escape drops the selection (the mode stays active): pick again
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(session.read(cx).annotations().selected().is_none());
            assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::PointingHand);
        });

        // press and drag past the click slop: holding — closed hand
        vcx.simulate_mouse_down(
            point(px(100.), px(70.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(110.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::ClosedHand));
        vcx.simulate_mouse_up(
            point(px(110.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        // release: back to the open hand — the pointer (110,80) sits on
        // the translated shape's top edge, still selected
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
    }

    #[gpui_kit::test]
    fn toolbar_hugs_its_content(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "main".into();
        capture.width = 800;
        capture.height = 600;
        capture.rgba = vec![255; 800 * 600 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (_overlay, vcx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        vcx.simulate_resize(size(px(800.), px(600.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();
        vcx.simulate_mouse_down(
            point(px(100.), px(200.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(600.), px(560.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(600.), px(560.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        // Single row (no tool active): the bar is TB_W_ROW1 wide and the
        // flex_1 spacer between the tool cluster and the action cluster
        // collapses to a small deliberate group break — NOT the ~70px
        // dead hole a full-TB_W bar showed (user-reported).
        vcx.update(|_, cx| {
            assert_eq!(
                session.read(cx).toolbar_bounds("main").unwrap().size.width,
                px(crate::model::placement::TB_W_ROW1)
            );
        });
        let gap = f32::from(
            vcx.debug_bounds("tb-ocr").unwrap().left()
                - vcx.debug_bounds("tb-clear").unwrap().right(),
        );
        assert!(
            gap <= 10.,
            "single-row toolbar has a {gap}px hole between the tools and the actions"
        );
        // …and the right edge is intact: the copy button ends far enough
        // from the bar's right edge that the grip strip still fits
        // (no clipping — the failure mode that motivated measuring
        // widths in the first place).
        let copy_right = f32::from(vcx.debug_bounds("tb-copy").unwrap().right());
        let bar_right =
            vcx.update(|_, cx| f32::from(session.read(cx).toolbar_bounds("main").unwrap().right()));
        assert!(
            copy_right
                <= bar_right - crate::model::placement::BAR_PAD - crate::model::placement::GRIP_W,
            "copy clipped: {copy_right} vs bar right {bar_right}"
        );

        // Two rows (a tool with the color settings row): the bar keeps
        // the full TB_W the color row needs — its last swatch must fit
        // inside the bar with room for the bar's padding.
        vcx.simulate_keystrokes("r");
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.update(|_, cx| {
            let b = session.read(cx).toolbar_bounds("main").unwrap();
            assert_eq!(b.size.height, px(crate::model::placement::TB_H));
        });
        let swatch = vcx.debug_bounds("tb-color-5").unwrap();
        let bar_right =
            vcx.update(|_, cx| f32::from(session.read(cx).toolbar_bounds("main").unwrap().right()));
        assert!(
            f32::from(swatch.right()) <= bar_right - 4.,
            "color row overflows the toolbar: {} > {bar_right}",
            f32::from(swatch.right())
        );

        // The mode+slider settings rows (mosaic, eraser): the readout must
        // sit INSIDE the row's own painted border. These rows once carried
        // fixed widths tuned for the old S/M/L buttons; the wider slider
        // bundle spilled past the border (user-reported, visible on HiDPI).
        for key in ["m", "d"] {
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            let readout = vcx.debug_bounds("tb-size-readout").unwrap();
            let options = vcx.debug_bounds("tb-options").unwrap();
            assert!(
                f32::from(readout.right())
                    <= f32::from(options.right()) - crate::model::placement::BAR_PAD + 0.5,
                "{key} settings row clips the size readout: readout right {} vs row right {}",
                f32::from(readout.right()),
                f32::from(options.right())
            );
        }
    }

    #[gpui_kit::test]
    fn toolbar_drag_works_through_the_event_pipeline(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "main".into();
        capture.width = 800;
        capture.height = 600;
        capture.rgba = vec![255; 800 * 600 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (overlay, vcx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        vcx.simulate_resize(size(px(800.), px(600.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();

        // A selection reaching the bottom parks the toolbar INSIDE the box:
        // anchor math puts its origin at (112, 514), TB_W wide, one row tall.
        vcx.simulate_mouse_down(
            point(px(100.), px(200.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(600.), px(560.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(600.), px(560.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let anchored = session.read(cx).toolbar_bounds("main").unwrap();
            assert_eq!(anchored.origin, point(px(112.), px(514.)));
        });

        // hovering either grip: the open-hand affordance
        vcx.simulate_mouse_move(
            point(px(118.), px(533.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
        let right_grip =
            vcx.update(|_, cx| session.read(cx).toolbar_grips("main").unwrap().1.center());
        vcx.simulate_mouse_move(right_grip, MouseButton::Left, Default::default());
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

        // press the LEFT grip, drag, release: the toolbar follows to any
        // spot on the layer; the selection beneath is untouched
        vcx.simulate_mouse_down(
            point(px(118.), px(533.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(250.), px(100.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::ClosedHand);
            let b = session.read(cx).toolbar_bounds("main").unwrap();
            // grab (6,19) held, x clamped to the window (narrow single-row bar)
            assert_eq!(
                b.origin,
                point(px(800. - crate::model::placement::TB_W_ROW1 - 8.), px(81.))
            );
        });
        vcx.simulate_mouse_up(
            point(px(250.), px(100.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(!session.read(cx).toolbar_drag_active());
            assert_eq!(
                session.read(cx).toolbar_bounds("main").unwrap().origin,
                point(px(800. - crate::model::placement::TB_W_ROW1 - 8.), px(81.))
            );
            // the press on the grip never became a selection interaction
            let b = session.read(cx).selection().bounds().unwrap();
            assert_eq!(b.origin, point(px(100.), px(200.)));
            assert_eq!(b.size, size(px(500.), px(360.)));
        });

        // A tool active → row two appears. Its edges are NOT grips (by
        // design): cursor stays Arrow there and a press does not drag.
        // Regression pair for the "draggable but not a hand" mismatch:
        // the cursor strip and the element rect are now one geometry.
        vcx.simulate_keystrokes("r");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                session.read(cx).toolbar_bounds("main").unwrap().size.height,
                px(crate::model::placement::TB_H)
            );
        });
        // row TWO's left edge: plain toolbar body
        vcx.simulate_mouse_move(
            point(px(255.), px(153.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::Arrow));
        vcx.simulate_mouse_down(
            point(px(255.), px(153.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(300.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(300.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(!session.read(cx).toolbar_drag_active());
            assert_eq!(
                session.read(cx).toolbar_bounds("main").unwrap().origin,
                point(px(160.), px(81.)) // unmoved
            );
        });

        // row ONE's strip still grabs with two rows on screen
        vcx.simulate_mouse_move(
            point(px(171.), px(100.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
        vcx.simulate_mouse_down(
            point(px(171.), px(100.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_move(
            point(px(200.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.simulate_mouse_up(
            point(px(200.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(!session.read(cx).toolbar_drag_active());
            // grab was (11,19); the clamp pins the toolbar at the
            // right margin on this narrow test window
            assert_eq!(
                session.read(cx).toolbar_bounds("main").unwrap().origin,
                point(px(160.), px(281.))
            );
        });
    }

    #[gpui_kit::test]
    fn rectangle_toolbar_keyboard_and_export_share_the_same_state(cx: &mut TestAppContext) {
        geometry_toolbar_keyboard_and_export(cx, crate::annotation::ShapeKind::Rectangle);
    }

    #[gpui_kit::test]
    fn ellipse_toolbar_keyboard_and_export_share_the_same_state(cx: &mut TestAppContext) {
        geometry_toolbar_keyboard_and_export(cx, crate::annotation::ShapeKind::Ellipse);
    }

    #[gpui_kit::test]
    fn line_and_polyline_pointer_keyboard_and_toolbar_workflows(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Line);
    }

    #[gpui_kit::test]
    fn arrow_and_polyline_share_keyboard_focus_history_and_export(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Arrow);
    }

    #[gpui_kit::test]
    fn number_toolbar_keeps_focus_size_and_shared_history(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Number);
    }

    #[gpui_kit::test]
    fn pencil_toolbar_and_curve_share_history(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Pencil);
    }

    #[gpui_kit::test]
    fn highlighter_toolbar_and_curve_share_history(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Highlighter);
    }

    #[gpui_kit::test]
    fn mosaic_and_blur_toolbar_share_focus_history(cx: &mut TestAppContext) {
        stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Mosaic);
    }

    #[gpui_kit::test]
    fn text_input_commit_cancel_and_history(cx: &mut TestAppContext) {
        // Host font fallback changes wrapping and can turn a valid resize
        // into a correctly rejected overflow on CI.
        crate::annotation::text::with_test_font(|| text_input_workflow(cx));
    }

    fn text_input_workflow(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            crate::actions::bind_keys(cx);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 600;
        capture.height = 500;
        capture.rgba = vec![255; 600 * 500 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (view, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(600.), px(500.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(
            point(px(20.), px(20.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(550.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let button = cx.debug_bounds("tb-text").unwrap();
        cx.simulate_click(button.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(32.))));
        cx.simulate_click(point(px(60.), px(60.)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("text-editor").unwrap().size.width <= px(9.));
        assert!(cx.debug_bounds("tb-text").is_some());
        let color = cx.debug_bounds("tb-color-4").unwrap();
        cx.simulate_click(color.center(), Default::default());
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(16.))));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(32.))));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let input = view.read(cx).text_editing.as_ref().unwrap().input().clone();
            input.update(cx, |s, cx| {
                gpui_kit::EntityInputHandler::replace_and_mark_text_in_range(
                    s,
                    None,
                    "nihao",
                    Some(5..5),
                    window,
                    cx,
                )
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                session
                    .read(cx)
                    .annotations()
                    .visible()
                    .last()
                    .unwrap()
                    .text
                    .as_deref(),
                Some("nihao")
            );
            let input = view.read(cx).text_editing.as_ref().unwrap().input().clone();
            input.update(cx, |s, cx| {
                gpui_kit::EntityInputHandler::replace_text_in_range(s, None, "", window, cx)
            });
        });
        cx.simulate_input("rental 中文");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("第二行很长的文字需要自动换行到下一行并立即显示");
        cx.update(|_, cx| {
            assert!(session.read(cx).blocked());
            assert_eq!(
                view.read(cx).text_editing.as_ref().unwrap().value(cx),
                "rental 中文
第二行很长的文字需要自动换行到下一行并立即显示"
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("text-editor").unwrap().size.height > px(32. * 1.35 * 2.));
        let live_pixels = cx.update(|_, cx| session.read(cx).crop("screen").unwrap());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).text_editing.is_none());
            let s = session.read(cx);
            assert_eq!(s.crop("screen").unwrap(), live_pixels);
            assert!(!s.blocked());
            let shapes = s.annotations().visible().collect::<Vec<_>>();
            assert_eq!(shapes.len(), 1);
            assert_eq!(
                shapes[0].text.as_deref(),
                Some(
                    "rental 中文
第二行很长的文字需要自动换行到下一行并立即显示"
                )
            );
            assert_eq!(shapes[0].width, 32.);
            assert_ne!(s.crop("screen"), s.crop_original("screen"));
        });
        // Reopening must retain the shape's style even when the next-tool
        // presets differ, and must replace the original in the raster path.
        let original = cx.update(|_, cx| session.read(cx).annotations().committed()[0].clone());
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| {
                    a.deselect();
                    a.set_tool_size(16.);
                    a.set_color(1);
                });
                cx.notify();
            })
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_click(point(px(65.), px(65.)), Default::default());
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(65.), px(65.)),
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
        });
        cx.simulate_mouse_up(
            point(px(65.), px(65.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.run_until_parked();
        cx.update(|_, cx| {
            let editor = view.read(cx).text_editing.as_ref().unwrap();
            assert_eq!(editor.font_size, original.width);
            assert_eq!(editor.color, original.color);
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("replacement text");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0]
                    .text
                    .as_deref(),
                Some("replacement text")
            );
            assert_ne!(session.read(cx).crop("screen").unwrap(), live_pixels);
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(&session.read(cx).annotations().committed()[0], &original));

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_click(point(px(65.), px(65.)), Default::default());
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(65.), px(65.)),
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
        });
        cx.simulate_mouse_up(
            point(px(65.), px(65.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("a long replacement which must grow beyond the old text width");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("another line");
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotation_settings(|a| a.apply_size(40.));
                cx.notify();
            })
        });
        cx.run_until_parked();
        // This long paragraph wraps into too many rows at 40px with the
        // bundled font. Rejection must keep editor and model in sync.
        cx.update(|_, cx| {
            let editor = view.read(cx).text_editing.as_ref().unwrap();
            assert_eq!(editor.font_size, 32.);
            assert_eq!(session.read(cx).annotations().text_size(), 32.);
            assert_eq!(
                editor.value(cx),
                "a long replacement which must grow beyond the old text width\nanother line"
            );
        });
        // Separately prove that a resize which fits is applied, rather than
        // weakening the assertion to accept either font size.
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("replacement text\nanother line");
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotation_settings(|a| a.apply_size(40.));
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let color = cx.debug_bounds("tb-color-2").unwrap();
        cx.simulate_click(color.center(), Default::default());
        cx.run_until_parked();
        let edited_pixels = cx.update(|_, cx| {
            let editor = view.read(cx).text_editing.as_ref().unwrap();
            assert_eq!(editor.font_size, 40.);
            assert_eq!(editor.color, crate::ui::theme::c().annotation_colors[2]);
            session.read(cx).crop("screen").unwrap()
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).text_editing.is_none());
            assert_eq!(session.read(cx).crop("screen").unwrap(), edited_pixels);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(&session.read(cx).annotations().committed()[0], &original));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert!(session.read(cx).annotations().visible().next().is_none()));
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
        cx.simulate_click(point(px(60.), px(230.)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_input("discard");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).text_editing.is_none());
            assert!(!session.read(cx).blocked());
            assert_eq!(session.read(cx).annotations().visible().count(), 1);
        });
        // Empty confirmation must not add an invisible history entry.
        cx.simulate_click(point(px(60.), px(230.)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).text_editing.is_none());
            assert_eq!(session.read(cx).annotations().visible().count(), 1);
        });
        cx.simulate_click(point(px(60.), px(230.)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_input("click outside");
        cx.simulate_click(point(px(540.), px(290.)), Default::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).text_editing.is_none());
            assert!(!session.read(cx).blocked());
            assert_eq!(session.read(cx).annotations().visible().count(), 2);
        });
    }

    #[gpui_kit::test]
    fn eraser_toolbar_modes_size_and_history(cx: &mut TestAppContext) {
        use crate::annotation::ShapeKind;
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            cx.bind_keys([gpui_kit::KeyBinding::new(
                "escape",
                super::QuitOverlay,
                Some("ShotoriOverlay"),
            )]);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 500;
        capture.height = 500;
        capture.rgba = vec![255; 500 * 500 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (view, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(500.), px(500.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // A selection, then one pencil stroke as erase fodder.
        cx.simulate_mouse_down(
            point(px(20.), px(20.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(350.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let pencil = cx.debug_bounds("tb-pencil").unwrap();
        cx.simulate_click(pencil.center(), Default::default());
        cx.simulate_mouse_down(
            point(px(50.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(
            point(px(180.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(180.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 1));

        // The eraser's toolbar: no color palette; the size slider
        // drives the brush ring's radius.
        let eraser = cx.debug_bounds("tb-eraser").unwrap();
        cx.simulate_click(eraser.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("tb-color-0").is_none());
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(48.))));
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().width(), 48.));

        // Brush press ON the stroke (it would park a click-select for
        // drawing tools) erases the whole shape at once — no zombie
        // half-erased pixels, one history entry.
        cx.simulate_mouse_down(
            point(px(100.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 0));
        cx.simulate_mouse_up(
            point(px(100.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 1));
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 0));

        // Rectangle-eraser mode: no size UI; deletion lands on RELEASE
        // (the dragged area's bounds are only final then).
        let rect = cx.debug_bounds("tb-eraser-rect").unwrap();
        cx.simulate_click(rect.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().tool(),
                Some(ShapeKind::EraserRect)
            )
        });
        cx.simulate_keystrokes("b"); // pencil: draw another stroke
        cx.simulate_mouse_down(
            point(px(60.), px(60.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(
            point(px(200.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(200.), px(80.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 1));
        cx.simulate_keystrokes("d"); // back to the eraser (brush)
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().tool(),
                Some(ShapeKind::Eraser)
            )
        });
        cx.simulate_mouse_down(
            point(px(40.), px(30.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| {
            // in-flight area: deletion only on release
            assert_eq!(session.read(cx).annotations().committed().len(), 1)
        });
        cx.simulate_mouse_move(
            point(px(220.), px(110.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(220.), px(110.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 0));
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 1));
        // "d" toggles the ACTIVE eraser variant — brush, not rect
        cx.simulate_keystrokes("d");
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        cx.simulate_keystrokes("d");
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().tool(),
                Some(ShapeKind::Eraser)
            )
        });
        let _ = view;
    }

    fn stroke_and_polyline_workflows(cx: &mut TestAppContext, kind: crate::annotation::ShapeKind) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            cx.bind_keys([gpui_kit::KeyBinding::new(
                "escape",
                super::QuitOverlay,
                Some("ShotoriOverlay"),
            )]);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 500;
        capture.height = 500;
        capture.rgba = vec![255; 500 * 500 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (_, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(500.), px(500.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(
            point(px(20.), px(20.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(350.), px(300.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let line_button = cx
            .debug_bounds(match kind {
                crate::annotation::ShapeKind::Arrow => "tb-arrow",
                crate::annotation::ShapeKind::Number => "tb-number",
                crate::annotation::ShapeKind::Pencil => "tb-pencil",
                crate::annotation::ShapeKind::Highlighter => "tb-highlighter",
                crate::annotation::ShapeKind::Mosaic => "tb-mosaic",
                crate::annotation::ShapeKind::Eraser => "tb-eraser",
                _ => "tb-line",
            })
            .unwrap();
        cx.simulate_click(line_button.center(), Default::default());
        if kind == crate::annotation::ShapeKind::Number {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|_, cx| {
                session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(40.)))
            });
            cx.simulate_keystrokes("n");
            cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
            cx.simulate_keystrokes("n");
            cx.update(|_, cx| {
                assert_eq!(session.read(cx).annotations().tool(), Some(kind));
                assert_eq!(session.read(cx).annotations().number_size(), 40.);
            });
        }

        if kind == crate::annotation::ShapeKind::Eraser {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("tb-color-0").is_none());
            cx.update(|_, cx| {
                session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(48.)))
            });
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().width(), 48.));
            let rect = cx.debug_bounds("tb-eraser-rect").unwrap();
            cx.simulate_click(rect.center(), Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            // (no size UI in rectangle-eraser mode — same as before)
            cx.simulate_keystrokes("d");
            cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
            cx.simulate_keystrokes("d");
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), Some(kind)));
        }
        if kind == crate::annotation::ShapeKind::Arrow {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_keystrokes("a");
            cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
            cx.simulate_keystrokes("a");
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), Some(kind)));
        }
        cx.simulate_mouse_down(
            point(px(50.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        if matches!(
            kind,
            crate::annotation::ShapeKind::Pencil | crate::annotation::ShapeKind::Highlighter
        ) {
            cx.simulate_mouse_move(
                point(px(90.), px(90.)),
                MouseButton::Left,
                Default::default(),
            );
        }
        cx.simulate_mouse_up(
            point(
                px(180.),
                px(if kind == crate::annotation::ShapeKind::Mosaic {
                    100.
                } else {
                    50.
                }),
            ),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| {
            assert_eq!(
                session
                    .read(cx)
                    .annotations()
                    .visible()
                    .next()
                    .unwrap()
                    .kind,
                kind
            )
        });
        if matches!(
            kind,
            crate::annotation::ShapeKind::Pencil | crate::annotation::ShapeKind::Highlighter
        ) {
            cx.update(|_, cx| {
                let marks = session.read(cx).annotations();
                assert_eq!(
                    marks.visible().next().unwrap().points,
                    vec![
                        point(px(50.), px(50.)),
                        point(px(90.), px(90.)),
                        point(px(180.), px(50.))
                    ]
                );
            });
            let key = if kind == crate::annotation::ShapeKind::Highlighter {
                "h"
            } else {
                "b"
            };
            cx.simulate_keystrokes(key);
            cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
            cx.simulate_keystrokes(key);
        }
        if kind == crate::annotation::ShapeKind::Mosaic {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let blur = cx.debug_bounds("tb-blur").unwrap();
            cx.simulate_click(blur.center(), Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|_, cx| {
                session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(24.)))
            });
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().tool(),
                    Some(crate::annotation::ShapeKind::Blur)
                );
                assert_eq!(session.read(cx).annotations().width(), 24.);
            });
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
            cx.simulate_keystrokes("ctrl-y");
            cx.simulate_keystrokes("m");
            cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        }
        cx.simulate_keystrokes("p");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for (x, y) in [(50., 80.), (100., 150.), (180., 80.)] {
            cx.simulate_click(point(px(x), px(y)), Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let color = cx.debug_bounds("tb-color-3").unwrap();
        cx.simulate_click(color.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let marks = session.read(cx).annotations();
            assert!(!marks.is_drawing_polyline());
            assert_eq!(marks.visible().count(), 2);
            assert_eq!(marks.visible().last().unwrap().points.len(), 3);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 2));
        // A right click commits only confirmed vertices, dropping the preview.
        cx.simulate_click(point(px(60.), px(190.)), Default::default());
        cx.simulate_click(point(px(180.), px(190.)), Default::default());
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position: point(px(250.), px(230.)),
                    ..Default::default()
                }),
                cx,
            );
        });
        cx.simulate_mouse_down(
            point(px(250.), px(230.)),
            MouseButton::Right,
            Default::default(),
        );
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(
                session
                    .read(cx)
                    .annotations()
                    .visible()
                    .last()
                    .unwrap()
                    .points
                    .len(),
                2
            );
        });
        // Double-click finishes once without adding a duplicate endpoint.
        cx.simulate_click(point(px(60.), px(240.)), Default::default());
        cx.simulate_click(point(px(160.), px(240.)), Default::default());
        cx.simulate_mouse_down(
            point(px(160.), px(240.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(160.), px(240.)),
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            let marks = session.read(cx).annotations();
            assert!(!marks.is_drawing_polyline());
            assert_eq!(marks.visible().count(), 4);
            assert_eq!(marks.visible().last().unwrap().points.len(), 2);
        });
        cx.simulate_click(point(px(60.), px(260.)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 4));
    }

    fn geometry_toolbar_keyboard_and_export(
        cx: &mut TestAppContext,
        kind: crate::annotation::ShapeKind,
    ) {
        let is_rectangle = kind == crate::annotation::ShapeKind::Rectangle;
        let key = if is_rectangle { "r" } else { "e" };
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            cx.bind_keys([gpui_kit::KeyBinding::new(
                "escape",
                super::QuitOverlay,
                Some("ShotoriOverlay"),
            )]);
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (_, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(400.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(
            point(px(20.), px(20.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(
            point(px(250.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(250.), px(180.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let button = cx
            .debug_bounds(if is_rectangle {
                "tb-rectangle"
            } else {
                "tb-ellipse"
            })
            .expect("geometry toolbar button");
        cx.simulate_click(button.center(), Default::default());
        cx.update(|_, cx| assert!(session.read(cx).annotations().enabled()));
        let shift = gpui_kit::Modifiers {
            shift: true,
            ..Default::default()
        };
        cx.simulate_mouse_down(
            point(px(50.), px(50.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(point(px(90.), px(110.)), MouseButton::Left, shift);
        cx.simulate_mouse_up(point(px(90.), px(110.)), MouseButton::Left, shift);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let shared = session.read(cx);
            let rectangle = shared.annotations().visible().next().unwrap();
            assert_eq!(rectangle.kind, kind);
            assert_eq!(rectangle.bounds.size, size(px(60.), px(60.)));
            assert_eq!(
                shared.selection().bounds().unwrap().size,
                size(px(230.), px(160.))
            );
            assert_ne!(
                shared.crop("screen").unwrap().2,
                shared.crop_original("screen").unwrap().2
            );
            let painted = window.painted_quads();
            let scale = window.scale_factor();
            for stroke in rectangle.strokes().into_iter().filter(|_| is_rectangle) {
                assert!(
                    painted.iter().any(|quad| {
                        quad.bounds.origin.x.0 == f32::from(stroke.origin.x) * scale
                            && quad.bounds.origin.y.0 == f32::from(stroke.origin.y) * scale
                            && quad.bounds.size.width.0 == f32::from(stroke.size.width) * scale
                            && quad.bounds.size.height.0 == f32::from(stroke.size.height) * scale
                    }),
                    "rectangle stroke {stroke:?} missing from preview: {:?}",
                    painted.iter().map(|q| q.bounds).collect::<Vec<_>>()
                );
            }
        });
        assert!(cx.debug_bounds("tb-undo").is_none());
        assert!(cx.debug_bounds("tb-redo").is_none());
        // Settings clicks must not strand keyboard focus on a transient button.
        // This test changes the next-stroke preset; editing the selected
        // shape would correctly add its own undoable style change.
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.deselect())));
        {
            let button = cx.debug_bounds("tb-color-3").unwrap();
            cx.simulate_click(button.center(), Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
            cx.simulate_keystrokes("ctrl-y");
            cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
        }
        cx.update(|_, cx| {
            // (the old tb-width-2 detent click set this; the detents are
            // gone — set the stroke width through the session instead)
            session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(5.)));
            assert_eq!(session.read(cx).annotations().color().1, "Green");
            assert_eq!(session.read(cx).annotations().width(), 5.);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert!(!session.read(cx).annotations().enabled());
            assert_eq!(session.read(cx).annotations().visible().count(), 1);
        });
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| assert!(session.read(cx).annotations().enabled()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // Even keyboard focus on a settings button must survive that row's
        // removal when the active tool is toggled off.
        cx.simulate_keystrokes("tab tab tab tab tab tab");
        cx.simulate_keystrokes(key);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
        cx.simulate_keystrokes("ctrl-shift-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
    }
}
