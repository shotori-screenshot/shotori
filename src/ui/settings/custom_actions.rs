//! Custom toolbar actions manager card inside the Capture & Toolbar settings
//! page: list inline (`settings.toml`) and drop-in (`~/.config/shotori/actions/`)
//! actions, toggle toolbar placement, import custom SVG icons, and create/edit
//! inline actions with live validation.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::base::{Button, IndexPath};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use crate::model::settings::{
    BUILTIN_ICON_IDS, CustomAction, CustomActionInput, CustomActionOrigin, CustomActionOutput,
    CustomActionSource, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS, ToolbarItem,
    actions_dir, icons_dir, is_valid_custom_action_id, slugify_custom_action_id,
};
use crate::ui::{theme, toolbar};

use super::{SettingsWindow, card_row, open_external};

#[derive(Clone)]
struct CustomInputItem(CustomActionInput);

impl SearchableListItem for CustomInputItem {
    type Value = CustomActionInput;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
struct CustomSourceItem(CustomActionSource);

impl SearchableListItem for CustomSourceItem {
    type Value = CustomActionSource;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
struct CustomOutputItem(CustomActionOutput);

impl SearchableListItem for CustomOutputItem {
    type Value = CustomActionOutput;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Default)]
pub(super) struct CustomActionsPageState {
    pub(super) editing: Option<EditingAction>,
    reload_flash: bool,
}

pub(super) struct EditingAction {
    /// `None` when creating a new action, `Some(old_id)` when editing an existing one.
    original_id: Option<String>,
    /// While creating, auto-sync `id_input` from `name_input` until the user edits `id_input`.
    id_touched: bool,
    pub(super) name_input: Entity<InputState>,
    pub(super) id_input: Entity<InputState>,
    pub(super) icon_input: Entity<InputState>,
    pub(super) command_input: Entity<InputState>,
    pub(super) working_dir_input: Entity<InputState>,
    pub(super) shortcut_input: Entity<InputState>,
    input_select: Entity<SelectState<SearchableVec<CustomInputItem>>>,
    source_select: Entity<SelectState<SearchableVec<CustomSourceItem>>>,
    output_select: Entity<SelectState<SearchableVec<CustomOutputItem>>>,
    timeout_slider: Entity<SliderState>,
    input_val: CustomActionInput,
    source_val: CustomActionSource,
    output_val: CustomActionOutput,
    close_overlay: bool,
    timeout_secs: u16,
    add_to_toolbar: bool,
    form_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    /// Resolve the active config directory (derived from `self.path` so tests
    /// using a tempdir stay completely isolated from the host `$HOME`).
    pub(super) fn effective_config_dir(&self) -> Option<PathBuf> {
        self.path
            .as_deref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(crate::model::settings::config_dir)
    }

