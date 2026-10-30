//! Spotlight outline controls keep slider state separate from the opacity control.
use super::*;
use gpui_kit::{base::slider::SliderEvent, component::popover::Popover};

#[derive(IntoElement)]
pub(super) struct SpotlightOptions {
    pub(super) session: Entity<ScreenshotSession>,
    pub(super) focus: FocusHandle,
    pub(super) color: Option<Entity<ColorPickerState>>,
}

struct OutlineControls {
    width: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl OutlineControls {
    fn new(
        session: Entity<ScreenshotSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value = session.read(cx).annotations().spotlight_outline().1;
        let width = cx.new(|_| SliderState::new().min(1.).max(20.).default_value(value));
        let change_session = session.clone();
        let change = cx.subscribe(&width, move |_, _, event, cx| {
            change_session.update(cx, |s, cx| {
                s.edit_annotations(|a| match event {
                    SliderEvent::Change(value) => a.set_spotlight_outline_width(value.start()),
                    SliderEvent::Release(_) => a.end_size_drag(),
                });
                cx.notify();
            });
        });
        let sync = cx.observe_in(&session, window, |this, session, window, cx| {
            let value = session.read(cx).annotations().spotlight_outline().1;
            if this.width.read(cx).value().start() != value {
                this.width
                    .update(cx, |s, cx| s.set_value(value, window, cx));
            }
            cx.notify();
        });
        Self {
            width,
            _subscriptions: vec![change, sync],
        }
    }
}

impl RenderOnce for SpotlightOptions {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let session = self.session.clone();
        let state = window.use_keyed_state("annotation-spotlight-controls", cx, |window, cx| {
            OutlineControls::new(session, window, cx)
        });
        let (enabled, width) = self.session.read(cx).annotations().spotlight_outline();
        let session = self.session.clone();
        let mut row = div().flex().items_center().gap(px(2.)).child(
            control(
                "spotlight-outline".into(),
                crate::i18n::t!("toolbar.spotlight_outline").into_owned(),
                self.focus.clone(),
                move |_, cx| {
                    session.update(cx, |s, cx| {
                        s.edit_annotations(|a| a.set_spotlight_outline(!enabled));
                        cx.notify();
                    })
                },
            )
            .selected(enabled)
            .child(annot_options::menu_icon(IconName::SquareDashed, 18.)),
        );
        if enabled {
            let slider = state.read(cx).width.clone();
            row = row.child(
                Popover::new("spotlight-outline-options")
                    .trigger(
                        control(
                            "spotlight-outline-options".into(),
                            crate::i18n::t!("toolbar.spotlight_outline_width").into_owned(),
                            self.focus,
                            |_, _| {},
                        )
                        .on_click(|_, _, _| {})
                        .w_auto()
                        .px_1()
                        .child(
                            svg()
                                .data(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 18 18'><path d='M2 4H16' stroke='black'/><path d='M2 9H16' stroke='black' stroke-width='2'/><path d='M2 14H16' stroke='black' stroke-width='3'/></svg>")
                                .size(px(18.))
                                .text_color(rgba(theme::c().toolbar_text)),
                        )
                        .child(format!("{width:.0}")),
                    )
                    .content(move |_, _, cx| {
                        let sc = SizeSlider {
                            current: slider.read(cx).value().start(),
                            percentage: slider.read(cx).percentage().start,
                            state: slider.clone(),
                        };
                        annotation_menu_panel()
                            .flex()
                            .items_center()
                            .child(size_control(&sc, ""))
                    }),
            );
            if let Some(color) = self.color {
                row = row.child(ColorPicker::new(
                    &color,
                    crate::i18n::t!("toolbar.spotlight_outline").into_owned(),
                ));
            }
        }
        row
    }
}
