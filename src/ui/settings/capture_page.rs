//! Capture & Toolbar settings page: selection HUD, corner loupe, post-exit
//! behavior, long-screenshot defaults, and the drag-and-drop toolbar layout.

use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::model::settings::{LoupeSize, ScrollAxisSetting, Settings, SizeLabelMode};

use super::{SettingsWindow, card_group, card_row, page_body, page_header};

#[derive(Clone)]
pub(super) struct SizeLabelItem(SizeLabelMode);

impl SearchableListItem for SizeLabelItem {
    type Value = SizeLabelMode;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct LoupeSizeItem(LoupeSize);

impl SearchableListItem for LoupeSizeItem {
    type Value = LoupeSize;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct ScrollAxisItem(ScrollAxisSetting);

impl SearchableListItem for ScrollAxisItem {
    type Value = ScrollAxisSetting;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

pub(super) struct CapturePageState {
    size_label_select: Entity<SelectState<SearchableVec<SizeLabelItem>>>,
    loupe_zoom_slider: Entity<SliderState>,
    loupe_size_select: Entity<SelectState<SearchableVec<LoupeSizeItem>>>,
    scroll_axis_select: Entity<SelectState<SearchableVec<ScrollAxisItem>>>,
    scroll_max_length_slider: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl CapturePageState {
    pub(super) fn new(
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        let mut subs = Vec::new();

        let size_label_current = settings.capture().size_label;
        let size_label_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(SizeLabelMode::ALL.map(SizeLabelItem).to_vec()),
                SizeLabelMode::ALL
                    .iter()
                    .position(|m| *m == size_label_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&size_label_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(mode)) = event
                && this.settings.capture().size_label != *mode
            {
                this.settings.capture_mut().size_label = *mode;
                this.persist(cx);
            }
        }));

        let loupe_zoom_slider = cx.new(|_| {
            SliderState::new()
                .max(8.)
                .min(2.)
                .step(1.)
                .default_value(f32::from(settings.capture().loupe_zoom))
        });
        subs.push(cx.subscribe(&loupe_zoom_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let zoom = value.round().clamp(2., 8.) as u8;
                if this.settings.capture().loupe_zoom != zoom {
                    this.settings.capture_mut().loupe_zoom = zoom;
                    this.persist(cx);
                }
            }
        }));

        let loupe_size_current = settings.capture().loupe_size;
        let loupe_size_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(LoupeSize::ALL.map(LoupeSizeItem).to_vec()),
                LoupeSize::ALL
                    .iter()
                    .position(|s| *s == loupe_size_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&loupe_size_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(sz)) = event
                && this.settings.capture().loupe_size != *sz
            {
                this.settings.capture_mut().loupe_size = *sz;
                this.persist(cx);
            }
        }));

        let scroll_axis_current = settings.scroll().default_axis;
        let scroll_axis_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ScrollAxisSetting::ALL.map(ScrollAxisItem).to_vec()),
                ScrollAxisSetting::ALL
                    .iter()
                    .position(|a| *a == scroll_axis_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&scroll_axis_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(axis)) = event
                && this.settings.scroll().default_axis != *axis
            {
                this.settings.scroll_mut().default_axis = *axis;
                this.persist(cx);
            }
        }));

        let scroll_max_length_slider = cx.new(|_| {
            // `SliderState::new()` starts with `max = 100.0` and re-clamps its
            // stored value inside `.min(...)`; setting `.max(...)` first keeps
            // `min <= max` when `min > 100.0` so `f32::clamp` never panics.
            SliderState::new()
                .max(100_000.)
                .min(2_000.)
                .step(1_000.)
                .default_value(settings.scroll().max_length as f32)
        });
        subs.push(
            cx.subscribe(&scroll_max_length_slider, |this, _, event, cx| {
                if let SliderEvent::Change(SliderValue::Single(value)) = event {
                    let length = value.round().clamp(2_000., 100_000.) as u32;
                    if this.settings.scroll().max_length != length {
                        this.settings.scroll_mut().max_length = length;
                        this.persist(cx);
                    }
                }
            }),
        );

        Self {
            size_label_select,
            loupe_zoom_slider,
            loupe_size_select,
            scroll_axis_select,
            scroll_max_length_slider,
            _subscriptions: subs,
        }
    }
}

