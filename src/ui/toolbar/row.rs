//! Bounded toolbar rows with a visible scroll affordance on narrow outputs.

use gpui_kit::{
    base::{Scrollbar, ScrollbarMode},
    *,
};

#[derive(IntoElement)]
pub(super) struct ToolbarRow {
    style: crate::model::settings::UiStyle,
    id: &'static str,
    width: Pixels,
    content: Div,
}

impl ToolbarRow {
    pub(super) fn new(
        style: crate::model::settings::UiStyle,
        id: &'static str,
        width: Pixels,
        content: Div,
    ) -> Self {
        Self {
            style,
            id,
            width,
            content,
        }
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
            .rounded(super::bar_radius_for(self.style))
            .shadow(super::bar_shadow_for(self.style))
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
