//! Capture & Toolbar settings page: drag-and-drop toolbar layout, selection
//! HUD, corner loupe, mouse gestures, post-exit behavior, and long-screenshot defaults.

use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::model::settings::{
    LoupeSize, ScrollAxisSetting, ScrollTransparencyMode, Settings, SizeLabelMode,
};

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

#[derive(Clone)]
pub(super) struct ScrollTransparencyModeItem(ScrollTransparencyMode);

impl SearchableListItem for ScrollTransparencyModeItem {
    type Value = ScrollTransparencyMode;

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
    scroll_transparency_select: Entity<SelectState<SearchableVec<ScrollTransparencyModeItem>>>,
    scroll_enter_cmd_input: Entity<InputState>,
    scroll_exit_cmd_input: Entity<InputState>,
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

        let transparency_current = settings.scroll().transparency_mode;
        let scroll_transparency_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    ScrollTransparencyMode::ALL
                        .map(ScrollTransparencyModeItem)
                        .to_vec(),
                ),
                ScrollTransparencyMode::ALL
                    .iter()
                    .position(|m| *m == transparency_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(
            cx.subscribe(&scroll_transparency_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(mode)) = event
                    && this.settings.scroll().transparency_mode != *mode
                {
                    this.settings.scroll_mut().transparency_mode = *mode;
                    this.persist(cx);
                }
            }),
        );

        let initial_enter = settings.scroll().enter_command.clone().unwrap_or_default();
        let scroll_enter_cmd_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial_enter)
                .placeholder("swaymsg blur disable")
        });
        subs.push(cx.subscribe(&scroll_enter_cmd_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let raw = this
                    .capture_page
                    .scroll_enter_cmd_input
                    .read(cx)
                    .value()
                    .to_string();
                let next = {
                    let trimmed = raw.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_owned())
                };
                if this.settings.scroll().enter_command != next {
                    this.settings.scroll_mut().set_enter_command(next);
                    this.persist(cx);
                }
            }
        }));

        let initial_exit = settings.scroll().exit_command.clone().unwrap_or_default();
        let scroll_exit_cmd_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial_exit)
                .placeholder("swaymsg blur enable")
        });
        subs.push(cx.subscribe(&scroll_exit_cmd_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let raw = this
                    .capture_page
                    .scroll_exit_cmd_input
                    .read(cx)
                    .value()
                    .to_string();
                let next = {
                    let trimmed = raw.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_owned())
                };
                if this.settings.scroll().exit_command != next {
                    this.settings.scroll_mut().set_exit_command(next);
                    this.persist(cx);
                }
            }
        }));

        Self {
            size_label_select,
            loupe_zoom_slider,
            loupe_size_select,
            scroll_axis_select,
            scroll_max_length_slider,
            scroll_transparency_select,
            scroll_enter_cmd_input,
            scroll_exit_cmd_input,
            _subscriptions: subs,
        }
    }

    pub(super) fn resync(&self, settings: &Settings, window: &mut Window, cx: &mut App) {
        let cap = settings.capture();
        let size_ix = SizeLabelMode::ALL
            .iter()
            .position(|m| *m == cap.size_label)
            .map(IndexPath::new);
        self.size_label_select
            .update(cx, |s, cx| s.set_selected_index(size_ix, window, cx));
        self.loupe_zoom_slider.update(cx, |s, cx| {
            s.set_value(f32::from(cap.loupe_zoom), window, cx)
        });
        let loupe_ix = LoupeSize::ALL
            .iter()
            .position(|sz| *sz == cap.loupe_size)
            .map(IndexPath::new);
        self.loupe_size_select
            .update(cx, |s, cx| s.set_selected_index(loupe_ix, window, cx));
        let axis_ix = ScrollAxisSetting::ALL
            .iter()
            .position(|a| *a == settings.scroll().default_axis)
            .map(IndexPath::new);
        self.scroll_axis_select
            .update(cx, |s, cx| s.set_selected_index(axis_ix, window, cx));
        self.scroll_max_length_slider.update(cx, |s, cx| {
            s.set_value(settings.scroll().max_length as f32, window, cx)
        });
        let transparency_ix = ScrollTransparencyMode::ALL
            .iter()
            .position(|m| *m == settings.scroll().transparency_mode)
            .map(IndexPath::new);
        self.scroll_transparency_select.update(cx, |s, cx| {
            s.set_selected_index(transparency_ix, window, cx)
        });
        let enter_val = settings.scroll().enter_command.clone().unwrap_or_default();
        self.scroll_enter_cmd_input
            .update(cx, |s, cx| s.set_value(enter_val, window, cx));
        let exit_val = settings.scroll().exit_command.clone().unwrap_or_default();
        self.scroll_exit_cmd_input
            .update(cx, |s, cx| s.set_value(exit_val, window, cx));
    }
}

