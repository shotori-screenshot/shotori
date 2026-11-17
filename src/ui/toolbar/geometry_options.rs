//! Rectangle corners have a dedicated control; the shared slider always edits stroke width.
use super::*;
use gpui_kit::{base::slider::SliderEvent, component::popover::Popover};

#[derive(IntoElement)]
pub(super) struct CornerRadius {
    style: UiStyle,
    session: Entity<ScreenshotSession>,
    focus: FocusHandle,
}

impl CornerRadius {
    pub(super) fn new(
        style: UiStyle,
        session: &Entity<ScreenshotSession>,
        focus: &FocusHandle,
    ) -> Self {
        Self {
            style,
            session: session.clone(),
            focus: focus.clone(),
        }
    }
}

struct RadiusState {
    slider: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl RadiusState {
    fn new(
        session: Entity<ScreenshotSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let radius = session.read(cx).annotations().geometry_radius();
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(crate::annotation::Annotations::MAX_CORNER_RADIUS)
                .step(1.)
                .default_value(radius)
        });
        let change_session = session.clone();
        let change = cx.subscribe(&slider, move |_, _, event, cx| {
            change_session.update(cx, |s, cx| {
                s.edit_annotation_settings(|a| match event {
                    SliderEvent::Change(value) => a.set_geometry_radius(value.start()),
                    SliderEvent::Release(_) => a.end_size_drag(),
                });
                cx.notify();
            });
        });
        let sync = cx.observe_in(&session, window, |this, session, window, cx| {
            let radius = session.read(cx).annotations().geometry_radius();
            if this.slider.read(cx).value().start() != radius {
                this.slider
                    .update(cx, |s, cx| s.set_value(radius, window, cx));
            }
            cx.notify();
        });
        Self {
            slider,
            _subscriptions: vec![change, sync],
        }
    }
}

impl RenderOnce for CornerRadius {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = self.style;
        let session = self.session.clone();
        let state = window.use_keyed_state("annotation-corner-radius", cx, |window, cx| {
            RadiusState::new(session, window, cx)
        });
        let radius = self.session.read(cx).annotations().geometry_radius();
        let slider = state.read(cx).slider.clone();
        let focus = self.focus.clone();
        let t = theme::c();
        let mut pop = Popover::new("corner-radius").p(px(0.));
        pop = match style {
            UiStyle::Classic => pop.bg(rgba(t.toolbar_bg)),
            UiStyle::Capsule => pop
                .rounded(px(12.))
                .bg(rgba(t.capsule_sheet_surface()))
                .border_1()
                .border_color(rgba(t.capsule_border()))
                .shadow(theme::capsule_floating_shadow()),
        };
        pop.on_open_change(move |open, window, cx| {
            if !open {
                window.focus(&focus, cx);
            }
            self.session.update(cx, |s, _| {
                s.edit_annotation_settings(|a| a.end_size_drag());
            });
        })
        .trigger(
            control_for(
                style,
                "corner-radius".into(),
                crate::i18n::t!("toolbar.corner_radius").into_owned(),
                self.focus,
                |_, _| {},
            )
            .on_click(|_, _, _| {})
            .w_auto()
            .px(px(6.))
            .gap(px(4.))
            .child(
                svg()
                    .data(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 18 18'><path d='M3 15V9a6 6 0 0 1 6-6h6' fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round'/></svg>")
                    .size(px(18.))
                    .text_color(rgba(t.toolbar_text)),
            )
            .child(div().text_size(px(12.)).child(format!("{radius:.0}"))),
        )
        .content(move |_, _, cx| {
            let sc = SizeSlider {
                current: slider.read(cx).value().start(),
                percentage: slider.read(cx).percentage().end,
                state: slider.clone(),
            };
            annotation_menu_panel()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(12.))
                .child(crate::i18n::t!("toolbar.corner_radius").into_owned())
                .child(
                    div()
                        .debug_selector(|| "corner-radius-slider".into())
                        .child(size_control_for(style, &sc, "")),
                )
        })
    }
}
