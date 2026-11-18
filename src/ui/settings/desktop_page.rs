//! Notifications & Floating Pins settings page: desktop toast banners with
//! live test trigger, and floating pin appearance preview, opacity, double-click
//! action, zoom, border, shadow, and edge snapping.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use crate::model::settings::{
    NotifySettings, PinDoubleClick, PinOpacityModifier, PinSettings, PinZoomModifier, Settings,
};
use crate::ui::theme;

use super::{SettingsWindow, card_row, page_body, page_header};

#[derive(Clone)]
pub(super) struct PinZoomModifierItem(PinZoomModifier);

impl SearchableListItem for PinZoomModifierItem {
    type Value = PinZoomModifier;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct PinDoubleClickItem(PinDoubleClick);

impl SearchableListItem for PinDoubleClickItem {
    type Value = PinDoubleClick;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct PinOpacityModifierItem(PinOpacityModifier);

impl SearchableListItem for PinOpacityModifierItem {
    type Value = PinOpacityModifier;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

pub(super) struct DesktopPageState {
    notify_timeout_slider: Entity<SliderState>,
    pin_opacity_slider: Entity<SliderState>,
    pin_double_click_select: Entity<SelectState<SearchableVec<PinDoubleClickItem>>>,
    pin_zoom_modifier_select: Entity<SelectState<SearchableVec<PinZoomModifierItem>>>,
    pin_opacity_modifier_select: Entity<SelectState<SearchableVec<PinOpacityModifierItem>>>,
    pin_zoom_step_slider: Entity<SliderState>,
    pin_max_history_slider: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl DesktopPageState {
    pub(super) fn new(
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        let mut subs = Vec::new();

        let notify_timeout_slider = cx.new(|_| {
            SliderState::new()
                .max(30.)
                .min(2.)
                .step(1.)
                .default_value(f32::from(settings.notify().timeout_secs))
        });
        subs.push(cx.subscribe(&notify_timeout_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let secs = value.round().clamp(2., 30.) as u8;
                if this.settings.notify().timeout_secs != secs {
                    this.settings.notify_mut().timeout_secs = secs;
                    this.persist(cx);
                }
            }
        }));

        let pin_opacity_slider = cx.new(|_| {
            SliderState::new()
                .max(100.)
                .min(20.)
                .step(5.)
                .default_value(f32::from(settings.pin().opacity))
        });
        subs.push(cx.subscribe(&pin_opacity_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let opacity = value.round().clamp(20., 100.) as u8;
                if this.settings.pin().opacity != opacity {
                    this.settings.pin_mut().opacity = opacity;
                    this.persist(cx);
                }
            }
        }));

        let dc_current = settings.pin().double_click;
        let pin_double_click_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(PinDoubleClick::ALL.map(PinDoubleClickItem).to_vec()),
                PinDoubleClick::ALL
                    .iter()
                    .position(|d| *d == dc_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(
            cx.subscribe(&pin_double_click_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(dc)) = event
                    && this.settings.pin().double_click != *dc
                {
                    this.settings.pin_mut().double_click = *dc;
                    this.persist(cx);
                }
            }),
        );

