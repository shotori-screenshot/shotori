//! # Screenshot overlay: assembly layer for frozen-screen + selection interaction
//!
//! Flow: frozen frame as the base (img) → drag a selection (the selection
//! "sees through", everything around it dims) → Enter copies to clipboard /
//! Ctrl+S saves PNG / Ctrl+O OCR / Esc exits.
//!
//! Division of labor: pure logic lives in [`crate::model::selection`] (state
//! machine) and [`crate::model::export`] (crop/encode), visuals in [`crate::ui::hud`]
//! and [`crate::ui::toolbar`] — this module only does gpui assembly:
//! window, events → state-machine calls, state → rendering. The impl is
//! split by concern: struct + lifecycle here, text editing in
//! [`text_edit`], the four exits in [`exits`], rendering and the
//! window-level pointer sink in [`render`].

use std::sync::Arc;

#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::model::selection::{PressTarget, Selection};
use crate::platform::capture::Capture;
use crate::ui::hud::handle_cursor;
use crate::ui::image_util;
use crate::ui::toolbar::{hsla_to_u32, u32_to_hsla};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};

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
    /// The inline swatch editor's state (see [`Overlay::new`]); None
    /// only in hand-built test instances, where the selected cell
    /// degrades to a plain swatch.
    swatch_picker: Option<Entity<gpui_kit::component::color_picker::ColorPickerState>>,
    /// The toolbar's size slider, rebuilt when the active tool (its
    /// spec) changes; `SliderEvent::Change` writes through to the
    /// session. UI state, never crosses to the model.
    slider: Option<(
        (crate::annotation::ShapeKind, bool),
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
        // The inline swatch editor's popover (gpui-component) must
        // follow the shotori palette, not the component default —
        // same push the settings window does at startup.
        crate::ui::settings::sync_shell_theme(cx);
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
            crate::ui::e2e::debug_editing_scene(debug_targeted, session, &capture.output_name);
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
            text_editing: None,
            text_subscription: None,
            number_edit: None,
            cursor: std::rc::Rc::new(std::cell::Cell::new(CursorStyle::Crosshair)),
            perf_first_render_pending: true,
            slider: None,
            slider_sub: None,
            size_scroll_acc: 0.0,
            swatch_picker: None,
        };
        overlay.attach_observers(window, cx);
        // The inline swatch editor: the SELECTED color cell in row two
        // renders as this picker; its popover (preset palette + HSLA
        // sliders + hex) edits the board through
        // `settings::set_annotation_color`, which also persists. The
        // state must live on the Overlay (a render-built picker would
        // reset the popover on every frame).
        let swatch_picker = cx.new(|cx| {
            let seed = overlay.session.read(cx).annotations().color().0;
            ColorPickerState::new(window, cx).default_value(u32_to_hsla(seed))
        });
        overlay.swatch_picker = Some(swatch_picker.clone());
        let sub = cx.subscribe(&swatch_picker, |this, _, event, cx| {
            if let ColorPickerEvent::Change(Some(hsla)) = event {
                // The edit lands on the ACTIVE swatch index — the
                // picker renders exactly there — and writes through
                // to settings.toml; the repaint picks the new color
                // up everywhere.
                let ix = this.session.read(cx).annotations().color_index();
                crate::model::settings::set_annotation_color(ix, hsla_to_u32(*hsla));
                cx.notify();
            }
        });
        overlay._subscriptions.push(sub);
        overlay
    }

    /// The inline swatch editor's state, seeded on the active color.
    /// While the popover is CLOSED the value tracks the active swatch
    /// (a switch must re-seed it, or the next open would edit the
    /// previous color); while OPEN it is the user's working draft and
    /// is left alone. `set_value` emits nothing, so this cannot loop.
    fn swatch_editor_seed(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<gpui_kit::component::color_picker::ColorPickerState>> {
        let picker = self.swatch_picker.clone()?;
        let target = u32_to_hsla(self.session.read(cx).annotations().color().0);
        let stale = {
            let state = picker.read(cx);
            !state.is_open() && state.value() != Some(target)
        };
        if stale {
            picker.update(cx, |state, cx| state.set_value(target, window, cx));
        }
        Some(picker)
    }

    fn attach_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions = vec![
            cx.on_focus_lost(window, |this, window, cx| {
                // Restore only when the focused control leaves the tree. Session
                // notifications also arrive while typing in toolbar inputs.
                if this.ocr_setup.is_none()
                    && !gpui_kit::base::GlobalState::is_in_deferred_context(cx)
                {
                    if let Some(editor) = &this.text_editing {
                        editor.focus(window, cx);
                    } else {
                        window.focus(&this.focus_handle, cx);
                    }
                }
            }),
            window.observe_window_appearance(|_, cx| {
                // a system appearance flip can reset the component design
                // system — re-align it with the active palette (the
                // shotori palette itself is fixed and ignores the flip)
                crate::ui::settings::sync_shell_theme(cx);
                cx.refresh_windows();
            }),
            cx.observe_in(&self.session, window, |this, _, _window, cx| {
                if let Some(editor) = &mut this.text_editing
                    && this.number_edit.is_none()
                {
                    let a = this.session.read(cx).annotations();
                    let style = a.text_style().clone();
                    if editor.sync_style(a.text_size(), a.color().0, &style, cx) {
                        let accepted_size = editor.font_size;
                        let accepted_style = editor.style.clone();
                        this.session.update(cx, |s, cx| {
                            s.edit_annotation_settings(|a| {
                                a.apply_size(accepted_size);
                                a.set_text_style(accepted_style);
                            });
                            cx.notify();
                        });
                        this.refresh_text(cx);
                    }
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
}

/// The cursor affordance of an annotation handle, shared by its hover
/// and drag states: corner handles resize along their diagonal, point
/// handles (line endpoints, polyline vertices) reposition — plain
/// arrow, grabbing feedback would add nothing there.
fn annotation_handle_cursor(kind: crate::annotation::ShapeKind, anchor: usize) -> CursorStyle {
    if anchor == crate::annotation::ROTATION_HANDLE || anchor >= 8 {
        CursorStyle::Crosshair
    } else if kind.is_region() {
        match anchor {
            0 | 2 => CursorStyle::ResizeUpLeftDownRight,
            4 | 6 => CursorStyle::ResizeUpDown,
            5 | 7 => CursorStyle::ResizeLeftRight,
            _ => CursorStyle::ResizeUpRightDownLeft,
        }
    } else {
        CursorStyle::Arrow
    }
}

mod exits;
#[cfg(test)]
mod multi_output_tests;
mod render;
mod text_edit;
