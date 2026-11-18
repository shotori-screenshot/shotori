//! In-pin secondary canvas annotation: pointer mapping onto base-local
//! coordinates, live/committed rasterization, inline text prompt, and the
//! floating two-row annotation toolbar.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::base::input::InputEvent;
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::actions::{
    ExitPinAnnotate, PinClearMarks, PinDeleteMark, PinRedo, PinSizeDown, PinSizeUp, PinToolArrow,
    PinToolEllipse, PinToolEraser, PinToolHighlighter, PinToolLine, PinToolMosaic, PinToolNumber,
    PinToolPencil, PinToolRect, PinToolSelect, PinToolText, PinUndo,
};
use crate::annotation::ShapeKind;
use crate::model::settings::PALETTE;
use crate::ui::text_input::TextInput;
use crate::ui::theme;

use super::{PinBoard, PinSurface};

pub(super) const ANNOT_BAR_W: f32 = 468.;
pub(super) const ANNOT_BAR_H: f32 = 72.;
const ANNOT_GAP: f32 = 6.;
const TEXT_PROMPT_W: f32 = 280.;
const TEXT_PROMPT_H: f32 = 38.;

/// Active pointer gesture inside an annotating pin's base-local space.
#[derive(Clone)]
pub(super) enum PinAnnotGesture {
    /// Drawing a new mark (`Annotations::begin` / `drag_to` / `end`).
    Draw { pin_id: u64 },
    /// Dragging an existing selected mark in Select mode.
    MoveMark {
        pin_id: u64,
        ix: usize,
        start_base: Point<Pixels>,
        before: crate::annotation::Shape,
    },
    /// Dragging a handle on the selected mark.
    MoveHandle {
        pin_id: u64,
        ix: usize,
        anchor: usize,
        before: crate::annotation::Shape,
    },
}

/// Layer-shell-safe inline text entry box anchored over an annotating pin.
pub(super) struct PinTextPrompt {
    pub pin_id: u64,
    /// Insertion anchor in the pin's base-local logical space.
    pub base_anchor: Point<Pixels>,
    /// Desktop-global bounds of the floating input pill.
    pub global_bounds: Bounds<Pixels>,
    pub input: Entity<TextInput>,
    pub _sub: Subscription,
}

impl PinBoard {
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

    /// Global bounds of the floating annotation toolbar when `annotating` is active
    /// and `output` is the chosen host output.
    pub(super) fn annot_bar_bounds_on(&self, output: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let id = self.annotating?;
        let pin = self.pins.iter().find(|p| p.id == id)?;
        if pin.collapsed.is_some() || pin.ghost || self.hidden {
            return None;
        }
        let host = self.host_output_for(pin.rect)?;
        if host.origin != output.origin {
            return None;
        }
        let w = ANNOT_BAR_W.min((f32::from(output.size.width) - 12.).max(120.));
        let h = ANNOT_BAR_H.min((f32::from(output.size.height) - 12.).max(36.));
        let out_l = f32::from(output.left());
        let out_t = f32::from(output.top());
        let out_r = f32::from(output.right());
        let out_b = f32::from(output.bottom());

        let x = f32::from(pin.rect.left()).clamp(out_l + 6., (out_r - w - 6.).max(out_l + 6.));
        let below_y = f32::from(pin.rect.bottom()) + ANNOT_GAP;
        let above_y = f32::from(pin.rect.top()) - ANNOT_GAP - h;
        let y = if below_y + h <= out_b - 6. {
            below_y
        } else if above_y >= out_t + 6. {
            above_y
        } else {
            (out_b - h - 8.).max(out_t + 6.)
        };
        Some(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
    }

    pub(super) fn toggle_annotate(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        self.menu = None;
        self.drag = None;
        self.annot_gesture = None;
        self.text_prompt = None;
        if self.annotating == Some(id) {
            self.finish_pin_annotation_state(id);
            self.annotating = None;
        } else {
            if let Some(prev) = self.annotating.take() {
                self.finish_pin_annotation_state(prev);
            }
            if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
                if let Some(collapsed) = pin.collapsed.take() {
                    pin.rect = crate::model::pin::expand_bounds(pin.rect, collapsed, &self.outputs);
                }
                pin.ghost = false;
                if pin.annotations.tool().is_none() {
                    pin.annotations.toggle(ShapeKind::Rectangle);
                }
            }
            self.raise(id);
            self.annotating = Some(id);
            self.show_hud(id, crate::i18n::t!("pin.hud.annotating").into_owned(), cx);
        }
        cx.notify();
    }

