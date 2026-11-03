//! Bounded toolbar rows with a visible scroll affordance on narrow outputs.

use gpui_kit::{
    base::{Scrollbar, ScrollbarMode},
    *,
};

#[derive(IntoElement)]
pub(super) struct ToolbarRow {
    id: &'static str,
    width: Pixels,
    content: Div,
}

impl ToolbarRow {
    pub(super) fn new(id: &'static str, width: Pixels, content: Div) -> Self {
        Self { id, width, content }
    }
}

impl RenderOnce for ToolbarRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |_, _| ScrollHandle::new());
        let scroll = state.read(cx).clone();
        let id = self.id;
        div()
            .relative()
            .max_w(self.width)
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .flex()
                    .max_w(self.width)
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .child(self.content.flex_shrink_0()),
            )
            .child(Scrollbar::horizontal(&scroll).mode(ScrollbarMode::Always))
    }
}