    pub(super) fn custom_actions_card(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let entity = cx.entity();
        let config_dir = self.effective_config_dir();

        let new_label = crate::i18n::t!("settings.custom_actions.new").to_string();
        let new_btn = Button::new("custom-action-new")
            .accessibility_label(new_label.clone())
            .flex()
            .items_center()
            .gap_1p5()
            .px_3p5()
            .h_8()
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(cx.theme().primary.opacity(0.65))
            .bg(s.selected_card_bg)
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().foreground)
            .hover(move |st| st.bg(pill_hover))
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .on_click({
                let entity = entity.clone();
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.begin_new_custom_action(window, cx));
                }
            })
            .child(Icon::new(IconName::Plus).size_3p5())
            .child(new_label);

        let folder_label = crate::i18n::t!("settings.custom_actions.open_folder").to_string();
        let folder_btn =
            super::layout::action_button(cx, "custom-actions-open-folder", folder_label, {
                let entity = entity.clone();
                move |_, _, cx| {
                    entity.update(cx, |this, _| this.open_actions_folder());
                }
            });

        let reload_label = if self.custom_actions_page.reload_flash {
            crate::i18n::t!("settings.custom_actions.reloaded").to_string()
        } else {
            crate::i18n::t!("settings.custom_actions.rescan").to_string()
        };
        let rescan_btn = super::layout::action_button(cx, "custom-actions-rescan", reload_label, {
            let entity = entity.clone();
            move |_, _, cx| {
                entity.update(cx, |this, cx| this.rescan_drop_in_actions(cx));
            }
        });

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::t!("settings.custom_actions.title").to_string())
                    .child(super::help_tooltip("settings.custom_actions.hint", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1p5()
                    .child(folder_btn)
                    .child(rescan_btn)
                    .child(new_btn),
            );

        let mut list = div().flex().flex_col().gap_2();
        if self.settings.custom_actions().is_empty() && self.custom_actions_page.editing.is_none() {
            list = list.child(
                div()
                    .p_3p5()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(s.hairline)
                    .bg(s.well_bg)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t!("settings.custom_actions.empty").to_string()),
            );
        } else {
            for action in self.settings.custom_actions() {
                list = list.child(self.custom_action_row(action, config_dir.as_deref(), cx));
            }
        }

        let mut card = div()
            .flex()
            .flex_col()
            .gap_4()
            .p(px(18.))
            .w_full()
            .min_w_0()
            .border_1()
            .border_color(s.hairline)
            .rounded(px(16.))
            .bg(s.card_bg)
            .shadow(s.card_shadow)
            .child(header)
            .child(list);

        if let Some(editor) = &self.custom_actions_page.editing {
            card = card.child(self.custom_action_editor_panel(
                editor,
                config_dir.as_deref(),
                window,
                cx,
            ));
        }

        card
    }

    fn custom_action_row(
        &self,
        action: &CustomAction,
        config_dir: Option<&Path>,
        cx: &Context<Self>,
    ) -> Div {
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let entity = cx.entity();
        let on_bar = self
            .settings
            .row()
            .contains(&ToolbarItem::Custom(action.id.clone()));
        let is_drop_in = action.origin.is_drop_in();

        let icon_tile = div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(34.))
            .flex_shrink_0()
            .rounded(px(9.))
            .bg(rgba(theme::c().capsule_surface()))
            .border_1()
            .border_color(rgba(theme::c().capsule_border()))
            .child(toolbar::custom_action_svg(
                action,
                config_dir,
                theme::c().toolbar_text,
            ));

        let origin_badge = div()
            .px_2()
            .py(px(1.))
            .rounded_full()
            .border_1()
            .border_color(s.hairline)
            .bg(s.pill_bg)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(if is_drop_in {
                crate::i18n::t!("settings.custom_actions.origin.drop_in").to_string()
            } else {
                crate::i18n::t!("settings.custom_actions.origin.inline").to_string()
            });

        let shortcut_badge = action.shortcut.as_deref().map(|chord| {
            div()
                .px_2()
                .py(px(1.))
                .rounded(px(6.))
                .border_1()
                .border_color(s.hairline)
                .bg(s.pill_bg)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .child(crate::actions::pretty_keystroke(chord))
        });

        let summary = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .truncate()
                            .child(action.name.clone()),
                    )
                    .child(origin_badge)
                    .children(shortcut_badge),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(format!("{} · {}", action.toolbar_id(), action.command)),
            );

        let bar_toggle_label = if on_bar {
            crate::i18n::t!("settings.custom_actions.on_bar").to_string()
        } else {
            crate::i18n::t!("settings.custom_actions.add_to_bar").to_string()
        };
        let id_for_toggle = action.id.clone();
        let bar_toggle = Button::new(SharedString::from(format!("ca-bar-toggle-{}", action.id)))
            .accessibility_label(bar_toggle_label.clone())
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .h_7()
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(if on_bar {
                cx.theme().primary.opacity(0.65)
            } else {
                s.hairline
            })
            .bg(if on_bar {
                s.selected_card_bg
            } else {
                s.pill_bg
            })
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().foreground)
            .hover(move |st| st.bg(pill_hover))
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .on_click({
                let entity = entity.clone();
                move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.toggle_custom_action_on_bar(&id_for_toggle, cx);
                    });
                }
            })
            .child(bar_toggle_label);

        let mut controls = div()
            .flex()
            .items_center()
            .gap_1p5()
            .flex_shrink_0()
            .child(bar_toggle);

        if let Some(manifest_path) = action.origin.manifest_path() {
            let open_path = manifest_path.display().to_string();
            let open_label = crate::i18n::t!("settings.custom_actions.open_manifest").to_string();
            controls = controls.child(super::layout::action_button(
                cx,
                SharedString::from(format!("ca-open-{}", action.id)),
                open_label,
                move |_, _, _| open_external(open_path.clone()),
            ));
        } else {
            let edit_action = action.clone();
            let edit_label = crate::i18n::t!("settings.custom_actions.edit").to_string();
            controls = controls.child(super::layout::action_button(
                cx,
                SharedString::from(format!("ca-edit-{}", action.id)),
                edit_label,
                {
                    let entity = entity.clone();
                    move |_, window, cx| {
                        entity.update(cx, |this, cx| {
                            this.begin_edit_custom_action(&edit_action, window, cx);
                        });
                    }
                },
            ));

            let del_id = action.id.clone();
            let del_tip = crate::i18n::t!("settings.custom_actions.delete").to_string();
            let danger_fg = cx.theme().danger;
            controls = controls.child(
                Button::new(SharedString::from(format!("ca-del-{}", action.id)))
                    .accessibility_label(del_tip.clone())
                    .size_7()
                    .flex_shrink_0()
                    .rounded_full()
                    .text_color(cx.theme().muted_foreground)
                    .hover(move |st| st.bg(pill_hover).text_color(danger_fg))
                    .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(del_tip.clone())).into())
                    .child(Icon::new(IconName::Trash).size_3p5())
                    .on_click({
                        let entity = entity.clone();
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| this.delete_custom_action(&del_id, cx));
                        }
                    }),
            );
        }

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .p_3()
            .w_full()
            .min_w_0()
            .rounded(px(12.))
            .border_1()
            .border_color(s.hairline)
            .bg(s.well_bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_1()
                    .min_w_0()
                    .child(icon_tile)
                    .child(summary),
            )
            .child(controls)
    }

    fn custom_action_editor_panel(
        &self,
        editor: &EditingAction,
        config_dir: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let entity = cx.entity();

        let draft_icon = editor.icon_input.read(cx).value().to_string();
        let preview_action = CustomAction {
            id: editor.id_input.read(cx).value().to_string(),
            name: editor.name_input.read(cx).value().to_string(),
            icon: draft_icon.clone(),
            command: String::new(),
            working_dir: None,
            input: editor.input_val,
            source: editor.source_val,
            output: editor.output_val,
            close_overlay: editor.close_overlay,
            timeout_secs: editor.timeout_secs,
            shortcut: None,
            origin: CustomActionOrigin::Settings,
        };

        let live_preview = div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .flex_shrink_0()
            .rounded(px(10.))
            .bg(rgba(theme::c().capsule_surface()))
            .border_1()
            .border_color(rgba(theme::c().capsule_border()))
            .child(toolbar::custom_action_svg(
                &preview_action,
                config_dir,
                theme::c().toolbar_text,
            ));

        let mut icon_pills = div().flex().flex_wrap().items_center().gap_1();
        for &builtin_id in BUILTIN_ICON_IDS {
            let selected = draft_icon.trim() == builtin_id;
            let Some(icon_name) = toolbar::custom_builtin_icon(builtin_id) else {
                continue;
            };
            let entity_pick = entity.clone();
            icon_pills = icon_pills.child(
                Button::new(SharedString::from(format!("ca-icon-pick-{builtin_id}")))
                    .accessibility_label(builtin_id.to_owned())
                    .size_7()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if selected {
                        cx.theme().primary
                    } else {
                        s.hairline
                    })
                    .bg(if selected {
                        s.selected_card_bg
                    } else {
                        s.pill_bg
                    })
                    .text_color(cx.theme().foreground)
                    .hover(move |st| st.bg(pill_hover))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(builtin_id.to_owned())).into())
                    .child(
                        svg()
                            .path(icon_name.path())
                            .size(px(15.))
                            .text_color(cx.theme().foreground),
                    )
                    .on_click(move |_, window, cx| {
                        entity_pick.update(cx, |this, cx| {
                            if let Some(ed) = &this.custom_actions_page.editing {
                                ed.icon_input
                                    .update(cx, |inp, cx| inp.set_value(builtin_id, window, cx));
                            }
                            cx.notify();
                        });
                    }),
            );
        }

        let import_svg_btn = super::layout::action_button(
            cx,
            "ca-import-svg",
            crate::i18n::t!("settings.custom_actions.form.import_svg").to_string(),
            {
                let entity = entity.clone();
                move |_, _, cx| {
                    pick_svg_file(entity.clone(), cx);
                }
            },
        );

        let close_overlay_switch = super::layout::settings_switch(
            "ca-close-overlay",
            editor.close_overlay,
            crate::i18n::t!("settings.custom_actions.form.close_overlay").to_string(),
            window,
            cx,
        )
        .on_change({
            let entity = entity.clone();
            move |checked, _, _, cx| {
                entity.update(cx, |this, cx| {
                    if let Some(ed) = &mut this.custom_actions_page.editing {
                        ed.close_overlay = checked;
                    }
                    cx.notify();
                });
            }
        });

        let add_to_bar_row = editor.original_id.is_none().then(|| {
            let sw = super::layout::settings_switch(
                "ca-add-to-bar",
                editor.add_to_toolbar,
                crate::i18n::t!("settings.custom_actions.form.add_to_bar").to_string(),
                window,
                cx,
            )
            .on_change({
                let entity = entity.clone();
                move |checked, _, _, cx| {
                    entity.update(cx, |this, cx| {
                        if let Some(ed) = &mut this.custom_actions_page.editing {
                            ed.add_to_toolbar = checked;
                        }
                        cx.notify();
                    });
                }
            });
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.custom_actions.form.add_to_bar").to_string(),
                None,
                None,
                sw,
            )
        });

        let field =
            |label_key: &'static str, hint_key: Option<&'static str>, control: AnyElement| {
                let mut title = div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(crate::i18n::t!(label_key).to_string());
                if let Some(hk) = hint_key {
                    title = title.child(super::help_tooltip(hk, cx));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .w_full()
                    .min_w_0()
                    .child(title)
                    .child(control)
            };

        let save_label = crate::i18n::t!("settings.custom_actions.form.save").to_string();
        let save_btn = Button::new("ca-form-save")
            .accessibility_label(save_label.clone())
            .flex()
            .items_center()
            .gap_1p5()
            .px_4()
            .h_8()
            .rounded_full()
            .border_1()
            .border_color(cx.theme().primary)
            .bg(cx.theme().primary)
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().primary_foreground)
            .hover(|st| st.opacity(0.9))
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .on_click({
                let entity = entity.clone();
                move |_, _, cx| {
                    entity.update(cx, |this, cx| this.save_editing_custom_action(cx));
                }
            })
            .child(save_label);

        let cancel_btn = super::layout::action_button(
            cx,
            "ca-form-cancel",
            crate::i18n::t!("settings.custom_actions.form.cancel").to_string(),
            {
                let entity = entity.clone();
                move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.custom_actions_page.editing = None;
                        cx.notify();
                    });
                }
            },
        );

        let form_title = if editor.original_id.is_some() {
            crate::i18n::t!("settings.custom_actions.form.edit_title").to_string()
        } else {
            crate::i18n::t!("settings.custom_actions.form.new_title").to_string()
        };

        let error_banner = editor.form_error.as_ref().map(|msg| {
            div()
                .px_3()
                .py_2()
                .rounded(px(9.))
                .border_1()
                .border_color(cx.theme().danger.opacity(0.6))
                .bg(cx.theme().danger.opacity(0.1))
                .text_xs()
                .text_color(cx.theme().danger)
                .child(msg.clone())
        });

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(px(12.))
            .border_1()
            .border_color(cx.theme().primary.opacity(0.45))
            .bg(s.well_bg)
            .child(
                div().flex().items_center().justify_between().gap_3().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .child(live_preview)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(form_title),
                        ),
                ),
            )
            .children(error_banner)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(div().flex_1().min_w(px(180.)).child(field(
                        "settings.custom_actions.form.name",
                        None,
                        Input::new(&editor.name_input).w_full().into_any_element(),
                    )))
                    .child(div().flex_1().min_w(px(180.)).child(field(
                        "settings.custom_actions.form.id",
                        Some("settings.custom_actions.form.id_hint"),
                        Input::new(&editor.id_input).w_full().into_any_element(),
                    ))),
            )
            .child(field(
                "settings.custom_actions.form.icon",
                Some("settings.custom_actions.form.icon_hint"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&editor.icon_input).w_full()),
                            )
                            .child(import_svg_btn),
                    )
                    .child(icon_pills)
                    .into_any_element(),
            ))
            .child(field(
                "settings.custom_actions.form.command",
                Some("settings.custom_actions.form.command_hint"),
                Input::new(&editor.command_input)
                    .w_full()
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(card_row(
                        cx,
                        crate::i18n::t!("settings.custom_actions.form.input").to_string(),
                        Some("settings.custom_actions.form.input_hint"),
                        None,
                        Select::new(&editor.input_select).w_full(),
                    ))
                    .child(card_row(
                        cx,
                        crate::i18n::t!("settings.custom_actions.form.source").to_string(),
                        None,
                        None,
                        Select::new(&editor.source_select).w_full(),
                    ))
                    .child(card_row(
                        cx,
                        crate::i18n::t!("settings.custom_actions.form.output").to_string(),
                        Some("settings.custom_actions.form.output_hint"),
                        None,
                        Select::new(&editor.output_select).w_full(),
                    ))
                    .child(super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.custom_actions.form.close_overlay").to_string(),
                        Some("settings.custom_actions.form.close_overlay_hint"),
                        None,
                        close_overlay_switch,
                    ))
                    .child(card_row(
                        cx,
                        crate::i18n::t!("settings.custom_actions.form.timeout").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &editor.timeout_slider,
                            format!("{} s", editor.timeout_secs),
                            cx,
                        ),
                    ))
                    .children(add_to_bar_row),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div().flex_1().min_w(px(180.)).child(field(
                            "settings.custom_actions.form.shortcut",
                            Some("settings.custom_actions.form.shortcut_hint"),
                            Input::new(&editor.shortcut_input)
                                .w_full()
                                .into_any_element(),
                        )),
                    )
                    .child(
                        div().flex_1().min_w(px(180.)).child(field(
                            "settings.custom_actions.form.working_dir",
                            None,
                            Input::new(&editor.working_dir_input)
                                .w_full()
                                .into_any_element(),
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(cancel_btn)
                    .child(save_btn),
            )
    }

    pub(super) fn begin_new_custom_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = CustomAction {
            id: String::new(),
            name: String::new(),
            icon: "cloud-upload".into(),
            command: String::new(),
            working_dir: None,
            input: CustomActionInput::File,
            source: CustomActionSource::Annotated,
            output: CustomActionOutput::CopyText,
            close_overlay: true,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            shortcut: None,
            origin: CustomActionOrigin::Settings,
        };
        self.custom_actions_page.editing = Some(build_editor(None, &seed, window, cx));
        cx.notify();
    }

    fn begin_edit_custom_action(
        &mut self,
        action: &CustomAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.custom_actions_page.editing =
            Some(build_editor(Some(action.id.clone()), action, window, cx));
        cx.notify();
    }

    pub(super) fn save_editing_custom_action(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &self.custom_actions_page.editing else {
            return;
        };
        let original_id = ed.original_id.clone();
        let name_raw = ed.name_input.read(cx).value().trim().to_owned();
        let id_entered = ed.id_input.read(cx).value().trim().to_ascii_lowercase();
        let id_raw = if id_entered.is_empty() && original_id.is_none() && !name_raw.is_empty() {
            slugify_custom_action_id(&name_raw, self.settings.custom_actions())
        } else {
            id_entered
        };
        let icon_raw = ed.icon_input.read(cx).value().trim().to_owned();
        let command = ed.command_input.read(cx).value().trim().to_owned();
        let working_dir_raw = ed.working_dir_input.read(cx).value().trim().to_owned();
        let shortcut_raw = ed
            .shortcut_input
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        let input = ed.input_val;
        let source = ed.source_val;
        let output = ed.output_val;
        let close_overlay = ed.close_overlay;
        let timeout_secs = ed.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        let add_to_toolbar = ed.add_to_toolbar;

        if !is_valid_custom_action_id(&id_raw) {
            if let Some(ed) = &mut self.custom_actions_page.editing {
                ed.form_error =
                    Some(crate::i18n::t!("settings.custom_actions.error.invalid_id").to_string());
            }
            cx.notify();
            return;
        }
        if command.is_empty() {
            if let Some(ed) = &mut self.custom_actions_page.editing {
                ed.form_error = Some(
                    crate::i18n::t!("settings.custom_actions.error.empty_command").to_string(),
                );
            }
            cx.notify();
            return;
        }
        let id_clash = self.settings.custom_actions().iter().any(|existing| {
            existing.id == id_raw && original_id.as_deref() != Some(existing.id.as_str())
        });
        if id_clash {
            if let Some(ed) = &mut self.custom_actions_page.editing {
                ed.form_error = Some(
                    crate::i18n::t!("settings.custom_actions.error.duplicate_id", id = id_raw)
                        .to_string(),
                );
            }
            cx.notify();
            return;
        }

        let shortcut = if shortcut_raw.is_empty() {
            None
        } else {
            if !crate::actions::keystroke_valid(&shortcut_raw) {
                if let Some(ed) = &mut self.custom_actions_page.editing {
                    ed.form_error = Some(
                        crate::i18n::t!("settings.custom_actions.error.invalid_shortcut")
                            .to_string(),
                    );
                }
                cx.notify();
                return;
            }
            if crate::actions::RESERVED_KEYS.contains(&shortcut_raw.as_str()) {
                if let Some(ed) = &mut self.custom_actions_page.editing {
                    ed.form_error =
                        Some(crate::i18n::t!("settings.keys_page.reserved").to_string());
                }
                cx.notify();
                return;
            }
            let conflict_builtin = self
                .settings
                .keys()
                .iter()
                .find(|(_, k)| k.as_str() == shortcut_raw)
                .map(|(k, _)| k.clone());
            let conflict_custom = self
                .settings
                .custom_actions()
                .iter()
                .find(|a| {
                    original_id.as_deref() != Some(a.id.as_str())
                        && a.id != id_raw
                        && a.shortcut.as_deref() == Some(shortcut_raw.as_str())
                })
                .map(|a| a.name.clone());
            if let Some(other) = conflict_builtin.or(conflict_custom) {
                if let Some(ed) = &mut self.custom_actions_page.editing {
                    ed.form_error = Some(
                        crate::i18n::t!("settings.keys_page.conflict", action = other).to_string(),
                    );
                }
                cx.notify();
                return;
            }
            Some(shortcut_raw)
        };

        let action = CustomAction {
            id: id_raw.clone(),
            name: if name_raw.is_empty() {
                id_raw.clone()
            } else {
                name_raw
            },
            icon: if icon_raw.is_empty() {
                "sparkles".into()
            } else {
                icon_raw
            },
            command,
            working_dir: (!working_dir_raw.is_empty()).then(|| PathBuf::from(working_dir_raw)),
            input,
            source,
            output,
            close_overlay,
            timeout_secs,
            shortcut,
            origin: CustomActionOrigin::Settings,
        };

        let is_new = original_id.is_none();
        self.settings
            .upsert_custom_action(original_id.as_deref(), action);
        if is_new && add_to_toolbar {
            let item = ToolbarItem::Custom(id_raw);
            if !self.settings.row().contains(&item) {
                let mut row = self.settings.row().to_vec();
                row.push(item);
                self.settings.set_row(row);
            }
        }
        self.custom_actions_page.editing = None;
        self.persist(cx);
    }

    fn delete_custom_action(&mut self, id: &str, cx: &mut Context<Self>) {
        if self
            .custom_actions_page
            .editing
            .as_ref()
            .and_then(|ed| ed.original_id.as_deref())
            == Some(id)
        {
            self.custom_actions_page.editing = None;
        }
        self.settings.remove_custom_action(id);
        self.persist(cx);
    }

    fn toggle_custom_action_on_bar(&mut self, id: &str, cx: &mut Context<Self>) {
        let item = ToolbarItem::Custom(id.to_owned());
        let mut row = self.settings.row().to_vec();
        if let Some(pos) = row.iter().position(|it| it == &item) {
            row.remove(pos);
        } else {
            row.push(item);
        }
        self.commit(row, cx);
    }

    fn open_actions_folder(&self) {
        let Some(cfg) = self.effective_config_dir() else {
            crate::notify::send(
                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                crate::i18n::t!("settings.no_config_dir").as_ref(),
            );
            return;
        };
        let dir = actions_dir(&cfg);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            crate::notify::send(
                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                crate::i18n::t!("settings.general.open_failed.body", error = format!("{e}"))
                    .as_ref(),
            );
            return;
        }
        open_external(dir.display().to_string());
    }

    fn rescan_drop_in_actions(&mut self, cx: &mut Context<Self>) {
        let Some(cfg) = self.effective_config_dir() else {
            return;
        };
        let warnings = self.settings.reload_drop_in_actions(&cfg);
        self.error = warnings.first().cloned();
        self.custom_actions_page.reload_flash = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1600))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.custom_actions_page.reload_flash = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn imported_svg_picked(
        &mut self,
        picked: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(src) = picked else {
            return;
        };
        let bytes = match std::fs::read(&src) {
            Ok(b) if toolbar::looks_like_svg(&b) => b,
            Ok(_) => {
                if let Some(ed) = &mut self.custom_actions_page.editing {
                    ed.form_error = Some(
                        crate::i18n::t!("settings.custom_actions.error.invalid_svg").to_string(),
                    );
                }
                cx.notify();
                return;
            }
            Err(e) => {
                if let Some(ed) = &mut self.custom_actions_page.editing {
                    ed.form_error = Some(e.to_string());
                }
                cx.notify();
                return;
            }
        };

        let icon_value = if let Some(cfg) = self.effective_config_dir() {
            let dest_dir = icons_dir(&cfg);
            let stem = self
                .custom_actions_page
                .editing
                .as_ref()
                .map(|ed| ed.id_input.read(cx).value().trim().to_ascii_lowercase())
                .filter(|s| is_valid_custom_action_id(s))
                .or_else(|| {
                    src.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_ascii_lowercase())
                        .filter(|s| is_valid_custom_action_id(s))
                })
                .unwrap_or_else(|| "custom-icon".to_owned());
            let file_name = format!("{stem}.svg");
            let dest_path = dest_dir.join(&file_name);
            if std::fs::create_dir_all(&dest_dir)
                .and_then(|_| std::fs::write(&dest_path, &bytes))
                .is_ok()
            {
                file_name
            } else {
                src.display().to_string()
            }
        } else {
            src.display().to_string()
        };

        if let Some(ed) = &mut self.custom_actions_page.editing {
            ed.form_error = None;
            ed.icon_input
                .update(cx, |inp, cx| inp.set_value(icon_value, window, cx));
        }
        cx.notify();
    }
}