        let modif_current = settings.pin().zoom_modifier;
        let pin_zoom_modifier_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(PinZoomModifier::ALL.map(PinZoomModifierItem).to_vec()),
                PinZoomModifier::ALL
                    .iter()
                    .position(|m| *m == modif_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(
            cx.subscribe(&pin_zoom_modifier_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(modif)) = event
                    && this.settings.pin().zoom_modifier != *modif
                {
                    this.settings.pin_mut().zoom_modifier = *modif;
                    this.persist(cx);
                }
            }),
        );

        let opacity_mod_current = settings.pin().opacity_modifier;
        let pin_opacity_modifier_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(PinOpacityModifier::ALL.map(PinOpacityModifierItem).to_vec()),
                PinOpacityModifier::ALL
                    .iter()
                    .position(|m| *m == opacity_mod_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(
            cx.subscribe(&pin_opacity_modifier_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(modif)) = event
                    && this.settings.pin().opacity_modifier != *modif
                {
                    this.settings.pin_mut().opacity_modifier = *modif;
                    this.persist(cx);
                }
            }),
        );

        let pin_zoom_step_slider = cx.new(|_| {
            SliderState::new()
                .max(50.)
                .min(5.)
                .step(1.)
                .default_value(f32::from(settings.pin().zoom_step))
        });
        subs.push(cx.subscribe(&pin_zoom_step_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let step = value.round().clamp(5., 50.) as u8;
                if this.settings.pin().zoom_step != step {
                    this.settings.pin_mut().zoom_step = step;
                    this.persist(cx);
                }
            }
        }));

        let pin_max_history_slider = cx.new(|_| {
            SliderState::new()
                .max(200.)
                .min(0.)
                .step(5.)
                .default_value(f32::from(settings.pin().max_history))
        });
        subs.push(cx.subscribe(&pin_max_history_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let cap = value.round().clamp(0., 200.) as u16;
                if this.settings.pin().max_history != cap {
                    this.settings.pin_mut().max_history = cap;
                    this.persist(cx);
                }
            }
        }));

        Self {
            notify_timeout_slider,
            pin_opacity_slider,
            pin_double_click_select,
            pin_zoom_modifier_select,
            pin_opacity_modifier_select,
            pin_zoom_step_slider,
            pin_max_history_slider,
            _subscriptions: subs,
        }
    }

    pub(super) fn resync(&self, settings: &Settings, window: &mut Window, cx: &mut App) {
        self.notify_timeout_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.notify().timeout_secs), window, cx)
        });
        self.pin_opacity_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.pin().opacity), window, cx)
        });
        let dc_ix = PinDoubleClick::ALL
            .iter()
            .position(|d| *d == settings.pin().double_click)
            .map(IndexPath::new);
        self.pin_double_click_select
            .update(cx, |s, cx| s.set_selected_index(dc_ix, window, cx));
        let modif_ix = PinZoomModifier::ALL
            .iter()
            .position(|m| *m == settings.pin().zoom_modifier)
            .map(IndexPath::new);
        self.pin_zoom_modifier_select
            .update(cx, |s, cx| s.set_selected_index(modif_ix, window, cx));
        let op_modif_ix = PinOpacityModifier::ALL
            .iter()
            .position(|m| *m == settings.pin().opacity_modifier)
            .map(IndexPath::new);
        self.pin_opacity_modifier_select
            .update(cx, |s, cx| s.set_selected_index(op_modif_ix, window, cx));
        self.pin_zoom_step_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.pin().zoom_step), window, cx)
        });
        self.pin_max_history_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.pin().max_history), window, cx)
        });
    }
}

