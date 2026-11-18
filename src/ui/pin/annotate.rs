//! In-pin secondary canvas annotation backed by the main overlay's
//! [`ScreenshotSession`] pipeline: user-configured row-1 tools (without explicit
//! undo/redo or exit buttons), shared Row-2 option menus/sliders/swatches,
//! inline `TextEditor`, base-local pointer mapping, and right-click tool/mode exit.

use gpui_kit::base::GlobalState;
use gpui_kit::base::input::InputEvent;
use gpui_kit::component::color_picker::ColorPickerState;
use gpui_kit::*;

use crate::annotation::ShapeKind;
use crate::model::placement::{TOOLBAR_GAP, pin_toolbar_size};
use crate::model::session::ScreenshotSession;
use crate::ui::text_editor::TextEditor;
use crate::ui::toolbar::{SizeSlider, u32_to_hsla};

use super::{PinBoard, PinSurface};

/// Active annotation session owned by `PinBoard` for one editing pin.
pub(super) struct PinAnnotSession {
    pub pin_id: u64,
    pub session: Entity<ScreenshotSession>,
    pub pointer_down_active: bool,
    pub _session_sub: Subscription,
}

impl PinBoard {
    /// Id of the pin currently in interactive annotation mode, if any.
    pub(super) fn annotating_id(&self) -> Option<u64> {
        self.annot_session.as_ref().map(|a| a.pin_id)
    }

    /// Choose the single output that hosts the annotation bar for `pin_rect`
    /// (the output with the largest overlap area, avoiding duplicate bars).
    pub(super) fn host_output_for(&self, pin_rect: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        self.outputs
            .iter()
            .copied()
            .filter_map(|out| {
                let inter = pin_rect.intersect(&out);
                let area =
                    f32::from(inter.size.width).max(0.) * f32::from(inter.size.height).max(0.);
                (area > 0.).then_some((area, out))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, out)| out)
    }

    /// Global bounds of the floating annotation toolbar when `annot_session` is active
    /// and `output` is the chosen host output.
    pub(super) fn annot_bar_bounds_on(
        &self,
        output: Bounds<Pixels>,
        cx: &App,
    ) -> Option<Bounds<Pixels>> {
        let annot = self.annot_session.as_ref()?;
        let pin = self.pins.iter().find(|p| p.id == annot.pin_id)?;
        if pin.collapsed.is_some() || pin.ghost || self.hidden {
            return None;
        }
        let host = self.host_output_for(pin.rect)?;
        if host.origin != output.origin {
            return None;
        }
        let settings = crate::model::settings::current();
        let has_options = annot.session.read(cx).annotations().edit_kind().is_some();
        let (pref_w, pref_h) = pin_toolbar_size(has_options, settings);
        let w = pref_w.min((f32::from(output.size.width) - 16.).max(1.));
        let h = pref_h.min((f32::from(output.size.height) - 16.).max(1.));
        let out_l = f32::from(output.left());
        let out_t = f32::from(output.top());
        let out_r = f32::from(output.right());
        let out_b = f32::from(output.bottom());

        let center_x = f32::from(pin.rect.left()) + f32::from(pin.rect.size.width) * 0.5 - w * 0.5;
        let x = center_x.clamp(out_l + 8., (out_r - w - 8.).max(out_l + 8.));
        let below_y = f32::from(pin.rect.bottom()) + TOOLBAR_GAP;
        let above_y = f32::from(pin.rect.top()) - TOOLBAR_GAP - h;
        let y = if below_y + h + 12. <= out_b {
            below_y
        } else if above_y >= out_t + 8. {
            above_y
        } else {
            (out_b - h - 8.).max(out_t + 8.)
        };
        Some(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
    }

    pub(super) fn toggle_annotate(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        self.menu = None;
        self.drag = None;
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        } else {
            self.enter_annotate(id, cx);
        }
    }

