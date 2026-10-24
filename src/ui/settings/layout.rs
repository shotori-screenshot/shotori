//! Shared settings form anatomy. The scroll owner spans the pane; its
//! inner column owns the gutter so the scrollbar stays at the pane edge.
use super::*;
use gpui_kit::prelude::FluentBuilder;

#[derive(Clone, Copy)]
struct SwitchGeometry {
    width: Pixels,
    height: Pixels,
    thumb: Pixels,
    inset: Pixels,
}

fn switch_geometry(rem: Pixels, scale: f32) -> SwitchGeometry {
    // Equal device-pixel insets keep both edges symmetric at fractional scale.
    let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
    let height = snap(rem * 1.25);
    let inset = snap(rem * 0.125);
    SwitchGeometry {
        width: snap(rem * 2.25),
        height,
        thumb: height - inset * 2.,
        inset,
    }
}

/// Native switch interaction with symmetric, device-aligned track and thumb bounds.
pub(super) fn settings_switch(
    id: &'static str,
    checked: bool,
    label: String,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Switch {
    use gpui_kit::base::{Switch, SwitchThumb, SwitchTrack, spring};

    let id = ElementId::from(id);
    let focus = window
        .use_keyed_state((id.clone(), "focus"), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let focused = focus.is_focused(window) && window.last_input_was_keyboard();
    let geometry = switch_geometry(window.rem_size(), window.scale_factor());
    let thumb_x = spring(
        (id.clone(), "thumb"),
        if checked {
            geometry.width - geometry.inset - geometry.thumb
        } else {
            geometry.inset
        },
        cx.theme().motion_tokens().spring_move,
        window,
        cx,
    );
    Switch::new(id.clone())
        .checked(checked)
        .accessibility_label(label)
        .track_focus(&focus)
        .child(
            SwitchTrack::new((id, "track"))
                .debug_selector(|| "settings-switch-track".into())
                .relative()
                .w(geometry.width)
                .h(geometry.height)
                .rounded_full()
                .bg(if checked {
                    cx.theme().primary
                } else {
                    cx.theme().switch
                })
                .child(
                    div()
                        .absolute()
                        .debug_selector(|| "settings-switch-thumb".into())
                        .top(geometry.inset)
                        .left(thumb_x)
                        .size(geometry.thumb)
                        .child(
                            SwitchThumb::new(checked)
                                .size_full()
                                .rounded_full()
                                .bg(cx.theme().switch_thumb),
                        ),
                )
                .when(focused, |track| {
                    track.child(
                        div()
                            .absolute()
                            .top(-geometry.inset)
                            .bottom(-geometry.inset)
                            .left(-geometry.inset)
                            .right(-geometry.inset)
                            .rounded_full()
                            .border_2()
                            .border_color(cx.theme().ring.opacity(0.6)),
                    )
                }),
        )
}

pub(super) fn content_column() -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w_full()
        .max_w(rems(CONTENT_MAX))
        .mx_auto()
        .px_6()
        .py_5()
        .gap_4()
}

/// Page title with an optional inline [`super::help_tooltip`] mark.
/// Both arguments are i18n keys. Only pages whose blurb teaches
/// something the page itself does not show (an interaction, a restart
/// question) keep a mark: a subtitle that restates the title next to
/// the rows that already say it is noise, not help.
pub(super) fn page_header(
    cx: &App,
    title_key: &'static str,
    hint_key: Option<&'static str>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child(crate::i18n::t!(title_key).to_string()),
        )
        .children(hint_key.map(|key| super::help_tooltip(key, cx)))
}

/// Section captions share a compact divider above their form rows.
pub(super) fn card_group(
    cx: &App,
    caption: Option<&str>,
    rows: impl IntoIterator<Item = impl IntoElement>,
) -> Div {
    let mut group = div().flex().flex_col().gap_1().w_full().min_w_0();
    if let Some(caption) = caption {
        group = group.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .w_full()
                .pb_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(caption.to_owned()),
        );
    }
    let mut body = div().flex().flex_col().w_full().min_w_0();
    for row in rows {
        body = body.child(row);
    }
    group.child(body)
}

/// Wrapping is driven by available layout width, not a cached window size.
/// At narrow widths the control drops below its label instead of squeezing it.
///
/// Rows carry two optional annotations with different jobs: `hint` is
/// a static i18n key collapsed into the label's circled-? hover
/// tooltip; `description` is live state (model readiness, the cache
/// path, a factory keystroke) that must stay visible — hiding state
/// behind a hover removes information the row exists to show.
pub(super) fn card_row(
    cx: &App,
    label: impl Into<SharedString>,
    hint: Option<&'static str>,
    description: Option<String>,
    control: impl IntoElement,
) -> Div {
    form_row(cx, label, hint, description, control, true)
}