impl SettingsWindow {
    fn bool_capture_switch(
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

    pub(super) fn page_capture(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let smart_region = self.bool_capture_switch(
            "smart-region",
            self.settings.smart_region(),
            "settings.general.smart_region.title",
            window,
            cx,
            |s, v| s.set_smart_region(v),
        );
        let window_snap = self.bool_capture_switch(
            "capture-window-snap",
            self.settings.capture().window_snap,
            "settings.capture.window_snap.title",
            window,
            cx,
            |s, v| s.capture_mut().window_snap = v,
        );
        let crosshair = self.bool_capture_switch(
            "capture-crosshair",
            self.settings.capture().crosshair,
            "settings.capture.crosshair.title",
            window,
            cx,
            |s, v| s.capture_mut().crosshair = v,
        );
        let loupe = self.bool_capture_switch(
            "capture-loupe",
            self.settings.capture().loupe,
            "settings.capture.loupe.title",
            window,
            cx,
            |s, v| s.capture_mut().loupe = v,
        );
        let keep_open_copy = self.bool_capture_switch(
            "capture-keep-open-copy",
            self.settings.capture().keep_open_after_copy,
            "settings.capture.keep_open_after_copy.title",
            window,
            cx,
            |s, v| s.capture_mut().keep_open_after_copy = v,
        );
        let keep_open_save = self.bool_capture_switch(
            "capture-keep-open-save",
            self.settings.capture().keep_open_after_save,
            "settings.capture.keep_open_after_save.title",
            window,
            cx,
            |s, v| s.capture_mut().keep_open_after_save = v,
        );

        let body = super::layout::content_column()
            .child(page_header(cx, "settings.preferences.capture", None))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.capture.group.hud").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.general.smart_region.title").to_string(),
                        Some("settings.general.smart_region.body"),
                        None,
                        smart_region,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.window_snap.title").to_string(),
                        Some("settings.capture.window_snap.body"),
                        None,
                        window_snap,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.crosshair.title").to_string(),
                        Some("settings.capture.crosshair.body"),
                        None,
                        crosshair,
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.capture.size_label_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.capture_page.size_label_select).w_full(),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.loupe.title").to_string(),
                        Some("settings.capture.loupe.body"),
                        None,
                        loupe,
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.capture.loupe_zoom.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.capture_page.loupe_zoom_slider,
                            format!("{}×", self.settings.capture().loupe_zoom),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.capture.loupe_size_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.capture_page.loupe_size_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.appearance.dim_opacity").to_string(),
                        Some("settings.appearance.dim_hint"),
                        None,
                        self.opacity_control(cx),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.capture.group.behavior").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.keep_open_after_copy.title").to_string(),
                        Some("settings.capture.keep_open_after_copy.body"),
                        None,
                        keep_open_copy,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.keep_open_after_save.title").to_string(),
                        Some("settings.capture.keep_open_after_save.body"),
                        None,
                        keep_open_save,
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.capture.group.scroll").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.scroll.default_axis.title").to_string(),
                        None,
                        None,
                        Select::new(&self.capture_page.scroll_axis_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.scroll.max_length.title").to_string(),
                        Some("settings.scroll.max_length.body"),
                        None,
                        super::layout::slider_control(
                            &self.capture_page.scroll_max_length_slider,
                            format!("{} px", self.settings.scroll().max_length),
                            cx,
                        ),
                    ),
                ],
            ))
            .child(self.toolbar_editor_card(cx));

        self.attach_toolbar_drag_handlers(page_body("page-toolbar").child(body), cx)
    }
}