impl SettingsWindow {
    fn reset_capture_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.reset_capture_defaults();
        self.capture_page.resync(&self.settings, window, cx);
        self.persist(cx);
        if self.appearance.file.dim_opacity.take().is_some() {
            self.appearance_resync(window, cx);
            self.appearance_apply(cx);
        }
    }

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
        let cap = self.settings.capture();
        let loupe_on = cap.loupe;

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
            cap.window_snap,
            "settings.capture.window_snap.title",
            window,
            cx,
            |s, v| s.capture_mut().window_snap = v,
        );
        let crosshair = self.bool_capture_switch(
            "capture-crosshair",
            cap.crosshair,
            "settings.capture.crosshair.title",
            window,
            cx,
            |s, v| s.capture_mut().crosshair = v,
        );
        let loupe = self.bool_capture_switch(
            "capture-loupe",
            cap.loupe,
            "settings.capture.loupe.title",
            window,
            cx,
            |s, v| s.capture_mut().loupe = v,
        );
        let double_click_copy = self.bool_capture_switch(
            "capture-double-click-copy",
            cap.double_click_copy,
            "settings.capture.double_click_copy.title",
            window,
            cx,
            |s, v| s.capture_mut().double_click_copy = v,
        );
        let right_click_cancel = self.bool_capture_switch(
            "capture-right-click-cancel",
            cap.right_click_cancel,
            "settings.capture.right_click_cancel.title",
            window,
            cx,
            |s, v| s.capture_mut().right_click_cancel = v,
        );
        let keep_open_copy = self.bool_capture_switch(
            "capture-keep-open-copy",
            cap.keep_open_after_copy,
            "settings.capture.keep_open_after_copy.title",
            window,
            cx,
            |s, v| s.capture_mut().keep_open_after_copy = v,
        );
        let keep_open_save = self.bool_capture_switch(
            "capture-keep-open-save",
            cap.keep_open_after_save,
            "settings.capture.keep_open_after_save.title",
            window,
            cx,
            |s, v| s.capture_mut().keep_open_after_save = v,
        );

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.preferences.capture",
                        Some("settings.capture.blurb"),
                    ))
                    .child(super::shell::reset_button("settings.capture.reset", cx, {
                        let entity = cx.entity();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.reset_capture_page(window, cx);
                            });
                        }
                    })),
            )
            // Keep the interactive toolbar layout editor above the fold so the
            // primary "Capture & toolbar" customization is immediately reachable.
            .child(self.toolbar_editor_card(cx))
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
                    card_row(
                        cx,
                        crate::i18n::t!("settings.appearance.dim_opacity").to_string(),
                        Some("settings.appearance.dim_hint"),
                        None,
                        self.opacity_control(cx),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.loupe.title").to_string(),
                        Some("settings.capture.loupe.body"),
                        None,
                        loupe,
                    ),
                    super::layout::dim_when(
                        !loupe_on,
                        card_row(
                            cx,
                            crate::i18n::t!("settings.capture.loupe_zoom.title").to_string(),
                            None,
                            None,
                            super::layout::slider_control_disabled(
                                &self.capture_page.loupe_zoom_slider,
                                format!("{}×", cap.loupe_zoom),
                                !loupe_on,
                                cx,
                            ),
                        ),
                    ),
                    super::layout::dim_when(
                        !loupe_on,
                        card_row(
                            cx,
                            crate::i18n::t!("settings.capture.loupe_size_row.title").to_string(),
                            None,
                            None,
                            Select::new(&self.capture_page.loupe_size_select)
                                .disabled(!loupe_on)
                                .w_full(),
                        ),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.capture.group.behavior").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.double_click_copy.title").to_string(),
                        Some("settings.capture.double_click_copy.body"),
                        None,
                        double_click_copy,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.capture.right_click_cancel.title").to_string(),
                        Some("settings.capture.right_click_cancel.body"),
                        None,
                        right_click_cancel,
                    ),
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
                self.scroll_settings_rows(cx),
            ));

        self.attach_toolbar_drag_handlers(page_body("page-toolbar").child(body), cx)
    }

    fn scroll_settings_rows(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let scroll = self.settings.scroll();
        let transparency_desc =
            (scroll.transparency_mode == ScrollTransparencyMode::Auto).then(|| {
                match crate::platform::scroll_capture::detected_builtin_compositor() {
                    Some(compositor) => crate::i18n::t!(
                        "settings.scroll.transparency.detected",
                        compositor = compositor
                    )
                    .to_string(),
                    None => crate::i18n::t!("settings.scroll.transparency.unsupported").to_string(),
                }
            });
        let mut rows: Vec<AnyElement> = vec![
            card_row(
                cx,
                crate::i18n::t!("settings.scroll.default_axis.title").to_string(),
                None,
                None,
                Select::new(&self.capture_page.scroll_axis_select).w_full(),
            )
            .into_any_element(),
            card_row(
                cx,
                crate::i18n::t!("settings.scroll.max_length.title").to_string(),
                Some("settings.scroll.max_length.body"),
                None,
                super::layout::slider_control(
                    &self.capture_page.scroll_max_length_slider,
                    format!("{} px", scroll.max_length),
                    cx,
                ),
            )
            .into_any_element(),
            card_row(
                cx,
                crate::i18n::t!("settings.scroll.transparency.title").to_string(),
                Some("settings.scroll.transparency.body"),
                transparency_desc,
                Select::new(&self.capture_page.scroll_transparency_select).w_full(),
            )
            .into_any_element(),
        ];

        if scroll.transparency_mode == ScrollTransparencyMode::Custom {
            let cmd_row =
                |title_key: &'static str, hint_key: &'static str, input: &Entity<InputState>| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py_3()
                        .w_full()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(crate::i18n::t!(title_key).to_string())
                                .child(super::help_tooltip(hint_key, cx)),
                        )
                        .child(Input::new(input).w_full())
                        .into_any_element()
                };
            rows.push(cmd_row(
                "settings.scroll.enter_command.title",
                "settings.scroll.enter_command.body",
                &self.capture_page.scroll_enter_cmd_input,
            ));
            rows.push(cmd_row(
                "settings.scroll.exit_command.title",
                "settings.scroll.exit_command.body",
                &self.capture_page.scroll_exit_cmd_input,
            ));
        }

        rows
    }
}
