//! Inline editor placement; text layout and input live in TextInput.
use crate::ui::text_input::TextInput;
use gpui_kit::*;

pub(crate) struct TextEditor {
    input: Entity<TextInput>,
    bounds: Bounds<Pixels>,
    local: Point<Pixels>,
    height: f32,
    visible_width: f32,
    pub(crate) font_size: f32,
    pub(crate) color: u32,
}
impl TextEditor {
    pub(crate) fn new(
        bounds: Bounds<Pixels>,
        local: Point<Pixels>,
        font_size: f32,
        color: u32,
        cx: &mut App,
    ) -> Self {
        Self::new_with_text(bounds, local, font_size, color, "", None, cx)
    }

    pub(crate) fn new_with_text(
        bounds: Bounds<Pixels>,
        local: Point<Pixels>,
        font_size: f32,
        color: u32,
        initial_text: &str,
        click_local: Option<Point<Pixels>>,
        cx: &mut App,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = TextInput::new_with_initial(
                initial_text,
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                font_size,
                cx,
            );
            input.set_color(color, cx);
            // Position the cursor at the click point so the user can
            // immediately type at the exact location they tapped.
            if let Some(click) = click_local {
                let p = click - local;
                let (x, y) = (f32::from(p.x) as i32, f32::from(p.y) as i32);
                input.action(cosmic_text::Action::Click { x, y });
            }
            input
        });
        let mut editor = Self {
            input,
            font_size,
            color,
            bounds,
            local,
            visible_width: 1.,
            height: (font_size * 1.35).min(f32::from(bounds.size.height)),
        };
        editor.refresh(cx);
        editor
    }
    pub(crate) fn input(&self) -> &Entity<TextInput> {
        &self.input
    }
    pub(crate) fn actual_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            self.bounds.origin,
            size(
                px(self.visible_width)
                    .max(px(1.))
                    .min(self.bounds.size.width),
                px(self.height).min(self.bounds.size.height),
            ),
        )
    }
    pub(crate) fn value(&self, cx: &App) -> String {
        self.input.read(cx).value()
    }
    pub(crate) fn refresh(&mut self, cx: &App) {
        self.height = self.input.read(cx).height();
        self.visible_width = self.input.read(cx).content_width();
    }
    pub(crate) fn sync_style(&mut self, size: f32, color: u32, cx: &mut App) -> bool {
        let changed = self.font_size != size || self.color != color;
        if self.font_size != size {
            if self.input.update(cx, |s, cx| s.set_font_size(size, cx)) {
                self.font_size = size;
            }
            self.refresh(cx);
        }
        if self.color != color {
            self.color = color;
            self.input.update(cx, |s, cx| s.set_color(color, cx));
        }
        changed
    }
    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }
    pub(crate) fn render(&self) -> impl IntoElement {
        // Keep the empty editor clickable without moving its text origin.
        let min_w = (self.font_size * 0.35).round().clamp(6., 9.);
        let display_w = self.visible_width.max(min_w);
        let display_h = self.height.max(self.font_size * 1.35);
        let visible = size(
            px(display_w).min(self.bounds.size.width),
            px(display_h).min(self.bounds.size.height),
        );
        div()
            .id("text-editor")
            .debug_selector(|| "text-editor".to_owned())
            .absolute()
            .left(self.local.x)
            .top(self.local.y)
            .w(visible.width)
            .h(visible.height)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.input.clone())
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, TestAppContext,
        Window, div, point, px,
    };

    struct EditorHarness {
        selection: Bounds<Pixels>,
        editor: super::TextEditor,
    }
    impl Render for EditorHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(crate::ui::hud::selection_backdrop(Some(self.selection)))
                .child(self.editor.render())
        }
    }
    #[gpui_kit::test]
    fn actual_text_editor_stays_inside_selection(cx: &mut TestAppContext) {
        let selection =
            Bounds::from_corners(point(px(20.2), px(20.3)), point(px(350.5), px(360.8)));
        let (view, cx) = cx.add_window_view(|_, cx| EditorHarness {
            selection,
            editor: super::TextEditor::new_with_text(
                Bounds::from_corners(point(px(70.2), px(80.6)), selection.bottom_right()),
                point(px(70.2), px(80.6)),
                24.,
                0xff0000ff,
                &"long text which wraps ".repeat(100),
                None,
                cx,
            ),
        });
        for scale in [1., 1.25, 1.5, 1.73, 2.] {
            cx.simulate_scale_factor_change(scale);
            let quads = cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.painted_quads()
            });
            let borders = quads
                .iter()
                .filter(|q| q.border_widths.top.0 > 0.)
                .collect::<Vec<_>>();
            let mut outlines = borders.iter().map(|q| q.bounds).collect::<Vec<_>>();
            outlines.dedup();
            assert_eq!(
                outlines.len(),
                1,
                "only the screenshot selection has a border"
            );
            view.update(cx, |s, _| {
                assert!(s.editor.actual_bounds().right() <= selection.right());
                assert!(s.editor.actual_bounds().bottom() <= selection.bottom());
            });
        }
    }
}
