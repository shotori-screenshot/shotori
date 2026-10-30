//! The sequence value editor retains focus while the model owns numbering and history.
use super::*;
use gpui_kit::component::{
    Sizable,
    input::{Input, InputEvent, InputState},
};

#[derive(IntoElement)]
pub(super) struct NumberValue {
    session: Entity<ScreenshotSession>,
}

impl NumberValue {
    pub(super) fn new(session: &Entity<ScreenshotSession>) -> Self {
        Self {
            session: session.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueTarget {
    Next,
    Badge(usize),
}

fn value_target(session: &ScreenshotSession) -> ValueTarget {
    session
        .annotations()
        .selected_index()
        .map_or(ValueTarget::Next, ValueTarget::Badge)
}

struct ValueState {
    input: Entity<InputState>,
    target: ValueTarget,
    _subscriptions: Vec<Subscription>,
}

impl ValueState {
    fn new(
        session: Entity<ScreenshotSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value = session.read(cx).annotations().number_value().to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let input_session = session.clone();
        let commit = cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
            if matches!(event, InputEvent::Focus) {
                this.target = value_target(input_session.read(cx));
                return;
            }
            if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            if this.target == value_target(input_session.read(cx))
                && let Ok(value) = input.read(cx).value().trim().parse::<u32>()
            {
                input_session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.set_number_value(value));
                    cx.notify();
                });
            }
            let value = input_session
                .read(cx)
                .annotations()
                .number_value()
                .to_string();
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        });
        let sync = cx.observe_in(&session, window, |this, session, window, cx| {
            let target = value_target(session.read(cx));
            if this.target != target || !this.input.read(cx).focus_handle(cx).is_focused(window) {
                this.target = target;
                let value = session.read(cx).annotations().number_value().to_string();
                if this.input.read(cx).value().as_ref() != value {
                    this.input
                        .update(cx, |input, cx| input.set_value(value, window, cx));
                }
            }
            cx.notify();
        });
        Self {
            target: value_target(session.read(cx)),
            input,
            _subscriptions: vec![commit, sync],
        }
    }
}

impl RenderOnce for NumberValue {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state("annotation-number-value", cx, |window, cx| {
            ValueState::new(self.session, window, cx)
        });
        div()
            .debug_selector(|| "tb-number-value".into())
            .on_action(|_: &gpui_kit::base::input::Enter, _, cx| cx.stop_propagation())
            .child(
                Input::new(&state.read(cx).input)
                    .small()
                    .w_20()
                    .appearance(false)
                    .aria_label(crate::i18n::t!("toolbar.number_value").into_owned()),
            )
    }
}