    pub(super) fn enter_annotate(&mut self, id: u64, cx: &mut Context<Self>) {
        self.exit_annotate(cx);
        let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if let Some(collapsed) = pin.collapsed.take() {
            pin.rect = crate::model::pin::expand_bounds(pin.rect, collapsed, &self.outputs);
        }
        pin.ghost = false;
        let mut annotations = std::mem::take(&mut pin.annotations);
        if annotations.tool().is_none() {
            let initial = crate::ui::toolbar::initial_pin_tool(crate::model::settings::current());
            annotations.toggle(initial);
        }
        let (w, h, ref orig) = *pin.base_crop;
        let orig_rgba = orig.clone();
        let base_size = pin.base;
        let session =
            cx.new(|_| ScreenshotSession::for_pin(w, h, orig_rgba, base_size, annotations));
        let sub = cx.observe(&session, |board, _, cx| {
            board.sync_annot_visuals_from_session(cx);
        });
        self.annot_session = Some(PinAnnotSession {
            pin_id: id,
            session,
            pointer_down_active: false,
            _session_sub: sub,
        });
        self.raise(id);
        self.show_hud(id, crate::i18n::t!("pin.hud.annotating").into_owned(), cx);
        cx.notify();
    }

    /// Leave pin annotation mode, moving edited annotations back onto the pin
    /// and flushing updated visuals and disk history.
    pub(super) fn exit_annotate(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(annot) = self.annot_session.take() else {
            return false;
        };
        let pin_id = annot.pin_id;
        let session = annot.session;
        drop(annot._session_sub);
        let annotations = session.update(cx, |s, _| s.take_annotations());
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
            pin.annotations = annotations;
            pin.dirty_raster = true;
            pin.rebuild_visuals();
            self.sync_pin_to_history(pin_id);
        }
        cx.notify();
        true
    }

    pub(super) fn sync_annot_visuals_from_session(&mut self, cx: &mut Context<Self>) {
        let Some(annot) = self.annot_session.as_ref() else {
            return;
        };
        let pin_id = annot.pin_id;
        let session = annot.session.clone();
        let (live, is_dragging_complete) = {
            let s = session.read(cx);
            let live = s.is_body_moving()
                || s.handle_drag_anchor().is_some()
                || s.annotations().is_pressed()
                || s.annotations().is_drawing_polyline()
                || s.annotations().has_text_preview();
            (live, !live)
        };
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
            let annotations = session.read(cx).annotations();
            if live {
                pin.rebuild_live_preview_from(annotations);
            } else {
                pin.dirty_raster = true;
                pin.rebuild_visuals_from(annotations);
            }
        }
        if is_dragging_complete {
            self.sync_pin_to_history(pin_id);
        }
        cx.notify();
    }

    pub(super) fn flush_annot_visuals(&mut self, cx: &App) {
        let Some(annot) = self.annot_session.as_ref() else {
            return;
        };
        let pin_id = annot.pin_id;
        let session = annot.session.clone();
        let annotations = session.read(cx).annotations();
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
            pin.dirty_raster = true;
            pin.rebuild_visuals_from(annotations);
        }
    }

    pub(super) fn annot_pointer_move(
        &mut self,
        global: Point<Pixels>,
        shift: bool,
        alt: bool,
        cx: &mut App,
    ) -> bool {
        let Some(annot) = self.annot_session.as_ref() else {
            return false;
        };
        let pin_id = annot.pin_id;
        let pointer_down = annot.pointer_down_active;
        let session = annot.session.clone();
        let Some(pin) = self.pins.iter().find(|p| p.id == pin_id) else {
            return false;
        };
        let base_pt = pin.global_to_base_unclamped(global);
        let moved = session.update(cx, |s, cx| {
            let changed = s.pointer_move_with("pin", base_pt, shift, alt);
            if changed || s.eraser_ring_follows_pointer() {
                cx.notify();
            }
            changed
        });
        moved || pointer_down
    }

    pub(super) fn annot_pointer_up(
        &mut self,
        global: Point<Pixels>,
        shift: bool,
        alt: bool,
        cx: &mut App,
    ) -> bool {
        let Some(annot) = self.annot_session.as_mut() else {
            return false;
        };
        if !annot.pointer_down_active {
            return false;
        }
        annot.pointer_down_active = false;
        let pin_id = annot.pin_id;
        let session = annot.session.clone();
        let Some(pin) = self.pins.iter().find(|p| p.id == pin_id) else {
            return false;
        };
        let base_pt = pin.global_to_base_unclamped(global);
        session.update(cx, |s, cx| {
            s.pointer_up_with("pin", base_pt, shift, alt);
            cx.notify();
        });
        self.sync_pin_to_history(pin_id);
        true
    }
}

