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
    pub(crate) style: crate::annotation::text::TextStyle,
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
            style: Default::default(),
            bounds,
            local,
            visible_width: 1.,
            height: (font_size * 1.35).min(f32::from(bounds.size.height)),
        };
        editor.refresh(cx);
        editor
    }
    /// Hit-test after font metrics and decoration insets have been synchronized.
    pub(crate) fn place_caret(&self, click: Point<Pixels>, cx: &mut App) {
        let p = click - self.local - point(px(self.style.inset()), px(self.style.inset()));
        self.input.update(cx, |input, cx| {
            input.action(cosmic_text::Action::Click {
                x: f32::from(p.x) as i32,
                y: f32::from(p.y) as i32,
            });
            cx.notify();
        });
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
        self.height = self.input.read(cx).height() + self.style.inset() * 2.;
        self.visible_width = self.input.read(cx).content_width() + self.style.inset() * 2.;
    }
    pub(crate) fn sync_style(
        &mut self,
        size: f32,
        color: u32,
        style: &crate::annotation::text::TextStyle,
        cx: &mut App,
    ) -> bool {
        let changed = self.font_size != size || self.color != color || self.style != *style;
        if self.font_size != size || self.style != *style {
            let inset = style.inset() * 2.;
            let width = (f32::from(self.bounds.size.width) - inset).max(1.);
            let limit = (f32::from(self.bounds.size.height) - inset).max(0.);
            if self
                .input
                .update(cx, |s, cx| s.set_appearance(style, size, width, limit, cx))
            {
                self.font_size = size;
                self.style = style.clone();
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
        // Keep the empty input clickable after subtracting decoration insets.
        let min_w = (self.font_size * 0.35).round().clamp(6., 9.);
        let display_w = self.visible_width.max(min_w + self.style.inset() * 2.);
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
            // TextInput has no intrinsic layout size: all four edges must
            // constrain its parent so the caret and hitbox keep their extent.
            .child(
                div()
                    .absolute()
                    .left(px(self.style.inset()))
                    .top(px(self.style.inset()))
                    .right(px(self.style.inset()))
                    .bottom(px(self.style.inset()))
                    .child(self.input.clone()),
            )
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
    fn inline_editor_paints_caret_and_places_it_at_pointer(cx: &mut TestAppContext) {
        crate::annotation::text::with_test_font(|| {
            let selection =
                Bounds::from_corners(point(px(20.), px(20.)), point(px(420.), px(300.)));
            let (view, cx) = cx.add_window_view(|_, cx| EditorHarness {
                selection,
                editor: super::TextEditor::new(
                    Bounds::from_corners(point(px(70.2), px(80.3)), selection.bottom_right()),
                    point(px(70.2), px(80.3)),
                    24.,
                    0xe52a39ff,
                    cx,
                ),
            });
            for decorated in [false, true] {
                for scale in [1., 1.25, 1.5, 1.75, 2.] {
                    cx.simulate_scale_factor_change(scale);
                    cx.update(|window, cx| {
                        view.update(cx, |s, cx| {
                            let style = crate::annotation::text::TextStyle {
                                background: decorated.then_some(0x28394aff),
                                outline: decorated.then_some(0xffffffff),
                                ..Default::default()
                            };
                            s.editor.sync_style(24., 0xe52a39ff, &style, cx);
                            s.editor
                                .input()
                                .update(cx, |input, cx| input.set_value("", cx));
                            s.editor.refresh(cx);
                            s.editor.focus(window, cx);
                            cx.notify();
                        });
                    });
                    for value in ["", "Canvas note"] {
                        cx.update(|_, cx| {
                            view.update(cx, |s, cx| {
                                s.editor
                                    .input()
                                    .update(cx, |input, cx| input.set_value(value, cx));
                                s.editor.refresh(cx);
                                cx.notify();
                            });
                        });
                        let quads = cx.update(|window, cx| {
                            window.draw(cx).clear(cx);
                            window.painted_quads()
                        });
                        let color: gpui_kit::Background = gpui_kit::rgba(0xe52a39ff).into();
                        let carets = quads
                            .iter()
                            .filter(|q| q.background == color)
                            .collect::<Vec<_>>();
                        assert_eq!(
                            carets.len(),
                            1,
                            "visible caret: decorated={decorated}, scale={scale}, text={value:?}"
                        );
                        assert_eq!(
                            carets[0].bounds.intersect(&carets[0].content_mask.bounds),
                            carets[0].bounds
                        );
                    }
                    let at_start = cx.update(|_, cx| {
                        let editor = &view.read(cx).editor;
                        editor.local
                            + point(
                                px(editor.style.inset() + 1.),
                                px(editor.style.inset() + 12.),
                            )
                    });
                    cx.simulate_click(at_start, Default::default());
                    cx.simulate_input("A");
                    cx.update(|_, cx| assert_eq!(view.read(cx).editor.value(cx), "ACanvas note"));
                    cx.simulate_mouse_down(
                        at_start,
                        gpui_kit::MouseButton::Left,
                        Default::default(),
                    );
                    let drag_end = at_start + point(px(60.), px(0.));
                    cx.simulate_mouse_move(
                        drag_end,
                        Some(gpui_kit::MouseButton::Left),
                        Default::default(),
                    );
                    cx.simulate_mouse_up(drag_end, gpui_kit::MouseButton::Left, Default::default());
                    let has_selection = cx.update(|window, cx| {
                        window.draw(cx).clear(cx);
                        let color: gpui_kit::Background = gpui_kit::rgba(0x4080ff44).into();
                        window.painted_quads().iter().any(|quad| {
                            quad.background == color && quad.bounds.size.width.0 > 20. * scale
                        })
                    });
                    assert!(
                        has_selection,
                        "dragging selects text inside the input region"
                    );
                }
            }
        });
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