    pub(super) fn exit_annotate_step(&mut self, cx: &mut Context<Self>) {
        if self.text_prompt.take().is_some() {
            cx.notify();
            return;
        }
        self.annot_gesture = None;
        let Some(id) = self.annotating else {
            return;
        };
        let cancelled_inner = self
            .pins
            .iter_mut()
            .find(|p| p.id == id)
            .is_some_and(|pin| {
                if pin.annotations.selected_count() > 0
                    && !pin.annotations.is_pressed()
                    && pin.annotations.draft_shape().is_none()
                {
                    pin.annotations.deselect();
                    true
                } else if pin.annotations.finish_interaction() {
                    pin.rebuild_visuals();
                    true
                } else {
                    false
                }
            });
        if !cancelled_inner {
            self.finish_pin_annotation_state(id);
            self.annotating = None;
        }
        cx.notify();
    }

    pub(super) fn finish_pin_annotation_state(&mut self, id: u64) {
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.annotations.finish_polyline();
            let _ = pin.annotations.cancel();
            pin.annotations.deselect();
            pin.rebuild_visuals();
            self.sync_pin_to_history(id);
        }
    }

    pub(super) fn select_annot_tool(&mut self, kind: ShapeKind, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        self.text_prompt = None;
        self.annot_gesture = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.annotations.finish_interaction();
            pin.annotations.toggle(kind);
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn set_annot_color(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.annotations.set_color(ix);
            pin.dirty_raster = true;
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn step_annot_size(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            let area = Bounds::new(point(px(0.), px(0.)), pin.base);
            let mut changed = false;
            pin.annotations.edit_within(area, |a| {
                changed = a.step_size(up);
            });
            if changed {
                pin.dirty_raster = true;
                pin.rebuild_visuals();
            }
        }
        cx.notify();
    }

    pub(super) fn annot_undo(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        self.annot_gesture = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            let _ = pin.annotations.undo();
            pin.dirty_raster = true;
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn annot_redo(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        self.annot_gesture = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            let _ = pin.annotations.redo();
            pin.dirty_raster = true;
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn annot_delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        self.annot_gesture = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id)
            && pin.annotations.delete_selected()
        {
            pin.dirty_raster = true;
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn annot_clear(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.annotating else {
            return;
        };
        self.annot_gesture = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id)
            && pin.annotations.clear_all()
        {
            pin.dirty_raster = true;
            pin.rebuild_visuals();
        }
        cx.notify();
    }

    pub(super) fn annot_pointer_move(&mut self, global: Point<Pixels>, shift: bool) -> bool {
        let Some(gesture) = self.annot_gesture.clone() else {
            return false;
        };
        match gesture {
            PinAnnotGesture::Draw { pin_id } => {
                if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
                    let base_pt = pin.global_to_base(global);
                    let area = Bounds::new(point(px(0.), px(0.)), pin.base);
                    pin.annotations.drag_to(base_pt, area, shift);
                    pin.rebuild_live_preview();
                    return true;
                }
            }
            PinAnnotGesture::MoveMark {
                pin_id,
                ix,
                start_base,
                before,
            } => {
                if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
                    let base_pt = pin.global_to_base(global);
                    let raw_delta = base_pt - start_base;
                    let box_before = before.selection_box();
                    let min_dx = -f32::from(box_before.left());
                    let max_dx = f32::from(pin.base.width - box_before.right());
                    let min_dy = -f32::from(box_before.top());
                    let max_dy = f32::from(pin.base.height - box_before.bottom());
                    let clamped = point(
                        px(crate::model::pin::clamp_to(
                            f32::from(raw_delta.x),
                            min_dx,
                            max_dx,
                        )),
                        px(crate::model::pin::clamp_to(
                            f32::from(raw_delta.y),
                            min_dy,
                            max_dy,
                        )),
                    );
                    pin.annotations.place_shape(ix, &before, clamped);
                    pin.dirty_raster = true;
                    pin.rebuild_visuals();
                    return true;
                }
            }
            PinAnnotGesture::MoveHandle {
                pin_id,
                ix,
                anchor,
                before,
            } => {
                if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
                    let base_pt = pin.global_to_base(global);
                    let area = Bounds::new(point(px(0.), px(0.)), pin.base);
                    pin.annotations
                        .place_handle_with(ix, anchor, &before, base_pt, area, shift);
                    pin.dirty_raster = true;
                    pin.rebuild_visuals();
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn annot_pointer_up(&mut self, global: Point<Pixels>, shift: bool) -> bool {
        let _ = self.annot_pointer_move(global, shift);
        let Some(gesture) = self.annot_gesture.take() else {
            return false;
        };
        match gesture {
            PinAnnotGesture::Draw { pin_id } => {
                if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
                    pin.annotations.end();
                    pin.dirty_raster = true;
                    pin.rebuild_visuals();
                    self.sync_pin_to_history(pin_id);
                }
            }
            PinAnnotGesture::MoveMark {
                pin_id, ix, before, ..
            }
            | PinAnnotGesture::MoveHandle {
                pin_id, ix, before, ..
            } => {
                if let Some(pin) = self.pins.iter_mut().find(|p| p.id == pin_id) {
                    pin.annotations.commit_move(ix, before);
                    pin.dirty_raster = true;
                    pin.rebuild_visuals();
                    self.sync_pin_to_history(pin_id);
                }
            }
        }
        true
    }
}

impl PinSurface {
    pub(super) fn handle_annot_mouse_down(
        &mut self,
        pin_id: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let global = event.position + self.output.bounds.origin;
        // Commit any open text prompt before starting a new stroke.
        self.commit_pin_text_prompt(window, cx);

        let tool_is_text = self
            .board
            .read(cx)
            .pins
            .iter()
            .find(|p| p.id == pin_id)
            .is_some_and(|p| p.annotations.tool() == Some(ShapeKind::Text));

        if tool_is_text {
            self.open_pin_text_prompt(pin_id, global, window, cx);
            return;
        }

        self.board.update(cx, |board, cx| {
            let Some(pin) = board.pins.iter_mut().find(|p| p.id == pin_id) else {
                return;
            };
            let base_pt = pin.global_to_base(global);
            let area = Bounds::new(point(px(0.), px(0.)), pin.base);

            // Check handles on a single selected mark first.
            if let Some(sel_ix) = pin.annotations.selected_index()
                && pin.annotations.selected_count() == 1
                && let Some(shape) = pin.annotations.shape(sel_ix).cloned()
                && let Some(anchor) = shape.handle_at(base_pt)
            {
                board.annot_gesture = Some(PinAnnotGesture::MoveHandle {
                    pin_id,
                    ix: sel_ix,
                    anchor,
                    before: shape,
                });
                cx.notify();
                return;
            }

            // Pick/move existing shape when Select mode (or clicking an already selected frame).
            if let Some(hit_ix) = pin.annotations.interaction_hit_test(base_pt) {
                pin.annotations.select_index(hit_ix);
                if let Some(before) = pin.annotations.shape(hit_ix).cloned() {
                    board.annot_gesture = Some(PinAnnotGesture::MoveMark {
                        pin_id,
                        ix: hit_ix,
                        start_base: base_pt,
                        before,
                    });
                }
                cx.notify();
                return;
            }

            if matches!(pin.annotations.tool(), None | Some(ShapeKind::Select)) {
                pin.annotations.deselect();
                cx.notify();
                return;
            }

            pin.annotations.begin(base_pt, area, false);
            pin.rebuild_live_preview();
            board.annot_gesture = Some(PinAnnotGesture::Draw { pin_id });
            cx.notify();
        });
    }

    fn open_pin_text_prompt(
        &mut self,
        pin_id: u64,
        global: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((base_anchor, color, font_size)) = self.board.update(cx, |board, _| {
            let pin = board.pins.iter().find(|p| p.id == pin_id)?;
            Some((
                pin.global_to_base(global),
                pin.annotations.color().0,
                pin.annotations.text_size(),
            ))
        }) else {
            return;
        };

        let out = self.output.bounds;
        let w = TEXT_PROMPT_W.min((f32::from(out.size.width) - 12.).max(100.));
        let x = f32::from(global.x).clamp(
            f32::from(out.left()) + 6.,
            (f32::from(out.right()) - w - 6.).max(f32::from(out.left()) + 6.),
        );
        let y = f32::from(global.y).clamp(
            f32::from(out.top()) + 6.,
            (f32::from(out.bottom()) - TEXT_PROMPT_H - 6.).max(f32::from(out.top()) + 6.),
        );
        let prompt_bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(TEXT_PROMPT_H)));

        let input = cx.new(|cx| {
            let mut ti = TextInput::new_with_initial("", w - 16., 28., 15., cx);
            ti.set_color(color, cx);
            ti
        });
        let _ = font_size;
        let sub = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.commit_pin_text_prompt(window, cx);
            }
        });
        let focus_target = input.clone();
        self.board.update(cx, |board, cx| {
            board.text_prompt = Some(PinTextPrompt {
                pin_id,
                base_anchor,
                global_bounds: prompt_bounds,
                input,
                _sub: sub,
            });
            cx.notify();
        });
        focus_target.update(cx, |ti, cx| ti.focus(window, cx));
    }

    pub(super) fn commit_pin_text_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self.board.update(cx, |board, _| board.text_prompt.take());
        let Some(prompt) = taken else {
            return;
        };
        prompt.input.update(cx, |ti, cx| ti.discard_preedit(cx));
        let value = prompt.input.read(cx).value();
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            let committed_text = trimmed.to_owned();
            self.board.update(cx, |board, cx| {
                if let Some(pin) = board.pins.iter_mut().find(|p| p.id == prompt.pin_id) {
                    let fsz = pin.annotations.text_size();
                    let style = pin.annotations.text_style().clone();
                    let max_w = (f32::from(pin.base.width) - f32::from(prompt.base_anchor.x))
                        .max(48.)
                        .min(f32::from(pin.base.width));
                    let max_h = (f32::from(pin.base.height) - f32::from(prompt.base_anchor.y))
                        .max(fsz * 1.5)
                        .min(f32::from(pin.base.height));
                    let buf = crate::annotation::text::styled_buffer(
                        &committed_text,
                        max_w,
                        max_h,
                        fsz,
                        &style,
                    );
                    let mut text_w = 0.0_f32;
                    let mut text_h = 0.0_f32;
                    for run in buf.layout_runs() {
                        text_w = text_w.max(run.line_w);
                        text_h = text_h.max(run.line_top + run.line_height);
                    }
                    let inset = style.inset() * 2. + 6.;
                    let box_w = (text_w + inset).clamp(24., f32::from(pin.base.width));
                    let box_h = (text_h + inset).clamp(fsz * 1.4, f32::from(pin.base.height));
                    let bx = f32::from(prompt.base_anchor.x)
                        .clamp(0., (f32::from(pin.base.width) - box_w).max(0.));
                    let by = f32::from(prompt.base_anchor.y)
                        .clamp(0., (f32::from(pin.base.height) - box_h).max(0.));
                    pin.annotations.add_text(
                        Bounds::new(point(px(bx), px(by)), size(px(box_w), px(box_h))),
                        committed_text,
                    );
                    pin.dirty_raster = true;
                    pin.rebuild_visuals();
                    board.sync_pin_to_history(prompt.pin_id);
                }
                cx.notify();
            });
        } else {
            self.board.update(cx, |_, cx| cx.notify());
        }
        window.focus(&self.focus_handle, cx);
    }

    pub(super) fn render_annot_bar(
        &self,
        bar_global: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let state = self.board.read(cx);
        let id = state.annotating?;
        let pin = state.pins.iter().find(|p| p.id == id)?;
        let active_tool = pin.annotations.tool();
        let active_swatch = pin
            .annotations
            .active_swatch_index()
            .unwrap_or_else(|| pin.annotations.color_index());
        let cur_size = pin.annotations.current_edit_size().round() as u32;
        let palette_colors = *crate::model::settings::current().annotation_colors();
        let c = theme::c();
        let local = bar_global.origin - self.output.bounds.origin;

        let tool_btn = |btn_id: &'static str,
                        kind: ShapeKind,
                        icon: IconName,
                        action: fn() -> Box<dyn Action>| {
            let selected = active_tool == Some(kind);
            Button::new(btn_id)
                .debug_selector(move || btn_id.into())
                .w(px(28.))
                .h(px(28.))
                .rounded_sm()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(if selected {
                    c.toolbar_selected
                } else {
                    0x0000_0000
                }))
                .hover(|s| s.bg(rgba(c.toolbar_hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                .child(Icon::new(icon).size_4().text_color(rgba(c.toolbar_text)))
        };

        let act_btn = |btn_id: &'static str,
                       icon: IconName,
                       action: fn() -> Box<dyn Action>,
                       accent: bool| {
            Button::new(btn_id)
                .debug_selector(move || btn_id.into())
                .w(px(28.))
                .h(px(28.))
                .rounded_sm()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(if accent { c.accent } else { 0x0000_0000 }))
                .hover(|s| s.bg(rgba(if accent { c.accent } else { c.toolbar_hover })))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                .child(Icon::new(icon).size_4().text_color(rgba(if accent {
                    0xFFFF_FFFF
                } else {
                    c.toolbar_text
                })))
        };

        let sep = || {
            div()
                .w(px(1.))
                .h(px(18.))
                .mx_0p5()
                .bg(rgba(c.toolbar_border))
        };

        let row1 = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_0p5()
            .child(tool_btn(
                "pin-tool-select",
                ShapeKind::Select,
                IconName::Scan,
                || Box::new(PinToolSelect),
            ))
            .child(tool_btn(
                "pin-tool-rect",
                ShapeKind::Rectangle,
                IconName::Square,
                || Box::new(PinToolRect),
            ))
            .child(tool_btn(
                "pin-tool-ellipse",
                ShapeKind::Ellipse,
                IconName::Circle,
                || Box::new(PinToolEllipse),
            ))
            .child(tool_btn(
                "pin-tool-arrow",
                ShapeKind::Arrow,
                IconName::ArrowUpRight,
                || Box::new(PinToolArrow),
            ))
            .child(tool_btn(
                "pin-tool-line",
                ShapeKind::Line,
                IconName::Slash,
                || Box::new(PinToolLine),
            ))
            .child(tool_btn(
                "pin-tool-pencil",
                ShapeKind::Pencil,
                IconName::Pencil,
                || Box::new(PinToolPencil),
            ))
            .child(tool_btn(
                "pin-tool-hl",
                ShapeKind::Highlighter,
                IconName::Highlighter,
                || Box::new(PinToolHighlighter),
            ))
            .child(tool_btn(
                "pin-tool-mosaic",
                ShapeKind::Mosaic,
                IconName::SquareDashed,
                || Box::new(PinToolMosaic),
            ))
            .child(tool_btn(
                "pin-tool-number",
                ShapeKind::Number,
                IconName::ListOrdered,
                || Box::new(PinToolNumber),
            ))
            .child(tool_btn(
                "pin-tool-text",
                ShapeKind::Text,
                IconName::Type,
                || Box::new(PinToolText),
            ))
            .child(tool_btn(
                "pin-tool-eraser",
                ShapeKind::Eraser,
                IconName::Eraser,
                || Box::new(PinToolEraser),
            ))
            .child(sep())
            .child(act_btn(
                "pin-annot-undo",
                IconName::Undo2,
                || Box::new(PinUndo),
                false,
            ))
            .child(act_btn(
                "pin-annot-redo",
                IconName::Redo2,
                || Box::new(PinRedo),
                false,
            ))
            .child(act_btn(
                "pin-annot-clear",
                IconName::Trash,
                || Box::new(PinClearMarks),
                false,
            ))
            .child(act_btn(
                "pin-annot-done",
                IconName::Check,
                || Box::new(ExitPinAnnotate),
                true,
            ));

        let mut swatches = div().flex().items_center().gap_1p5();
        for (ix, &swatch_rgba) in palette_colors.iter().enumerate().take(PALETTE) {
            let is_sel = ix == active_swatch;
            let board_handle = self.board.clone();
            swatches = swatches.child(
                div()
                    .id(("pin-swatch", ix))
                    .w(px(18.))
                    .h(px(18.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(rgba(swatch_rgba))
                    .border(px(if is_sel { 2. } else { 1. }))
                    .border_color(rgba(if is_sel { c.accent } else { c.toolbar_border }))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        board_handle.update(cx, |b, cx| b.set_annot_color(ix, cx));
                        cx.stop_propagation();
                    }),
            );
        }

        let size_cluster = div()
            .flex()
            .items_center()
            .gap_1()
            .child(act_btn(
                "pin-size-down",
                IconName::Minus,
                || Box::new(PinSizeDown),
                false,
            ))
            .child(
                div()
                    .min_w(px(28.))
                    .text_center()
                    .text_xs()
                    .text_color(rgba(c.toolbar_text))
                    .child(format!("{cur_size}px")),
            )
            .child(act_btn(
                "pin-size-up",
                IconName::Plus,
                || Box::new(PinSizeUp),
                false,
            ))
            .child(act_btn(
                "pin-mark-del",
                IconName::X,
                || Box::new(PinDeleteMark),
                false,
            ));

        let row2 = div()
            .flex()
            .items_center()
            .justify_between()
            .px_1()
            .child(swatches)
            .child(size_cluster);

        Some(
            div()
                .id("pin-annot-bar")
                .debug_selector(|| "pin-annot-bar".into())
                .absolute()
                .left(local.x)
                .top(local.y)
                .w(bar_global.size.width)
                .h(bar_global.size.height)
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(rgba(c.toolbar_border))
                .bg(rgba(c.toolbar_bg))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(row1)
                .child(row2),
        )
    }

    pub(super) fn render_text_prompt(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let state = self.board.read(cx);
        let prompt = state.text_prompt.as_ref()?;
        if !self.output.bounds.contains(&prompt.global_bounds.origin) {
            return None;
        }
        let c = theme::c();
        let local = prompt.global_bounds.origin - self.output.bounds.origin;
        let input = prompt.input.clone();
        Some(
            div()
                .id("pin-text-prompt")
                .debug_selector(|| "pin-text-prompt".into())
                .absolute()
                .left(local.x)
                .top(local.y)
                .w(prompt.global_bounds.size.width)
                .h(prompt.global_bounds.size.height)
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgba(c.accent))
                .bg(rgba(c.toolbar_bg))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div().relative().size_full().child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(4.))
                            .right(px(0.))
                            .bottom(px(2.))
                            .child(input),
                    ),
                ),
        )
    }
}