impl SettingsWindow {
    fn reset_desktop_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        *self.settings.notify_mut() = NotifySettings::default();
        *self.settings.pin_mut() = PinSettings::default();
        self.desktop_page.resync(&self.settings, window, cx);
        self.persist(cx);
    }

    fn bool_desktop_switch(
        &self,
        id: &'static str,
        checked: bool,
        title_key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Settings, bool) + 'static,
    ) -> Switch {
        let entity = cx.entity();
        super::layout::settings_switch(
            id,
            checked,
            crate::i18n::t!(title_key).to_string(),
            window,
            cx,
        )
        .on_change(move |checked, _, _, cx| {
            entity.update(cx, |this, cx| {
                apply(&mut this.settings, checked);
                this.persist(cx);
            });
        })
    }

    /// Live miniature preview of a floating pin window reflecting border,
    /// shadow halo, and initial opacity in real time.
    fn pin_preview_row(&self, p: &PinSettings, cx: &App) -> Div {
        let s = super::palette::surfaces(cx);
        let palette = theme::c();
        let opacity = (f32::from(p.opacity) / 100.).clamp(0.2, 1.0);
        let shadows = if p.shadow {
            vec![
                BoxShadow::new(
                    px(0.),
                    px(0.),
                    rgba((palette.accent & 0xFFFF_FF00) | 0x8C).into(),
                )
                .blur_radius(px(5.)),
                BoxShadow::new(
                    px(0.),
                    px(2.),
                    rgba((palette.accent & 0xFFFF_FF00) | 0x40).into(),
                )
                .blur_radius(px(16.)),
            ]
        } else {
            Vec::new()
        };

        let mut mini_pin = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .bg(rgba(palette.toolbar_bg))
            .text_xs()
            .text_color(rgba(palette.toolbar_text))
            .opacity(opacity)
            .shadow(shadows);
        if p.show_border {
            mini_pin = mini_pin.border_1().border_color(rgba(palette.pin_border));
        }

        let stage = div()
            .flex()
            .items_center()
            .justify_center()
            .px_5()
            .py_3()
            .rounded(px(11.))
            .bg(s.well_bg)
            .border_1()
            .border_color(s.hairline)
            .child(
                mini_pin
                    .child(
                        Icon::new(IconName::Scan)
                            .size_3p5()
                            .text_color(rgba(palette.accent)),
                    )
                    .child(
                        crate::i18n::t!("settings.pin.preview.sample", opacity = p.opacity)
                            .to_string(),
                    ),
            );

        super::layout::compact_row(
            cx,
            crate::i18n::t!("settings.pin.preview.label").to_string(),
            None,
            None,
            stage,
        )
    }

    pub(super) fn page_desktop(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let s = super::palette::surfaces(cx);
        let n = self.settings.notify();
        let p = self.settings.pin();
        let notify_off = !n.enabled;

        let notify_enabled = self.bool_desktop_switch(
            "notify-enabled",
            n.enabled,
            "settings.notify.enabled.title",
            window,
            cx,
            |s, v| s.notify_mut().enabled = v,
        );
        let notify_thumb = self
            .bool_desktop_switch(
                "notify-show-thumbnail",
                n.show_thumbnail,
                "settings.notify.show_thumbnail.title",
                window,
                cx,
                |s, v| s.notify_mut().show_thumbnail = v,
            )
            .disabled(notify_off);

        let primary = cx.theme().primary;
        let on_primary = cx.theme().primary_foreground;
        let ring = cx.theme().ring;
        let unselected_bg = s.well_bg;
        let selected_bg = s.selected_card_bg;
        let hairline = s.hairline;
        let event_chip =
            |id: &'static str, label_key: &'static str, active: bool, toggle: fn(&mut Settings)| {
                let entity = cx.entity();
                let label = crate::i18n::t!(label_key).to_string();
                let badge = if active {
                    div()
                        .size_3p5()
                        .rounded_full()
                        .bg(primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Check)
                                .size(px(9.))
                                .text_color(on_primary),
                        )
                } else {
                    div()
                        .size_3p5()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Minus)
                                .size_3()
                                .text_color(cx.theme().muted_foreground),
                        )
                };
                Button::new(id)
                    .accessibility_label(label.clone())
                    .disabled(notify_off)
                    .selected(active)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .h_7()
                    .rounded_full()
                    .border_1()
                    .border_color(if active { primary } else { hairline })
                    .bg(if active { selected_bg } else { unselected_bg })
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .hover(move |st| st.border_color(primary))
                    .focus_visible(move |st| st.border_color(ring))
                    .child(badge)
                    .child(label)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            toggle(&mut this.settings);
                            this.persist(cx);
                        });
                    })
            };

        let event_pills = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_1p5()
            .child(event_chip(
                "notify-pill-copy",
                "settings.notify.events.copy",
                n.on_copy,
                |s| s.notify_mut().on_copy = !s.notify().on_copy,
            ))
            .child(event_chip(
                "notify-pill-save",
                "settings.notify.events.save",
                n.on_save,
                |s| s.notify_mut().on_save = !s.notify().on_save,
            ))
            .child(event_chip(
                "notify-pill-ocr",
                "settings.notify.events.ocr",
                n.on_ocr,
                |s| s.notify_mut().on_ocr = !s.notify().on_ocr,
            ))
            .child(event_chip(
                "notify-pill-qr",
                "settings.notify.events.qr",
                n.on_qr,
                |s| s.notify_mut().on_qr = !s.notify().on_qr,
            ));

        let test_toast_btn = {
            let secs = n.timeout_secs;
            super::layout::action_button(
                cx,
                "notify-send-test",
                crate::i18n::t!("settings.notify.test_btn").to_string(),
                move |_, _, _| {
                    crate::notify::send_test(
                        crate::i18n::t!("settings.notify.test_title").as_ref(),
                        crate::i18n::t!("settings.notify.test_body", secs = secs).as_ref(),
                        secs,
                    );
                },
            )
            .into_any_element()
        };

        let pin_snap = self.bool_desktop_switch(
            "pin-snap-to-edge",
            p.snap_to_edge,
            "settings.pin.snap_to_edge.title",
            window,
            cx,
            |s, v| s.pin_mut().snap_to_edge = v,
        );
        let pin_snap_siblings = self.bool_desktop_switch(
            "pin-snap-to-pins",
            p.snap_to_pins,
            "settings.pin.snap_to_pins.title",
            window,
            cx,
            |s, v| s.pin_mut().snap_to_pins = v,
        );
        let pin_border = self.bool_desktop_switch(
            "pin-show-border",
            p.show_border,
            "settings.pin.show_border.title",
            window,
            cx,
            |s, v| s.pin_mut().show_border = v,
        );
        let pin_shadow = self.bool_desktop_switch(
            "pin-shadow",
            p.shadow,
            "settings.pin.shadow.title",
            window,
            cx,
            |s, v| s.pin_mut().shadow = v,
        );

        let open_history_btn = super::layout::action_button(
            cx,
            "pin-open-history",
            crate::i18n::t!("settings.pin.open_history_btn").to_string(),
            |_, _, _| {
                crate::launch_detached_args(&["pin", "--history"]);
            },
        )
        .into_any_element();

        let history_readout = if p.max_history == 0 {
            crate::i18n::t!("settings.pin.max_history.disabled").to_string()
        } else {
            crate::i18n::t!("settings.pin.max_history.count", count = p.max_history).to_string()
        };

        let timeout_readout =
            crate::i18n::t!("settings.notify.timeout_secs.unit", secs = n.timeout_secs).to_string();

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.preferences.desktop",
                        Some("settings.desktop.blurb"),
                    ))
                    .child(super::shell::reset_button("settings.desktop.reset", cx, {
                        let entity = cx.entity();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.reset_desktop_page(window, cx);
                            });
                        }
                    })),
            )
            .child(super::layout::card_group_with_action(
                cx,
                Some(crate::i18n::t!("settings.notify.group").as_ref()),
                Some(test_toast_btn),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.enabled.title").to_string(),
                        Some("settings.notify.enabled.body"),
                        None,
                        notify_enabled,
                    ),
                    super::layout::dim_when(
                        notify_off,
                        super::layout::compact_row(
                            cx,
                            crate::i18n::t!("settings.notify.events.title").to_string(),
                            None,
                            None,
                            event_pills,
                        ),
                    ),
                    super::layout::dim_when(
                        notify_off,
                        super::layout::compact_row(
                            cx,
                            crate::i18n::t!("settings.notify.show_thumbnail.title").to_string(),
                            None,
                            None,
                            notify_thumb,
                        ),
                    ),
                    super::layout::dim_when(
                        notify_off,
                        card_row(
                            cx,
                            crate::i18n::t!("settings.notify.timeout_secs.title").to_string(),
                            None,
                            None,
                            super::layout::slider_control_disabled(
                                &self.desktop_page.notify_timeout_slider,
                                timeout_readout,
                                notify_off,
                                cx,
                            ),
                        ),
                    ),
                ],
            ))
            .child(super::layout::card_group_with_action(
                cx,
                Some(crate::i18n::t!("settings.pin.group").as_ref()),
                Some(open_history_btn),
                vec![
                    self.pin_preview_row(p, cx),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.opacity.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.desktop_page.pin_opacity_slider,
                            format!("{}%", p.opacity),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.double_click_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.desktop_page.pin_double_click_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.zoom_modifier_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.desktop_page.pin_zoom_modifier_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.opacity_modifier_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.desktop_page.pin_opacity_modifier_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.zoom_step.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.desktop_page.pin_zoom_step_slider,
                            format!("{}%", p.zoom_step),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.max_history.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.desktop_page.pin_max_history_slider,
                            history_readout,
                            cx,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.pin.snap_to_edge.title").to_string(),
                        Some("settings.pin.snap_to_edge.body"),
                        None,
                        pin_snap,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.pin.snap_to_pins.title").to_string(),
                        Some("settings.pin.snap_to_pins.body"),
                        None,
                        pin_snap_siblings,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.pin.show_border.title").to_string(),
                        None,
                        None,
                        pin_border,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.pin.shadow.title").to_string(),
                        None,
                        None,
                        pin_shadow,
                    ),
                ],
            ));

        page_body("page-desktop").child(body)
    }
}
