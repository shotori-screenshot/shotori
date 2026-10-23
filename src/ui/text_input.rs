//! Canvas text input. Cosmic owns glyphs, wrapping, hit testing and editing;
//! GPUI's input-handler protocol supplies native IME and clipboard integration.
use crate::{annotation::text, ui::theme};
use cosmic_text::{Action as EditAction, Cursor, Edit, Editor, Metrics, Motion, Scroll, Selection};
use gpui_kit::{base::input::InputEvent, *};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const CARET_WIDTH_LOGICAL_PIXELS: f32 = 1.;

pub(crate) struct TextInput {
    editor: Editor<'static>,
    focus: FocusHandle,
    marked: Option<Range<usize>>,
    composition_start: Option<Editor<'static>>,
    undo: Vec<Editor<'static>>,
    redo: Vec<Editor<'static>>,
    width: f32,
    limit: f32,
    font_size: f32,
    text_color: u32,
    bounds: Bounds<Pixels>,
    dragging: bool,
    truncated_input: bool,
}
impl EventEmitter<InputEvent> for TextInput {}
impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl TextInput {
    // Used by unit tests in this file (`TextInput::new` creates an empty
    // editor; production code calls `new_with_initial` directly).
    #[allow(dead_code)]
    pub(crate) fn new(width: f32, limit: f32, font_size: f32, cx: &mut Context<Self>) -> Self {
        Self::new_with_initial("", width, limit, font_size, cx)
    }

    pub(crate) fn new_with_initial(
        initial: &str,
        width: f32,
        limit: f32,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            editor: Editor::new(text::buffer(initial, width, limit, font_size)),
            focus: cx.focus_handle(),
            marked: None,
            composition_start: None,
            undo: Vec::new(),
            redo: Vec::new(),
            width,
            limit,
            font_size,
            text_color: 0,
            bounds: Bounds::default(),
            dragging: false,
            truncated_input: false,
        }
    }
    /// End editing without turning unfinished phonetic preedit into annotation text.
    pub(crate) fn discard_preedit(&mut self, cx: &mut Context<Self>) {
        if let Some(before) = self.composition_start.take() {
            self.editor = before;
            self.marked = None;
            self.changed(cx);
        }
    }

    pub(crate) fn input_was_truncated(&self) -> bool {
        self.truncated_input
    }

    /// Insert the longest fitting prefix without splitting a grapheme cluster.
    /// The original selection is retained when no input can fit.
    fn replace_fitting(&mut self, range: Range<usize>, value: &str) -> bool {
        let before = self.editor.clone();
        self.replace(range.clone(), value);
        if self.accept_edit(before.clone()) {
            self.truncated_input = false;
            return true;
        }
        let ends = value
            .grapheme_indices(true)
            .map(|(offset, grapheme)| offset + grapheme.len())
            .collect::<Vec<_>>();
        let (mut low, mut high) = (0, ends.len());
        while low < high {
            let mid = (low + high).div_ceil(2);
            self.editor = before.clone();
            self.replace(range.clone(), &value[..ends[mid - 1]]);
            self.shape();
            if self.layout_fits() {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        self.editor = before;
        if low > 0 {
            self.replace(range, &value[..ends[low - 1]]);
            self.shape();
        }
        self.truncated_input = true;
        low > 0
    }

    pub(crate) fn set_color(&mut self, color: u32, cx: &mut Context<Self>) {
        self.text_color = color;
        cx.notify();
    }
    pub(crate) fn value(&self) -> String {
        self.editor.with_buffer(|b| {
            b.lines
                .iter()
                .map(|l| l.text())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
    /// Replace the whole buffer (pre-filling the number editor with the
    /// badge's current value); the caret lands after the last glyph.
    pub(crate) fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.remember(self.editor.clone());
        self.replace(0..self.value().len(), value);
        self.editor.set_selection(Selection::None);
        self.editor.set_cursor(self.cursor_for(self.value().len()));
        self.changed(cx);
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
    pub(crate) fn content_width(&self) -> f32 {
        self.editor.with_buffer(|b| {
            (b.layout_runs().map(|r| r.line_w).fold(0., f32::max) + CARET_WIDTH_LOGICAL_PIXELS)
                .min(self.width)
        })
    }
    pub(crate) fn height(&self) -> f32 {
        self.layout_height().min(self.limit)
    }
    fn layout_height(&self) -> f32 {
        self.editor.with_buffer(|b| {
            let single_line_h = self.font_size * 1.35;
            let run_height = b
                .layout_runs()
                .map(|r| r.line_top + r.line_height)
                .fold(single_line_h, f32::max);
            let min_lines_h = b.lines.len() as f32 * single_line_h;
            run_height.max(min_lines_h)
        })
    }
    pub(crate) fn set_font_size(&mut self, font_size: f32, cx: &mut Context<Self>) -> bool {
        let old_size = self.font_size;
        let before = self.editor.clone();
        self.font_size = font_size;
        self.shape();
        if !self.layout_fits() {
            self.font_size = old_size;
            self.editor = before;
            return false;
        }
        self.changed(cx);
        true
    }

    fn layout_fits(&self) -> bool {
        self.layout_height() <= self.limit + 0.01
            && self
                .editor
                .with_buffer(|b| b.layout_runs().all(|run| run.line_w <= self.width + 0.01))
    }

    // Validate shaped geometry rather than the clipped widget dimensions.
    fn accept_edit(&mut self, before: Editor<'static>) -> bool {
        self.shape();
        if !self.layout_fits() {
            self.editor = before;
            false
        } else {
            true
        }
    }
    fn shape(&mut self) {
        text::with_fonts(|fonts| {
            self.editor.with_buffer_mut(|b| {
                b.set_metrics(Metrics::new(self.font_size, self.font_size * 1.35));
                b.set_size(Some(self.width), None);
                b.set_scroll(Scroll::default());
                b.shape_until_scroll(fonts, false);
            })
        });
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.shape();
        cx.emit(InputEvent::Change);
        cx.notify();
    }
    fn remember(&mut self, before: Editor<'static>) {
        self.undo
            .push(self.composition_start.take().unwrap_or(before));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn byte(&self, c: Cursor) -> usize {
        self.editor.with_buffer(|b| {
            b.lines
                .iter()
                .take(c.line)
                .map(|l| l.text().len() + 1)
                .sum::<usize>()
                + c.index
        })
    }
    fn cursor_for(&self, offset: usize) -> Cursor {
        let value = self.value();
        let offset = offset.min(value.len());
        let prefix = &value[..offset];
        Cursor::new(
            prefix.bytes().filter(|&b| b == b'\n').count(),
            prefix.rsplit('\n').next().unwrap_or("").len(),
        )
    }
    fn selection(&self) -> Range<usize> {
        let (start, end) = self
            .editor
            .selection_bounds()
            .unwrap_or((self.editor.cursor(), self.editor.cursor()));
        self.byte(start)..self.byte(end)
    }
    fn byte_range_for_utf16(&self, r: Range<usize>) -> Range<usize> {
        let text = self.value();
        utf16_byte(&text, r.start)..utf16_byte(&text, r.end)
    }
    fn to_utf16(&self, r: Range<usize>) -> Range<usize> {
        let text = self.value();
        text[..r.start].encode_utf16().count()..text[..r.end].encode_utf16().count()
    }
    pub(crate) fn replace(&mut self, range: Range<usize>, value: &str) {
        let start = self.cursor_for(range.start);
        let end = self.cursor_for(range.end);
        self.editor.set_cursor(end);
        self.editor.set_selection(Selection::Normal(start));
        self.editor.insert_string(value, None);
    }
    fn caret(&self, cursor: Cursor) -> Bounds<Pixels> {
        self.editor.with_buffer(|b| {
            let single_line_h = self.font_size * 1.35;
            b.layout_runs()
                .find_map(|r| {
                    r.cursor_position(&cursor).map(|x| {
                        Bounds::new(
                            point(px(x), px(r.line_top)),
                            size(px(CARET_WIDTH_LOGICAL_PIXELS), px(r.line_height)),
                        )
                    })
                })
                .unwrap_or_else(|| {
                    let top = cursor.line as f32 * single_line_h;
                    Bounds::new(
                        point(px(0.), px(top)),
                        size(px(CARET_WIDTH_LOGICAL_PIXELS), px(single_line_h)),
                    )
                })
        })
    }
    fn highlights(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        if range.is_empty() {
            return Vec::new();
        }
        let start = self.cursor_for(range.start);
        let end = self.cursor_for(range.end);
        self.editor.with_buffer(|b| {
            b.layout_runs()
                // Cosmic's editor also checks paragraph bounds before calling
                // highlight; runs outside both endpoints otherwise select every glyph.
                .filter(|r| r.line_i >= start.line && r.line_i <= end.line)
                .flat_map(|r| {
                    r.highlight(start, end).map(move |(x, w)| {
                        Bounds::new(point(px(x), px(r.line_top)), size(px(w), px(r.line_height)))
                    })
                })
                .collect()
        })
    }
    pub(crate) fn action(&mut self, a: EditAction) {
        text::with_fonts(|fonts| self.editor.action(fonts, a));
        self.shape();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        } // the IME owns keys while composing
        let before = self.editor.clone();
        let mut record = false;
        let mut history_direction = None;
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let command = mods.control || mods.platform;
        let motion = match key {
            "left" => Some(if command {
                Motion::LeftWord
            } else {
                Motion::Left
            }),
            "right" => Some(if command {
                Motion::RightWord
            } else {
                Motion::Right
            }),
            "up" => Some(Motion::Up),
            "down" => Some(Motion::Down),
            "home" => Some(if command {
                Motion::BufferStart
            } else {
                Motion::Home
            }),
            "end" => Some(if command {
                Motion::BufferEnd
            } else {
                Motion::End
            }),
            _ => None,
        };
        if let Some(motion) = motion {
            if mods.shift {
                if self.editor.selection() == Selection::None {
                    self.editor
                        .set_selection(Selection::Normal(self.editor.cursor()));
                }
            } else {
                self.editor.set_selection(Selection::None);
            }
            self.action(EditAction::Motion(motion));
        } else {
            match key {
                "enter" if !mods.shift => {
                    cx.emit(InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    });
                }
                "enter" => {
                    record = true;
                    self.action(EditAction::Enter);
                }
                "escape" => {
                    window.dispatch_action(Box::new(crate::actions::EscapeOverlay), cx);
                }
                "backspace" | "delete" => {
                    record = true;
                    if command && self.editor.selection_bounds().is_none() {
                        self.editor
                            .set_selection(Selection::Normal(self.editor.cursor()));
                        self.action(EditAction::Motion(if key == "backspace" {
                            Motion::PreviousWord
                        } else {
                            Motion::NextWord
                        }));
                    }
                    self.action(if key == "backspace" {
                        EditAction::Backspace
                    } else {
                        EditAction::Delete
                    });
                }
                "a" if command => {
                    self.editor
                        .set_selection(Selection::Normal(Cursor::new(0, 0)));
                    self.editor.set_cursor(self.cursor_for(self.value().len()));
                }
                "c" | "x" if command => {
                    if let Some(value) = self.editor.copy_selection() {
                        cx.write_to_clipboard(ClipboardItem::new_string(value));
                    }
                    if key == "x" {
                        record = true;
                        self.editor.delete_selection();
                    }
                }
                "v" if command => {
                    if let Some(value) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.replace_text_in_range(None, &value, window, cx);
                        cx.stop_propagation();
                        return;
                    }
                }
                "z" | "y" if command => {
                    let redo = key == "y" || mods.shift;
                    let next = if redo {
                        self.redo.pop()
                    } else {
                        self.undo.pop()
                    };
                    if let Some(next) = next {
                        history_direction = Some(redo);
                        let old = std::mem::replace(&mut self.editor, next);
                        if redo {
                            self.undo.push(old);
                        } else {
                            self.redo.push(old);
                        }
                    }
                }
                _ => return,
            }
        }
        let attempted = self.editor.clone();
        if self.accept_edit(before.clone()) {
            if history_direction.is_some() {
                self.truncated_input = false;
            }
            if record {
                self.truncated_input = false;
                self.remember(before);
            }
        } else if let Some(redo) = history_direction {
            if redo {
                self.undo.pop();
                self.redo.push(attempted);
            } else {
                self.redo.pop();
                self.undo.push(attempted);
            }
        }
        self.changed(cx);
        cx.stop_propagation();
        window.prevent_default();
    }
}
fn utf16_byte(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= offset {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}
impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.byte_range_for_utf16(range);
        *actual = Some(self.to_utf16(range.clone()));
        Some(self.value()[range].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selection()),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|r| self.to_utf16(r))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(marked) = self.marked.clone() {
            let value = self.value()[marked].to_owned();
            self.replace_text_in_range(None, &value, window, cx);
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.byte_range_for_utf16(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let before = self.editor.clone();
        if self.replace_fitting(range, value) {
            self.remember(before);
        } else if let Some(original) = self.composition_start.take() {
            // An empty accepted prefix must not leave phonetic preedit behind.
            self.editor = original;
        }
        self.marked = None;
        self.changed(cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.byte_range_for_utf16(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let before = self.editor.clone();
        self.replace(range.clone(), value);
        // Keep the full preedit range even when its phonetic spelling is wider
        // than the final candidate. Only committed text is fitted to the crop.
        if self.composition_start.is_none() {
            self.composition_start = Some(before);
        }
        self.marked = (!value.is_empty()).then_some(range.start..range.start + value.len());
        if let Some(selection) = selection {
            let start = self.cursor_for(range.start + utf16_byte(value, selection.start));
            let end = self.cursor_for(range.start + utf16_byte(value, selection.end));
            self.editor.set_selection(Selection::Normal(start));
            self.editor.set_cursor(end);
        }
        self.changed(cx);
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.byte_range_for_utf16(range);
        let mut caret = self.caret(self.cursor_for(range.end));
        caret.origin += bounds.origin;
        Some(caret)
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let local = p - self.bounds.origin;
        let c = self
            .editor
            .with_buffer(|b| b.hit(f32::from(local.x), f32::from(local.y)))?;
        Some(self.to_utf16(self.byte(c)..self.byte(c)).start)
    }
}
impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        let text_color = self.text_color;
        let caret = self.caret(self.editor.cursor());
        let selection = self.highlights(self.selection());
        let marked = self
            .marked
            .clone()
            .map(|r| self.highlights(r))
            .unwrap_or_default();
        div()
            .size_full()
            .key_context("ShotoriTextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus(window, cx);
                    this.dragging = true;
                    let p = event.position - this.bounds.origin;
                    let (x, y) = (f32::from(p.x) as i32, f32::from(p.y) as i32);
                    this.action(match event.click_count {
                        2 => EditAction::DoubleClick { x, y },
                        3 => EditAction::TripleClick { x, y },
                        _ => EditAction::Click { x, y },
                    });
                    this.changed(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.dragging {
                    let p = event.position - this.bounds.origin;
                    this.action(EditAction::Drag {
                        x: f32::from(p.x) as i32,
                        y: f32::from(p.y) as i32,
                    });
                    this.changed(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |s, _| s.bounds = bounds);
                        entity.clone()
                    },
                    move |bounds, entity, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            for mut r in selection {
                                r.origin += bounds.origin;
                                window.paint_quad(fill(r, rgba(0x4080ff44)));
                            }
                            for mut r in marked {
                                r.origin += bounds.origin;
                                r.origin.y += r.size.height - px(2.);
                                r.size.height = px(1.);
                                window.paint_quad(fill(r, rgba(theme::c().accent)));
                            }
                            if focus.is_focused(window)
                                && let Some(stroke) =
                                    caret_stroke(caret, bounds, window.scale_factor())
                            {
                                window.paint_quad(fill(stroke, rgba(text_color | 0xff)));
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

/// Align the caret to device pixels and keep its full width inside the editor.
fn caret_stroke(
    caret: Bounds<Pixels>,
    editor: Bounds<Pixels>,
    scale: f32,
) -> Option<Bounds<Pixels>> {
    let left = (f32::from(editor.left()) * scale).ceil();
    let right = (f32::from(editor.right()) * scale).floor();
    let width = (CARET_WIDTH_LOGICAL_PIXELS * scale).round().max(1.);
    if right - left < width {
        return None;
    }
    let x = (f32::from(editor.left() + caret.left()) * scale)
        .round()
        .clamp(left, right - width);
    let top = (f32::from(editor.top() + caret.top()) * scale)
        .round()
        .max((f32::from(editor.top()) * scale).ceil());
    let bottom = (f32::from(editor.top() + caret.bottom()) * scale)
        .round()
        .min((f32::from(editor.bottom()) * scale).floor());
    (bottom > top).then(|| {
        Bounds::new(
            point(px(x / scale), px(top / scale)),
            size(px(width / scale), px((bottom - top) / scale)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::TextInput;
    use cosmic_text::Edit;
    use gpui_kit::ClipboardItem;
    use gpui_kit::{Bounds, EntityInputHandler, TestAppContext, point, px, size};

    struct CaretHarness {
        input: gpui_kit::Entity<TextInput>,
    }
    impl gpui_kit::Render for CaretHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled};
            let input = self.input.read(cx);
            gpui_kit::div().size_full().child(
                gpui_kit::div()
                    .absolute()
                    .left(px(20.2))
                    .top(px(24.3))
                    .w(px(input.content_width().max(9.)))
                    .h(px(input.height()))
                    .child(self.input.clone()),
            )
        }
    }

    #[gpui_kit::test]
    fn focused_caret_keeps_one_unclipped_stroke_at_every_text_position(cx: &mut TestAppContext) {
        use gpui_kit::AppContext;
        const COLOR: u32 = 0x395ea8ff;
        let input = cx.new(|cx| TextInput::new(160., 240., 24., cx));
        let (_, cx) = cx.add_window_view(|_, _| CaretHarness {
            input: input.clone(),
        });
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                s.set_color(COLOR, cx);
                s.focus(window, cx);
            })
        });
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            cx.simulate_scale_factor_change(scale);
            for value in ["", "Canvas", "Wide and narrow"] {
                cx.update(|_, cx| input.update(cx, |s, cx| s.set_value(value, cx)));
                for index in 0..=value.len() {
                    cx.update(|_, cx| {
                        input.update(cx, |s, cx| {
                            s.editor.set_cursor(cosmic_text::Cursor::new(0, index));
                            s.editor.set_selection(cosmic_text::Selection::None);
                            cx.notify();
                        })
                    });
                    let quads = cx.update(|window, cx| {
                        window.draw(cx).clear(cx);
                        window.painted_quads()
                    });
                    assert_eq!(
                        quads.len(),
                        1,
                        "one caret without a halo at {scale}: {index}"
                    );
                    let caret = &quads[0];
                    let color: gpui_kit::Background = gpui_kit::rgba(COLOR).into();
                    assert_eq!(caret.background, color);
                    assert_eq!(caret.bounds.size.width.0, scale.round().max(1.));
                    assert_eq!(caret.bounds.origin.x.0.fract(), 0.);
                    assert_eq!(caret.bounds.origin.y.0.fract(), 0.);
                    assert_eq!(
                        caret.bounds.intersect(&caret.content_mask.bounds),
                        caret.bounds
                    );
                }
            }
        }
        cx.update(|window, cx| window.blur(cx));
        let quads = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.painted_quads()
        });
        assert!(quads.is_empty(), "an unfocused editor has no caret");
    }

    #[gpui_kit::test]
    fn committed_input_fills_remaining_space_as_one_undo_step(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(120., 64.8, 24., cx));
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.replace_text_in_range(None, "one\ntwo", window, cx);
                    s.replace_text_in_range(None, "\nthird", window, cx);
                    assert_eq!(s.value(), "one\ntwo");
                    assert_eq!(s.undo.len(), 1);
                    assert!(s.input_was_truncated());
                    assert!(!s.set_font_size(48., cx));
                    cx.write_to_clipboard(ClipboardItem::new_string("W".repeat(200)));
                })
            });
            cx.simulate_keystrokes("ctrl-v");
            let filled = cx.update(|_, cx| {
                let s = input.read(cx);
                assert!(s.value().starts_with("one\ntwoW"));
                assert!(s.value().len() < 207);
                assert!(s.layout_height() <= s.limit + 0.01);
                assert_eq!(s.undo.len(), 2);
                assert!(s.input_was_truncated());
                s.value()
            });
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| assert_eq!(input.read(cx).value(), "one\ntwo"));
            cx.simulate_keystrokes("ctrl-y");
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    assert_eq!(s.value(), filled);
                    s.replace_and_mark_text_in_range(
                        None,
                        &"phonetic".repeat(30),
                        None,
                        window,
                        cx,
                    );
                    assert!(s.marked.is_some());
                    s.replace_text_in_range(None, &"W".repeat(200), window, cx);
                    assert_eq!(s.value(), filled);
                    assert!(s.marked.is_none());
                    assert_eq!(s.undo.len(), 2);
                    let all = s.value().encode_utf16().count();
                    s.replace_text_in_range(Some(0..all), "short\nline", window, cx);
                    assert_eq!(s.value(), "short\nline");
                    assert!(!s.input_was_truncated());
                })
            });
        });
    }

    #[gpui_kit::test]
    fn ime_partial_commit_preserves_clusters_ranges_and_original_undo(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            for cluster in ["界", "e\u{301}", "👩‍💻", "🇨🇦"] {
                let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(120., 64.8, 24., cx));
                let candidate = cluster.repeat(80);
                cx.update(|window, cx| {
                    input.update(cx, |s, cx| {
                        s.focus(window, cx);
                        s.replace_text_in_range(None, "A", window, cx);
                        s.replace_and_mark_text_in_range(
                            None,
                            &"pinyin".repeat(80),
                            None,
                            window,
                            cx,
                        );
                        assert_eq!(s.marked_text_range(window, cx), Some(1..481));
                        s.replace_text_in_range(None, &candidate, window, cx);
                        let value = s.value();
                        let accepted = &value[1..];
                        assert!(!accepted.is_empty());
                        assert!(accepted.len() < candidate.len());
                        assert_eq!(accepted.len() % cluster.len(), 0, "{cluster:?}");
                        assert!(candidate.starts_with(accepted));
                        assert!(s.marked.is_none());
                        assert!(s.composition_start.is_none());
                        assert!(s.input_was_truncated());
                        let end = value.encode_utf16().count();
                        assert_eq!(
                            s.selected_text_range(false, window, cx).unwrap().range,
                            end..end
                        );
                        assert!(s.layout_height() <= s.limit + 0.01);
                        assert_eq!(s.undo.len(), 2);
                    })
                });
                cx.simulate_keystrokes("ctrl-z");
                cx.update(|_, cx| assert_eq!(input.read(cx).value(), "A"));
            }
        });
    }

    #[gpui_kit::test]
    fn partial_replacement_preserves_suffix_and_unmark_fits_preedit(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(120., 64.8, 24., cx));
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    s.replace_text_in_range(None, "A🙂Z", window, cx);
                    s.replace_text_in_range(Some(1..3), &"界".repeat(100), window, cx);
                    assert!(s.value().starts_with("A界"));
                    assert!(s.value().ends_with('Z'));
                    assert!(!s.value().contains('🙂'));
                    assert!(s.input_was_truncated());
                    let all = s.value().encode_utf16().count();
                    s.replace_and_mark_text_in_range(
                        Some(0..all),
                        &"W".repeat(100),
                        None,
                        window,
                        cx,
                    );
                    s.unmark_text(window, cx);
                    assert!(s.marked.is_none());
                    assert!(s.composition_start.is_none());
                    assert!(!s.value().is_empty() && s.value().len() < 100);
                    assert!(s.layout_fits());
                    assert!(s.input_was_truncated());
                })
            });
            let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(12., 64.8, 24., cx));
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    s.replace_text_in_range(None, "i", window, cx);
                    s.replace_text_in_range(Some(0..1), "W", window, cx);
                    assert_eq!(
                        s.value(),
                        "i",
                        "a too-wide cluster cannot replace existing text"
                    );
                    assert!(s.input_was_truncated());
                    assert_eq!(s.undo.len(), 1);
                })
            });
        });
    }

    #[gpui_kit::test]
    fn soft_wrap_stops_before_a_partial_bottom_row(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(120., 80., 24., cx));
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    for _ in 0..100 {
                        s.replace_text_in_range(None, "W", window, cx);
                    }
                    assert!(s.value().len() > 1 && s.value().len() < 100);
                    assert_eq!(s.undo.len(), s.value().len());
                    assert!((s.layout_height() - 24. * 1.35 * 2.).abs() < 0.01);
                    assert!(s.layout_height() < s.limit);
                })
            });
        });
    }

    #[gpui_kit::test]
    fn set_value_replaces_the_whole_buffer_and_appends_after(cx: &mut TestAppContext) {
        let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(160., 400., 24., cx));
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                s.replace_text_in_range(None, "old text", window, cx);
                s.set_value("42", cx);
                assert_eq!(s.value(), "42");
                // the caret must sit AFTER the prefilled digits so a
                // typed character appends, not prepends
                s.replace_text_in_range(None, "1", window, cx);
                assert_eq!(s.value(), "421");
            })
        });
    }

    #[gpui_kit::test]
    fn ime_preedit_replacement_and_utf16_ranges(cx: &mut TestAppContext) {
        let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(160., 400., 24., cx));
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                s.replace_text_in_range(None, "🙂", window, cx);
                s.replace_and_mark_text_in_range(None, "nihao", Some(5..5), window, cx);
                assert_eq!(s.value(), "🙂nihao");
                assert_eq!(s.marked_text_range(window, cx), Some(2..7));
                assert!(!s.highlights(s.marked.clone().unwrap()).is_empty());
                assert_eq!(
                    s.selected_text_range(false, window, cx).unwrap().range,
                    7..7
                );
                s.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                assert_eq!(s.value(), "🙂ni");
                s.replace_text_in_range(None, "你好", window, cx);
                assert_eq!(s.value(), "🙂你好");
                assert!(s.marked.is_none());
                assert_eq!(
                    s.selected_text_range(false, window, cx).unwrap().range,
                    4..4
                );
            })
        });
    }

    #[gpui_kit::test]
    fn text_grows_until_the_selection_right_edge(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(500., 600., 24., cx));
            cx.update(|window, cx| {
                input.update(cx, |s, cx| {
                    assert_eq!(s.content_width(), 1.);
                    s.replace_text_in_range(None, "short", window, cx);
                    let short_width = s.content_width();
                    assert!(short_width > 1. && short_width < 100.);
                    // These glyphs are covered by the bundled font. CJK
                    // fallback availability must not change layout assertions.
                    s.replace_text_in_range(None, &"W".repeat(12), window, cx);
                    assert!(s.content_width() > 320.);
                    assert_eq!(s.height(), 24. * 1.35);
                    s.replace_text_in_range(None, &"W".repeat(12), window, cx);
                    assert!(s.height() > 24. * 1.35);
                    assert!(s.content_width() <= 500.);
                })
            });
        });
    }

    #[gpui_kit::test]
    fn paragraph_changes_do_not_paint_unselected_text(cx: &mut TestAppContext) {
        let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(160., 600., 24., cx));
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                for paragraph in ["第一段文字自动换行", "second paragraph wraps", "第三段"]
                {
                    s.replace_text_in_range(None, paragraph, window, cx);
                    assert!(
                        s.highlights(s.selection()).is_empty(),
                        "a caret is not a selection"
                    );
                    s.action(cosmic_text::Action::Enter); // Shift+Enter uses this action.
                    assert!(
                        s.highlights(s.selection()).is_empty(),
                        "newline must not highlight prior paragraphs"
                    );
                }
                // Moving the caret into any paragraph must not tint the others.
                for line in 0..3 {
                    s.editor.set_cursor(cosmic_text::Cursor::new(line, 0));
                    s.editor.set_selection(cosmic_text::Selection::None);
                    assert!(s.highlights(s.selection()).is_empty());
                }
                // A real selection (and an IME underline) stays within its paragraphs.
                let start = s.byte(cosmic_text::Cursor::new(1, 0));
                let end = s.byte(cosmic_text::Cursor::new(1, 6));
                let spans = s.highlights(start..end);
                assert!(!spans.is_empty());
                let (top, bottom) = s.editor.with_buffer(|b| {
                    let runs = b
                        .layout_runs()
                        .filter(|r| r.line_i == 1)
                        .collect::<Vec<_>>();
                    (
                        runs.first().unwrap().line_top,
                        runs.last().unwrap().line_top + runs.last().unwrap().line_height,
                    )
                });
                assert!(
                    spans
                        .iter()
                        .all(|r| r.top() >= px(top) && r.bottom() <= px(bottom))
                );
            });
        });
    }

    #[gpui_kit::test]
    fn caret_stays_on_last_glyph_after_many_soft_wraps(cx: &mut TestAppContext) {
        let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(160., 2000., 24., cx));
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                for font_size in [16., 24., 32.] {
                    s.set_font_size(font_size, cx);
                    for length in [1, 20, 60, 120] {
                        s.replace_text_in_range(
                            Some(0..s.value().encode_utf16().count()),
                            &"abcdefgh".repeat(length),
                            window,
                            cx,
                        );
                        let caret = s.caret(s.editor.cursor());
                        s.editor.with_buffer(|b| {
                            let run = b.layout_runs().last().unwrap();
                            let glyph = run.glyphs.last().unwrap();
                            assert_eq!(caret.origin.y, px(run.line_top));
                            assert!((f32::from(caret.origin.x) - glyph.x - glyph.w).abs() < 0.01);
                        });
                        let ime = s
                            .bounds_for_range(
                                s.to_utf16(s.selection()),
                                Bounds::new(point(px(20.), px(30.)), size(px(160.), px(2000.))),
                                window,
                                cx,
                            )
                            .unwrap();
                        assert_eq!(ime.origin, caret.origin + point(px(20.), px(30.)));
                    }
                }
            })
        });
    }

    #[gpui_kit::test]
    fn trailing_newline_expands_height_and_positions_caret(cx: &mut TestAppContext) {
        let (input, cx) = cx.add_window_view(|_, cx| TextInput::new(300., 600., 24., cx));
        cx.update(|window, cx| {
            input.update(cx, |s, cx| {
                let single_h = 24. * 1.35;
                s.replace_text_in_range(None, "hello", window, cx);
                assert!((s.height() - single_h).abs() < 1e-3);
                s.action(cosmic_text::Action::Enter);
                assert!((s.height() - single_h * 2.).abs() < 1e-3);
                let caret = s.caret(s.editor.cursor());
                assert_eq!(caret.origin.x, px(0.));
                assert!((f32::from(caret.origin.y) - single_h).abs() < 1e-3);
            });
        });
    }
}