/// Switches and icon actions reserve only their actual width; fields use a
/// shared column. Reserving a field-sized slot for a switch squeezes prose.
pub(super) fn compact_row(
    cx: &App,
    label: impl Into<SharedString>,
    hint: Option<&'static str>,
    description: Option<String>,
    control: impl IntoElement,
) -> Div {
    form_row(cx, label, hint, description, control, false)
}

fn form_row(
    cx: &App,
    label: impl Into<SharedString>,
    hint: Option<&'static str>,
    description: Option<String>,
    control: impl IntoElement,
    field: bool,
) -> Div {
    let mut heading = div()
        .flex()
        .items_center()
        .gap_1()
        .text_sm()
        .text_color(cx.theme().foreground)
        .child(label.into());
    if let Some(key) = hint {
        heading = heading.child(super::help_tooltip(key, cx));
    }
    let mut label = div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .flex_basis(rems(18.))
        .min_w(rems(10.))
        .child(heading);
    if let Some(description) = description {
        label = label.child(
            div()
                .text_sm()
                .line_height(relative(1.5))
                .text_color(cx.theme().muted_foreground)
                .child(description),
        );
    }
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_4()
        .py_2()
        .w_full()
        .min_w_0()
        .child(label)
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .when(field, |row| row.w_56())
                .max_w_full()
                .flex_shrink_0()
                .child(control),
        )
}

pub(super) fn page_body(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
    // Inner padding belongs to the content column, not the scroll viewport.
}

#[cfg(test)]
mod tests {
    use super::{card_row, settings_switch, switch_geometry};
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
        ParentElement, Render, Styled, TestAppContext, Window, div, px,
    };

    struct SwitchFixture {
        checked: bool,
        rem: f32,
    }

    impl Render for SwitchFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(self.rem));
            let entity = cx.entity();
            settings_switch("feature", self.checked, "Feature".into(), window, cx).on_change(
                move |checked, _, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.checked = checked;
                        cx.notify();
                    });
                },
            )
        }
    }

    #[test]
    fn switch_edges_keep_equal_device_pixel_insets_at_fractional_scales() {
        for rem in [16., 18., 20., 24.] {
            for scale in [1., 1.25, 1.5, 1.75, 2.] {
                let geometry = switch_geometry(px(rem), scale);
                let height = f32::from(geometry.height) * scale;
                let thumb = f32::from(geometry.thumb) * scale;
                let inset = f32::from(geometry.inset) * scale;
                assert!((height - thumb - inset * 2.).abs() < 0.001);
                assert!((height - height.round()).abs() < 0.001);
                assert!((thumb - thumb.round()).abs() < 0.001);
                assert!((inset - inset.round()).abs() < 0.001);
            }
        }
    }

    #[gpui_kit::test]
    fn switch_thumb_stays_centered_and_native_activation_updates_the_value(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| SwitchFixture {
            checked: false,
            rem: 16.,
        });
        for rem in [16., 20., 24.] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.rem = rem;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let track = cx
                .debug_bounds("settings-switch-track")
                .expect("switch track");
            let thumb = cx
                .debug_bounds("settings-switch-thumb")
                .expect("switch thumb");
            assert!(
                (thumb.top() - track.top() - (track.bottom() - thumb.bottom())).abs() < px(0.01)
            );
            cx.simulate_click(track.center(), Modifiers::default());
            cx.update(|window, cx| {
                assert!(view.read(cx).checked);
                window.draw(cx).clear(cx);
            });
            let keystroke = Keystroke::parse("space").expect("space key");
            cx.simulate_event(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(KeyUpEvent { keystroke });
            cx.update(|window, cx| {
                assert!(!view.read(cx).checked);
                window.draw(cx).clear(cx);
            });
        }
    }

    struct FormFixture {
        width: f32,
        rem: f32,
    }

    impl Render for FormFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(self.rem));
            div().w(px(self.width)).child(
                card_row(
                    cx,
                    "Language",
                    None,
                    Some("Choose the interface language".to_string()),
                    div().debug_selector(|| "control".into()).w_full().h_8(),
                )
                .debug_selector(|| "row".into()),
            )
        }
    }

    #[gpui_kit::test]
    fn form_controls_wrap_without_horizontal_overflow_at_larger_type(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| FormFixture {
            width: 700.,
            rem: 16.,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let wide_row = cx.debug_bounds("row").expect("form row should render");
        let wide_control = cx.debug_bounds("control").expect("control should render");
        for rem in [16., 20., 24.] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.width = 360.;
                    view.rem = rem;
                    cx.notify();
                })
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            let row = cx
                .debug_bounds("row")
                .expect("narrow form row should render");
            let control = cx
                .debug_bounds("control")
                .expect("narrow control should render");
            assert!(control.left() >= row.left());
            assert!(control.right() <= row.right());
            assert!(control.bottom() <= row.bottom());
            assert!(
                control.top() > wide_control.top(),
                "control must wrap below its label"
            );
            assert!(row.size.height > wide_row.size.height);
        }
    }
}
