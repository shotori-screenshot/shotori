//! Notifications & Floating Pins settings page: desktop toast banners and
//! floating pin zoom, border, shadow, and edge snapping.

use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::model::settings::{NotifySettings, PinSettings, PinZoomModifier, Settings};

use super::{SettingsWindow, card_group, card_row, page_body, page_header};

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

pub(super) struct DesktopPageState {
    notify_timeout_slider: Entity<SliderState>,
    pin_zoom_modifier_select: Entity<SelectState<SearchableVec<PinZoomModifierItem>>>,
    pin_zoom_step_slider: Entity<SliderState>,
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

        Self {
            notify_timeout_slider,
            pin_zoom_modifier_select,
            pin_zoom_step_slider,
            _subscriptions: subs,
        }
    }

    pub(super) fn resync(&self, settings: &Settings, window: &mut Window, cx: &mut App) {
        self.notify_timeout_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.notify().timeout_secs), window, cx)
        });
        let modif_ix = PinZoomModifier::ALL
            .iter()
            .position(|m| *m == settings.pin().zoom_modifier)
            .map(IndexPath::new);
        self.pin_zoom_modifier_select
            .update(cx, |s, cx| s.set_selected_index(modif_ix, window, cx));
        self.pin_zoom_step_slider.update(cx, |s, cx| {
            s.set_value(f32::from(settings.pin().zoom_step), window, cx)
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

    pub(super) fn page_desktop(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let n = self.settings.notify();
        let p = self.settings.pin();

        let notify_enabled = self.bool_desktop_switch(
            "notify-enabled",
            n.enabled,
            "settings.notify.enabled.title",
            window,
            cx,
            |s, v| s.notify_mut().enabled = v,
        );
        let notify_copy = self.bool_desktop_switch(
            "notify-on-copy",
            n.on_copy,
            "settings.notify.on_copy.title",
            window,
            cx,
            |s, v| s.notify_mut().on_copy = v,
        );
        let notify_save = self.bool_desktop_switch(
            "notify-on-save",
            n.on_save,
            "settings.notify.on_save.title",
            window,
            cx,
            |s, v| s.notify_mut().on_save = v,
        );
        let notify_ocr = self.bool_desktop_switch(
            "notify-on-ocr",
            n.on_ocr,
            "settings.notify.on_ocr.title",
            window,
            cx,
            |s, v| s.notify_mut().on_ocr = v,
        );
        let notify_thumb = self.bool_desktop_switch(
            "notify-show-thumbnail",
            n.show_thumbnail,
            "settings.notify.show_thumbnail.title",
            window,
            cx,
            |s, v| s.notify_mut().show_thumbnail = v,
        );

        let pin_snap = self.bool_desktop_switch(
            "pin-snap-to-edge",
            p.snap_to_edge,
            "settings.pin.snap_to_edge.title",
            window,
            cx,
            |s, v| s.pin_mut().snap_to_edge = v,
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

        let timeout_readout =
            crate::i18n::t!("settings.notify.timeout_secs.unit", secs = n.timeout_secs).to_string();

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_start()
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
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.notify.group").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.enabled.title").to_string(),
                        Some("settings.notify.enabled.body"),
                        None,
                        notify_enabled,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.on_copy.title").to_string(),
                        None,
                        None,
                        notify_copy,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.on_save.title").to_string(),
                        None,
                        None,
                        notify_save,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.on_ocr.title").to_string(),
                        None,
                        None,
                        notify_ocr,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.notify.show_thumbnail.title").to_string(),
                        None,
                        None,
                        notify_thumb,
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.notify.timeout_secs.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.desktop_page.notify_timeout_slider,
                            timeout_readout,
                            cx,
                        ),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.pin.group").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.pin.zoom_modifier_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.desktop_page.pin_zoom_modifier_select).w_full(),
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
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.pin.snap_to_edge.title").to_string(),
                        Some("settings.pin.snap_to_edge.body"),
                        None,
                        pin_snap,
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