impl PinSurface {
    /// Observe the active pin's `ScreenshotSession` for text style edits.
    pub(super) fn ensure_annot_session_observer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_session = self
            .board
            .read(cx)
            .annot_session
            .as_ref()
            .map(|a| (a.pin_id, a.session.clone()));
        let Some((pin_id, session)) = current_session else {
            self.annot_session_sub = None;
            return;
        };
        let sid = session.entity_id();
        if self
            .annot_session_sub
            .as_ref()
            .is_some_and(|(existing, _)| *existing == sid)
        {
            return;
        }
        let sub = cx.observe_in(&session, window, move |this, session, _window, cx| {
            if let Some(editor) = &mut this.text_editing
                && this.number_edit.is_none()
            {
                let zoom = this
                    .board
                    .read(cx)
                    .pins
                    .iter()
                    .find(|p| p.id == pin_id)
                    .map(|p| p.zoom_factor())
                    .unwrap_or(1.);
                let a = session.read(cx).annotations();
                let style = a.text_style().clone();
                if editor.sync_style(a.text_size() * zoom, a.color().0, &style, cx) {
                    let accepted_size = editor.font_size / zoom;
                    let accepted_style = editor.style.clone();
                    session.update(cx, |s, cx| {
                        s.edit_annotation_settings(|a| {
                            a.apply_size(accepted_size);
                            a.set_text_style(accepted_style);
                        });
                        cx.notify();
                    });
                    this.refresh_pin_text(cx);
                }
            }
            cx.notify();
        });
        self.annot_session_sub = Some((sid, sub));
    }

    pub(super) fn active_annot_session(
        &self,
        cx: &App,
    ) -> Option<(u64, Entity<ScreenshotSession>)> {
        self.board
            .read(cx)
            .annot_session
            .as_ref()
            .map(|a| (a.pin_id, a.session.clone()))
    }

    /// Seed the inline swatch color-picker state for Row 2 without emitting events.
    pub(super) fn annot_swatch_editor_seed(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ColorPickerState>> {
        let picker = self.swatch_picker.clone()?;
        let (_, session) = self.active_annot_session(cx)?;
        let target = u32_to_hsla(session.read(cx).annotations().color().0);
        let stale = {
            let state = picker.read(cx);
            !state.is_open() && state.value() != Some(target)
        };
        if stale {
            picker.update(cx, |state, cx| state.set_value(target, window, cx));
        }
        Some(picker)
    }

    /// Build or synchronize Row 2's continuous size slider against the active session.
    pub(super) fn annot_size_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SizeSlider> {
        let (_, session) = self.active_annot_session(cx)?;
        let edit = {
            let a = session.read(cx).annotations();
            a.edit_kind().map(|kind| {
                (
                    (kind, a.editing_filter_brush_size()),
                    a.current_size_spec(),
                    a.current_edit_size(),
                )
            })
        };
        edit.map(|(kind, spec, current)| {
            let reusable = match self.slider.as_ref() {
                Some((k, state)) if *k == kind => Some(state.clone()),
                _ => None,
            };
            let state = if let Some(state) = reusable {
                if (state.read(cx).value().start() - current).abs() > f32::EPSILON {
                    state.update(cx, |s, cx| s.set_value(current, window, cx));
                }
                state
            } else {
                let state = cx.new(|_| {
                    gpui_kit::base::slider::SliderState::new()
                        .min(spec.min)
                        .max(spec.max)
                        .step(1.)
                        .default_value(current)
                });
                let board = self.board.clone();
                let sub = cx.subscribe(
                    &state,
                    move |_, _, event: &gpui_kit::base::slider::SliderEvent, cx| {
                        let Some(sess) = board
                            .read(cx)
                            .annot_session
                            .as_ref()
                            .map(|a| a.session.clone())
                        else {
                            return;
                        };
                        match event {
                            gpui_kit::base::slider::SliderEvent::Change(v) => {
                                sess.update(cx, |s, cx| {
                                    s.edit_annotation_settings(|a| a.apply_size(v.start()));
                                    cx.notify();
                                });
                            }
                            gpui_kit::base::slider::SliderEvent::Release(_) => {
                                sess.update(cx, |s, _| {
                                    s.edit_annotation_settings(|a| a.end_size_drag());
                                });
                            }
                        }
                    },
                );
                self.slider_sub = Some(sub);
                self.slider = Some((kind, state.clone()));
                state
            };
            let percentage = state.read(cx).percentage().end;
            SizeSlider {
                state,
                percentage,
                current,
            }
        })
    }

    pub(super) fn toggle_pin_tool(
        &mut self,
        kind: ShapeKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_pin_text(true, window, cx);
        window.focus(&self.focus_handle, cx);
        let Some((_, session)) = self.active_annot_session(cx) else {
            return;
        };
        session.update(cx, |s, cx| {
            s.toggle_annotation_tool(kind);
            cx.notify();
        });
    }

    pub(super) fn handle_annot_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_editing.is_some() {
            let commit = self.number_edit.is_none();
            self.finish_pin_text(commit, window, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some((_, session)) = self.active_annot_session(cx) else {
            return;
        };
        let interrupted = session.update(cx, |s, cx| {
            let interrupted = s.escape_interaction();
            cx.notify();
            interrupted
        });
        if !interrupted {
            self.board.update(cx, |b, cx| {
                b.exit_annotate(cx);
            });
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Right-click while annotating: never open the pin context menu; instead
    /// mirror the main overlay's exit flow (commit text -> finish polyline /
    /// exit active tool & selection -> exit pin annotation mode).
    pub(super) fn handle_annot_right_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if GlobalState::is_in_deferred_context(cx) {
            return;
        }
        if self.text_editing.is_some() {
            self.finish_pin_text(true, window, cx);
            return;
        }
        window.focus(&self.focus_handle, cx);
        let Some((_, session)) = self.active_annot_session(cx) else {
            return;
        };
        let had_active_tool_or_selection = {
            let a = session.read(cx).annotations();
            a.tool().is_some() || a.selected_index().is_some() || a.is_drawing_polyline()
        };
        if had_active_tool_or_selection {
            session.update(cx, |s, cx| {
                s.finish_or_exit_annotation();
                cx.notify();
            });
        } else {
            self.board.update(cx, |b, cx| {
                b.exit_annotate(cx);
            });
        }
    }

    /// Handle a left press inside the annotating pin. Returns `true` if consumed
    /// by annotation interaction; `false` if the caller should fall back to
    /// dragging the pin (no active tool and empty canvas hit).
    pub(super) fn handle_annot_mouse_down(
        &mut self,
        pin_id: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let global = event.position + self.output.bounds.origin;
        self.finish_pin_text(true, window, cx);

        let Some((base_pt, pin_local, zoom, session)) = self
            .board
            .read(cx)
            .annot_session
            .as_ref()
            .filter(|a| a.pin_id == pin_id)
            .and_then(|a| {
                let pin = self.board.read(cx).pins.iter().find(|p| p.id == pin_id)?;
                Some((
                    pin.global_to_base_unclamped(global),
                    pin.rect.origin - self.output.bounds.origin,
                    pin.zoom_factor(),
                    a.session.clone(),
                ))
            })
        else {
            return false;
        };

        let tool = session.read(cx).annotations().tool();
        if event.click_count >= 2
            && matches!(tool, None | Some(ShapeKind::Select | ShapeKind::Text))
        {
            let hit_text = session
                .read(cx)
                .annotations()
                .hit_test(base_pt)
                .and_then(|ix| {
                    (session.read(cx).annotations().shape_kind(ix) == Some(ShapeKind::Text))
                        .then_some(ix)
                });
            if let Some(ix) = hit_text {
                self.start_pin_edit_text(pin_id, ix, base_pt, pin_local, zoom, window, cx);
                return true;
            }
        }

        if tool == Some(ShapeKind::Text)
            && session
                .read(cx)
                .annotations()
                .interaction_hit_test(base_pt)
                .is_none()
        {
            self.start_pin_text(pin_id, base_pt, pin_local, zoom, window, cx);
            return true;
        }

        if event.click_count == 2 {
            let straightened = session.update(cx, |s, cx| {
                let ok = s.straighten_at_double_click("pin", base_pt);
                if ok {
                    cx.notify();
                }
                ok
            });
            if straightened {
                return true;
            }
        }

        if event.click_count == 2
            && let Some((ix, badge_base_origin)) =
                session.read(cx).number_at_double_click("pin", base_pt)
        {
            self.start_pin_number_edit(pin_id, ix, badge_base_origin, pin_local, zoom, window, cx);
            return true;
        }

        if session
            .read(cx)
            .pin_consumes_press(base_pt, event.modifiers.shift)
        {
            self.board.update(cx, |b, _| {
                if let Some(annot) = &mut b.annot_session {
                    annot.pointer_down_active = true;
                }
            });
            session.update(cx, |s, cx| {
                s.pointer_down("pin", base_pt, event.modifiers.alt, event.modifiers.shift);
                cx.notify();
            });
            return true;
        }

        if session.read(cx).annotations().selected_count() > 0 {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.deselect());
                cx.notify();
            });
        }
        false
    }

    fn subscribe_pin_text(
        &mut self,
        input: &Entity<crate::ui::text_input::TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text_subscription =
            Some(
                cx.subscribe_in(input, window, |this, _, event, window, cx| {
                    if matches!(event, gpui_kit::base::input::InputEvent::Change) {
                        this.refresh_pin_text(cx);
                    }
                    if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                        this.finish_pin_text(true, window, cx);
                    }
                }),
            );
    }

    fn start_pin_text(
        &mut self,
        pin_id: u64,
        base_pt: Point<Pixels>,
        pin_local: Point<Pixels>,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, session)) = self
            .active_annot_session(cx)
            .filter(|(id, _)| *id == pin_id)
        else {
            return;
        };
        session.update(cx, |s, _| {
            s.edit_annotations(|a| a.deselect());
        });
        let font_size = session.read(cx).annotations().text_size();
        let Some(bounds_base) = session.read(cx).text_bounds("pin", base_pt, font_size) else {
            return;
        };
        let color = session.read(cx).annotations().color().0;
        let style = session.read(cx).annotations().text_style().clone();
        let scaled_bounds = Bounds::new(
            bounds_base.origin,
            size(
                px(f32::from(bounds_base.size.width) * zoom),
                px(f32::from(bounds_base.size.height) * zoom),
            ),
        );
        let editor_local = pin_local
            + point(
                px(f32::from(bounds_base.origin.x) * zoom),
                px(f32::from(bounds_base.origin.y) * zoom),
            );
        let mut editor = TextEditor::new(scaled_bounds, editor_local, font_size * zoom, color, cx);
        editor.sync_style(font_size * zoom, color, &style, cx);
        self.subscribe_pin_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.refresh_pin_text(cx);
        session.update(cx, |s, cx| {
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

    #[allow(clippy::too_many_arguments)]
    fn start_pin_number_edit(
        &mut self,
        pin_id: u64,
        ix: usize,
        badge_base_origin: Point<Pixels>,
        pin_local: Point<Pixels>,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, session)) = self
            .active_annot_session(cx)
            .filter(|(id, _)| *id == pin_id)
        else {
            return;
        };
        let Some(shape) = session.read(cx).annotations().committed().get(ix).cloned() else {
            return;
        };
        let diameter = f32::from(shape.bounds.size.width);
        let scaled_bounds = Bounds::new(
            shape.bounds.origin,
            size(
                px(f32::from(shape.bounds.size.width) * zoom),
                px(f32::from(shape.bounds.size.height) * zoom),
            ),
        );
        let editor_local = pin_local
            + point(
                px(f32::from(badge_base_origin.x) * zoom),
                px(f32::from(badge_base_origin.y) * zoom),
            );
        let editor = TextEditor::new_with_text(
            scaled_bounds,
            editor_local,
            diameter * 0.55 * zoom,
            shape.color,
            &shape.number.unwrap_or(1).to_string(),
            None,
            cx,
        );
        self.subscribe_pin_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.number_edit = Some((ix, shape));
        session.update(cx, |s, cx| {
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

    #[allow(clippy::too_many_arguments)]
    fn start_pin_edit_text(
        &mut self,
        pin_id: u64,
        ix: usize,
        click_base: Point<Pixels>,
        pin_local: Point<Pixels>,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, session)) = self
            .active_annot_session(cx)
            .filter(|(id, _)| *id == pin_id)
        else {
            return;
        };
        let (shape, available_base, unrotated_click_scaled) = {
            let s = session.read(cx);
            let Some(shape) = s.annotations().shape(ix).cloned() else {
                return;
            };
            let unrotated_click = shape.rotated_point(click_base, true);
            let Some(available) = s.text_bounds("pin", shape.bounds.origin, shape.size()) else {
                return;
            };
            let click_scaled = pin_local
                + point(
                    px(f32::from(unrotated_click.x) * zoom),
                    px(f32::from(unrotated_click.y) * zoom),
                );
            (shape, available, click_scaled)
        };
        let text = shape.text.as_deref().unwrap_or("");
        let font_size = shape.size();
        let color = shape.color;
        let scaled_bounds = Bounds::new(
            available_base.origin,
            size(
                px(f32::from(available_base.size.width) * zoom),
                px(f32::from(available_base.size.height) * zoom),
            ),
        );
        let editor_local = pin_local
            + point(
                px(f32::from(available_base.origin.x) * zoom),
                px(f32::from(available_base.origin.y) * zoom),
            );
        let mut editor = TextEditor::new_with_text(
            scaled_bounds,
            editor_local,
            font_size * zoom,
            color,
            text,
            None,
            cx,
        );
        let mut upright_style = shape.text_style.clone();
        upright_style.rotation = 0.;
        editor.sync_style(font_size * zoom, color, &upright_style, cx);
        editor.place_caret(unrotated_click_scaled, cx);
        let started = session.update(cx, |s, cx| {
            let ok = s.begin_text_edit(ix);
            if ok {
                cx.notify();
            }
            ok
        });
        if !started {
            return;
        }
        self.subscribe_pin_text(editor.input(), window, cx);
        self.text_editing = Some(editor);
        self.refresh_pin_text(cx);
        cx.defer_in(window, |this, window, cx| {
            if let Some(editor) = &this.text_editing {
                editor.focus(window, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn refresh_pin_text(&mut self, cx: &mut Context<Self>) {
        let Some((pin_id, session)) = self.active_annot_session(cx) else {
            return;
        };
        let zoom = self
            .board
            .read(cx)
            .pins
            .iter()
            .find(|p| p.id == pin_id)
            .map(|p| p.zoom_factor())
            .unwrap_or(1.);
        let Some(editor) = &mut self.text_editing else {
            return;
        };
        editor.refresh(cx);
        let value = editor.value(cx);
        if let Some((ix, _)) = &self.number_edit {
            if let Ok(v) = value.trim().parse::<u32>() {
                session.update(cx, |s, cx| {
                    s.preview_number_edit(*ix, v);
                    cx.notify();
                });
            }
        } else {
            let mut bounds = editor.actual_bounds();
            bounds.size = size(
                px((f32::from(bounds.size.width) / zoom).max(1.)),
                px((f32::from(bounds.size.height) / zoom).max(1.)),
            );
            session.update(cx, |s, cx| {
                s.preview_text(bounds, value);
                cx.notify();
            });
        }
        cx.notify();
    }

    pub(super) fn finish_pin_text(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.text_editing.take() else {
            return;
        };
        self.text_subscription.take();
        editor
            .input()
            .update(cx, |input, cx| input.discard_preedit(cx));
        let Some((pin_id, session)) = self.active_annot_session(cx) else {
            self.number_edit = None;
            return;
        };
        let zoom = self
            .board
            .read(cx)
            .pins
            .iter()
            .find(|p| p.id == pin_id)
            .map(|p| p.zoom_factor())
            .unwrap_or(1.);
        if let Some((ix, before)) = self.number_edit.take() {
            let value = editor.value(cx);
            let parsed = commit
                .then(|| value.trim().parse::<u32>().ok())
                .flatten()
                .filter(|v| *v > 0);
            session.update(cx, |s, cx| {
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
            let mut bounds = editor.actual_bounds();
            bounds.size = size(
                px((f32::from(bounds.size.width) / zoom).max(1.)),
                px((f32::from(bounds.size.height) / zoom).max(1.)),
            );
            let value = editor.value(cx);
            session.update(cx, |s, cx| {
                s.preview_text(bounds, value);
                s.finish_text_edit(commit);
                cx.notify();
            });
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
}