fn build_editor(
    original_id: Option<String>,
    seed: &CustomAction,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> EditingAction {
    let mut subs = Vec::new();
    let is_new = original_id.is_none();

    let name_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(seed.name.clone())
            .placeholder(
                crate::i18n::t!("settings.custom_actions.form.name_placeholder").to_string(),
            )
    });
    let id_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(seed.id.clone())
            .placeholder("upload-r2")
    });
    let icon_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(seed.icon.clone())
            .placeholder("cloud-upload / my-icon.svg")
    });
    let command_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(seed.command.clone())
            .placeholder(
                crate::i18n::t!("settings.custom_actions.form.command_placeholder").to_string(),
            )
    });
    let working_dir_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(
                seed.working_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            )
            .placeholder(
                crate::i18n::t!("settings.custom_actions.form.optional_placeholder").to_string(),
            )
    });
    let shortcut_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(seed.shortcut.clone().unwrap_or_default())
            .placeholder("ctrl-u")
    });

    if is_new {
        subs.push(
            cx.subscribe_in(&name_input, window, |this, inp, event, window, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(ed) = &this.custom_actions_page.editing
                    && !ed.id_touched
                {
                    let name = inp.read(cx).value().to_string();
                    let slug = slugify_custom_action_id(&name, this.settings.custom_actions());
                    let id_input = ed.id_input.clone();
                    // `InputState::set_value` does not emit `InputEvent::Change`,
                    // so programmatic slug updates never mark `id_touched`.
                    id_input.update(cx, |s, cx| s.set_value(slug, window, cx));
                }
            }),
        );
        subs.push(cx.subscribe(&id_input, |this, _, event, _| {
            if matches!(event, InputEvent::Change)
                && let Some(ed) = &mut this.custom_actions_page.editing
            {
                ed.id_touched = true;
            }
        }));
    }

    subs.push(cx.subscribe(&icon_input, |_, _, event, cx| {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    }));

    let input_select = cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(CustomActionInput::ALL.map(CustomInputItem).to_vec()),
            CustomActionInput::ALL
                .iter()
                .position(|v| *v == seed.input)
                .map(IndexPath::new),
            window,
            cx,
        )
    });
    subs.push(cx.subscribe(&input_select, |this, _, event, cx| {
        if let SelectEvent::Confirm(Some(v)) = event
            && let Some(ed) = &mut this.custom_actions_page.editing
        {
            ed.input_val = *v;
            cx.notify();
        }
    }));

    let source_select = cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(CustomActionSource::ALL.map(CustomSourceItem).to_vec()),
            CustomActionSource::ALL
                .iter()
                .position(|v| *v == seed.source)
                .map(IndexPath::new),
            window,
            cx,
        )
    });
    subs.push(cx.subscribe(&source_select, |this, _, event, cx| {
        if let SelectEvent::Confirm(Some(v)) = event
            && let Some(ed) = &mut this.custom_actions_page.editing
        {
            ed.source_val = *v;
            cx.notify();
        }
    }));

    let output_select = cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(CustomActionOutput::ALL.map(CustomOutputItem).to_vec()),
            CustomActionOutput::ALL
                .iter()
                .position(|v| *v == seed.output)
                .map(IndexPath::new),
            window,
            cx,
        )
    });
    subs.push(cx.subscribe(&output_select, |this, _, event, cx| {
        if let SelectEvent::Confirm(Some(v)) = event
            && let Some(ed) = &mut this.custom_actions_page.editing
        {
            ed.output_val = *v;
            cx.notify();
        }
    }));

    let timeout_slider = cx.new(|_| {
        SliderState::new()
            .min(f32::from(MIN_TIMEOUT_SECS))
            .max(f32::from(MAX_TIMEOUT_SECS))
            .step(1.)
            .default_value(f32::from(seed.timeout_secs))
    });
    subs.push(cx.subscribe(&timeout_slider, |this, _, event, cx| {
        if let SliderEvent::Change(SliderValue::Single(v)) = event
            && let Some(ed) = &mut this.custom_actions_page.editing
        {
            ed.timeout_secs = v
                .round()
                .clamp(f32::from(MIN_TIMEOUT_SECS), f32::from(MAX_TIMEOUT_SECS))
                as u16;
            cx.notify();
        }
    }));

    EditingAction {
        original_id,
        id_touched: !is_new,
        name_input,
        id_input,
        icon_input,
        command_input,
        working_dir_input,
        shortcut_input,
        input_select,
        source_select,
        output_select,
        timeout_slider,
        input_val: seed.input,
        source_val: seed.source,
        output_val: seed.output,
        close_overlay: seed.close_overlay,
        timeout_secs: seed.timeout_secs,
        add_to_toolbar: is_new,
        form_error: None,
        _subscriptions: subs,
    }
}

fn pick_svg_file(entity: Entity<SettingsWindow>, cx: &mut App) {
    let mut dialog = rfd::FileDialog::new()
        .set_title(crate::i18n::t!("settings.custom_actions.form.import_svg_title").as_ref())
        .add_filter(
            crate::i18n::t!("settings.custom_actions.form.svg_filter").as_ref(),
            &["svg"],
        );
    if let Some(dir) = crate::model::settings::config_dir()
        .map(|d| icons_dir(&d))
        .filter(|d| d.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    {
        dialog = dialog.set_directory(dir);
    }
    let (tx, rx) = async_channel::bounded::<Option<PathBuf>>(1);
    std::thread::spawn(move || {
        let picked = dialog.pick_file();
        drop(tx.send_blocking(picked));
    });
    cx.spawn(async move |cx| {
        if let Ok(picked) = rx.recv().await {
            cx.update(|cx| {
                // Find the settings window to update the InputState with `&mut Window`.
                for handle in cx.windows() {
                    if let Ok(()) = handle.update(cx, |_, window, cx| {
                        entity.update(cx, |this, cx| {
                            this.imported_svg_picked(picked.clone(), window, cx);
                        });
                    }) {
                        break;
                    }
                }
            });
        }
    })
    .detach();
}
