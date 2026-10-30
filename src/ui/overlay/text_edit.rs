//! Text & number-badge annotation editing on the overlay: the inline
//! editor lifecycle (subscribe → start → refresh → finish), shared by
//! fresh text strokes and double-click edits of a number badge.

use super::Overlay;
use gpui_kit::*;

impl Overlay {
    /// Escape finishes active editing, or exits when the canvas is idle.
    pub(super) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ocr_setup.is_some() {
            self.ocr_setup_cancel(window, cx);
        } else if self.text_editing.is_some() {
            self.finish_text(true, window, cx);
        } else {
            let interrupted = self.session.update(cx, |s, cx| {
                let interrupted = s.escape_interaction();
                cx.notify();
                interrupted
            });
            if !interrupted {
                cx.quit();
            }
        }
        cx.stop_propagation();
        cx.notify();
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

    pub(super) fn start_text(
        &mut self,
        local: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let mut editor =
            crate::ui::text_editor::TextEditor::new(bounds, local, font_size, color, cx);
        let style = self.session.read(cx).annotations().text_style().clone();
        editor.sync_style(font_size, color, &style, cx);
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

    pub(super) fn start_number_edit(
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

    pub(super) fn refresh_text(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn start_edit_text(
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
                s.text_bounds(&self.capture.output_name, local_origin, shape.size())
            else {
                return;
            };
            (shape, available.origin - screen_origin, available)
        };
        let text = shape.text.as_deref().unwrap_or("");
        let font_size = shape.size();
        let color = shape.color;
        let mut editor = crate::ui::text_editor::TextEditor::new_with_text(
            available,
            local_origin,
            font_size,
            color,
            text,
            None,
            cx,
        );
        editor.sync_style(font_size, color, &shape.text_style, cx);
        editor.place_caret(click_local, cx);
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

    pub(super) fn finish_text(
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
}
