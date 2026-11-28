//! The Keys page: the shortcut recorder, per-row reset, live search filtering,
//! and the built-in fixed canvas gesture reference.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::{SettingsWindow, card_group, page_body, page_header};

/// The localized display name of a configurable shortcut id.
fn key_label(id: &str) -> String {
    match id {
        "copy" => crate::i18n::t!("settings.keys_page.copy").to_string(),
        "save" => crate::i18n::t!("settings.keys_page.save").to_string(),
        "pin" => crate::i18n::t!("settings.keys_page.pin").to_string(),
        "ocr" => crate::i18n::t!("settings.keys_page.ocr").to_string(),
        "qr" => crate::i18n::t!("settings.keys_page.qr").to_string(),
        "scroll" => crate::i18n::t!("settings.keys_page.scroll").to_string(),
        "select_screen" => crate::i18n::t!("settings.keys_page.select_screen").to_string(),
        "settings" => crate::i18n::t!("settings.keys_page.settings").to_string(),
        "tool_select" => crate::i18n::t!("settings.keys_page.tool_select").to_string(),
        "tool_rectangle" => crate::i18n::t!("settings.keys_page.tool_rectangle").to_string(),
        "tool_ellipse" => crate::i18n::t!("settings.keys_page.tool_ellipse").to_string(),
        "tool_line" => crate::i18n::t!("settings.keys_page.tool_line").to_string(),
        "tool_polyline" => crate::i18n::t!("settings.keys_page.tool_polyline").to_string(),
        "tool_arrow" => crate::i18n::t!("settings.keys_page.tool_arrow").to_string(),
        "tool_number" => crate::i18n::t!("settings.keys_page.tool_number").to_string(),
        "tool_pencil" => crate::i18n::t!("settings.keys_page.tool_pencil").to_string(),
        "tool_highlighter" => crate::i18n::t!("settings.keys_page.tool_highlighter").to_string(),
        "tool_mosaic" => crate::i18n::t!("settings.keys_page.tool_mosaic").to_string(),
        "tool_magnifier" => crate::i18n::t!("settings.keys_page.tool_magnifier").to_string(),
        "tool_spotlight" => crate::i18n::t!("settings.keys_page.tool_spotlight").to_string(),
        "tool_eraser" => crate::i18n::t!("settings.keys_page.tool_eraser").to_string(),
        "tool_text" => crate::i18n::t!("settings.keys_page.tool_text").to_string(),
        "undo" => crate::i18n::t!("settings.keys_page.undo").to_string(),
        "redo" => crate::i18n::t!("settings.keys_page.redo").to_string(),
        "delete" => crate::i18n::t!("settings.keys_page.delete").to_string(),
        "clear" => crate::i18n::t!("settings.keys_page.clear").to_string(),
        _ => id.to_owned(),
    }
}

/// Canonical stored form of a captured chord (`"ctrl-alt-p"`).
fn canonical_chord(keystroke: &gpui_kit::Keystroke) -> String {
    let mut chord = String::new();
    let m = &keystroke.modifiers;
    if m.control {
        chord.push_str("ctrl-");
    }
    if m.alt {
        chord.push_str("alt-");
    }
    if m.platform {
        chord.push_str("super-");
    }
    if m.shift {
        chord.push_str("shift-");
    }
    if m.function {
        chord.push_str("fn-");
    }
    chord.push_str(&keystroke.key);
    chord
}

fn row_matches_query(id: &str, label: &str, keystroke: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    // Generic section-level queries ("快捷键", "shortcuts", "keys") keep all rows visible.
    let page_title = crate::i18n::t!("settings.keys_page.title").to_lowercase();
    if page_title.contains(query) || "shortcuts keys keybindings 快捷键 按键".contains(query) {
        return true;
    }
    let pretty = crate::actions::pretty_keystroke(keystroke).to_lowercase();
    label.to_lowercase().contains(query)
        || id.to_lowercase().contains(query)
        || keystroke.to_lowercase().contains(query)
        || pretty.contains(query)
}

impl SettingsWindow {
    pub(super) fn page_keys(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let query = self.search.read(cx).value().trim().to_lowercase();

        let mut capture_rows = Vec::with_capacity(crate::actions::CAPTURE_KEYS.len());
        for &(id, factory) in crate::actions::CAPTURE_KEYS {
            let keystroke = self
                .settings
                .keys()
                .get(id)
                .map(String::as_str)
                .unwrap_or(factory);
            let label = key_label(id);
            if row_matches_query(id, &label, keystroke, &query) {
                capture_rows.push(self.key_row(id, factory, cx));
            }
        }
        let mut annot_rows = Vec::with_capacity(crate::actions::ANNOTATION_KEYS.len());
        for &(id, factory) in crate::actions::ANNOTATION_KEYS {
            let keystroke = self
                .settings
                .keys()
                .get(id)
                .map(String::as_str)
                .unwrap_or(factory);
            let label = key_label(id);
            if row_matches_query(id, &label, keystroke, &query) {
                annot_rows.push(self.key_row(id, factory, cx));
            }
        }

        let mut custom_rows = Vec::new();
        for action in self.settings.custom_actions() {
            let keystroke = action.shortcut.as_deref().unwrap_or("");
            let custom_key_id = action.toolbar_id();
            if row_matches_query(&custom_key_id, &action.name, keystroke, &query) {
                custom_rows.push(self.custom_key_row(action, cx));
            }
        }

        let any_customized = crate::actions::CONFIGURABLE_KEYS
            .iter()
            .any(|&(id, factory)| {
                self.settings
                    .keys()
                    .get(id)
                    .is_some_and(|k| k.as_str() != factory)
            });

        let mut body = super::layout::content_column().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(page_header(
                    cx,
                    "settings.keys_page.title",
                    Some("settings.keys_page.blurb"),
                ))
                .child(
                    super::shell::reset_button("settings.keys_page.reset", cx, {
                        let entity = cx.entity();
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.settings.reset_keys();
                                this.persist(cx);
                            });
                        }
                    })
                    .disabled(!any_customized),
                ),
        );

        if capture_rows.is_empty() && annot_rows.is_empty() && custom_rows.is_empty() {
            body = body.child(card_group(
                cx,
                None,
                vec![
                    div()
                        .py_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t!("settings.keys_page.no_matches").to_string()),
                ],
            ));
        } else {
            if !capture_rows.is_empty() {
                body = body.child(card_group(
                    cx,
                    Some(crate::i18n::t!("settings.keys_page.group.capture").as_ref()),
                    capture_rows,
                ));
            }
            if !annot_rows.is_empty() {
                body = body.child(card_group(
                    cx,
                    Some(crate::i18n::t!("settings.keys_page.group.annotation").as_ref()),
                    annot_rows,
                ));
            }
            if !custom_rows.is_empty() {
                body = body.child(card_group(
                    cx,
                    Some(crate::i18n::t!("settings.keys_page.group.custom").as_ref()),
                    custom_rows,
                ));
            }
        }

        // Built-in fixed canvas shortcuts & gestures reference card.
        let s = super::palette::surfaces(cx);
        let static_cap = |text: String| {
            div()
                .px_3()
                .py_1()
                .rounded(px(9.))
                .border_1()
                .border_color(s.hairline)
                .bg(s.well_bg)
                .shadow(vec![
                    BoxShadow::new(px(0.), px(1.), s.hairline).blur_radius(px(0.)),
                ])
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let builtin_rows = vec![
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.keys_page.builtin.enter").to_string(),
                None,
                None,
                static_cap("Enter".into()),
            ),
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.keys_page.builtin.escape").to_string(),
                None,
                None,
                static_cap("Esc".into()),
            ),
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.keys_page.builtin.arrows").to_string(),
                None,
                None,
                static_cap(crate::i18n::t!("settings.keys_page.builtin.arrows_key").to_string()),
            ),
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.keys_page.builtin.wheel").to_string(),
                None,
                None,
                static_cap(crate::i18n::t!("settings.keys_page.builtin.wheel_key").to_string()),
            ),
            super::layout::compact_row(
                cx,
                crate::i18n::t!("settings.keys_page.builtin.alt_click").to_string(),
                None,
                None,
                static_cap(crate::i18n::t!("settings.keys_page.builtin.alt_click_key").to_string()),
            ),
        ];
        body = body.child(card_group(
            cx,
            Some(crate::i18n::t!("settings.keys_page.builtin.group").as_ref()),
            builtin_rows,
        ));

        page_body("page-keys").child(body)
    }

    fn key_row(&self, id: &str, factory: &str, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let recording = self.recording.as_deref() == Some(id);
        let keystroke = self
            .settings
            .keys()
            .get(id)
            .map(String::as_str)
            .unwrap_or(factory)
            .to_owned();
        let customized = keystroke != factory;
        let cap_label = if recording {
            match &self.record_error {
                Some(error) => error.clone(),
                None => crate::i18n::t!("settings.keys_page.record").to_string(),
            }
        } else {
            crate::actions::pretty_keystroke(&keystroke)
        };
        let description = if recording || !customized {
            None
        } else {
            Some(
                crate::i18n::t!(
                    "settings.keys_page.default",
                    keys = crate::actions::pretty_keystroke(factory)
                )
                .to_string(),
            )
        };
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let cap_id = SharedString::from(format!("key-cap-{id}"));
        let mut cap = Button::new(cap_id)
            .accessibility_label(key_label(id))
            .flex()
            .items_center()
            .justify_center()
            .min_w(rems(6.5))
            .max_w_full()
            .min_h_8()
            .h_auto()
            .py_1()
            .when(recording, |cap| cap.min_w(rems(11.)))
            .px_3()
            .rounded(px(9.))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(if recording {
                cx.theme().primary
            } else if customized {
                cx.theme().primary.opacity(0.65)
            } else {
                s.hairline
            })
            .bg(if recording {
                s.selected_card_bg
            } else {
                s.well_bg
            })
            .shadow(if recording {
                s.selected_shadow
            } else {
                vec![BoxShadow::new(px(0.), px(1.), s.hairline).blur_radius(px(0.))]
            })
            .text_color(cx.theme().foreground)
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .hover(move |st| st.bg(pill_hover));
        if recording {
            cap = cap.track_focus(&self.record_focus);
        }
        let cap = cap
            .on_click({
                let entity = entity.clone();
                let id = id.to_owned();
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.toggle_recording(&id, window, cx));
                }
            })
            .child(cap_label);

        let reset_single = (customized && !recording).then(|| {
            let entity = entity.clone();
            let id_owned = id.to_owned();
            let fg = cx.theme().foreground;
            let reset_tip = crate::i18n::t!("settings.keys_page.reset_single").to_string();
            Button::new(SharedString::from(format!("key-reset-{id}")))
                .accessibility_label(format!("{reset_tip}: {}", key_label(id)))
                .size_7()
                .flex_shrink_0()
                .rounded_full()
                .text_color(cx.theme().muted_foreground)
                .hover(move |st| st.bg(pill_hover).text_color(fg))
                .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                .tooltip(move |_, cx| cx.new(|_| Tooltip::new(reset_tip.clone())).into())
                .child(Icon::new(IconName::Undo2).size_3p5())
                .on_click(move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.settings.reset_key(&id_owned);
                        this.persist(cx);
                    });
                })
        });

        let control = div()
            .flex()
            .items_center()
            .gap_1p5()
            .children(reset_single)
            .child(
                div()
                    .capture_key_down(cx.listener(Self::capture_key))
                    .child(cap),
            );
        super::layout::compact_row(cx, key_label(id), None, description, control)
    }

    fn custom_key_row(
        &self,
        action: &crate::model::settings::CustomAction,
        cx: &mut Context<Self>,
    ) -> Div {
        let entity = cx.entity();
        let rec_id = action.toolbar_id();
        let recording = self.recording.as_deref() == Some(rec_id.as_str());
        let is_drop_in = action.origin.is_drop_in();
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;

        let cap_label = if recording {
            match &self.record_error {
                Some(error) => error.clone(),
                None => crate::i18n::t!("settings.keys_page.record").to_string(),
            }
        } else {
            match action.shortcut.as_deref() {
                Some(chord) if !chord.is_empty() => crate::actions::pretty_keystroke(chord),
                _ => crate::i18n::t!("settings.keys_page.unset").to_string(),
            }
        };

        let description = is_drop_in
            .then(|| crate::i18n::t!("settings.custom_actions.origin.drop_in").to_string());

        let has_key = action.shortcut.as_deref().is_some_and(|k| !k.is_empty());
        let mut cap = Button::new(SharedString::from(format!("key-cap-{rec_id}")))
            .accessibility_label(action.name.clone())
            .disabled(is_drop_in)
            .flex()
            .items_center()
            .justify_center()
            .min_w(rems(6.5))
            .max_w_full()
            .min_h_8()
            .h_auto()
            .py_1()
            .when(recording, |cap| cap.min_w(rems(11.)))
            .px_3()
            .rounded(px(9.))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(if recording {
                cx.theme().primary
            } else if has_key {
                cx.theme().primary.opacity(0.65)
            } else {
                s.hairline
            })
            .bg(if recording {
                s.selected_card_bg
            } else {
                s.well_bg
            })
            .shadow(if recording {
                s.selected_shadow
            } else {
                vec![BoxShadow::new(px(0.), px(1.), s.hairline).blur_radius(px(0.))]
            })
            .text_color(if has_key || recording {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            })
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .hover(move |st| st.bg(pill_hover));
        if recording {
            cap = cap.track_focus(&self.record_focus);
        }
        let cap = cap
            .on_click({
                let entity = entity.clone();
                let rec_id = rec_id.clone();
                move |_, window, cx| {
                    if !is_drop_in {
                        entity.update(cx, |this, cx| this.toggle_recording(&rec_id, window, cx));
                    }
                }
            })
            .child(cap_label);

        let clear_btn = (!is_drop_in && has_key && !recording).then(|| {
            let entity = entity.clone();
            let action_id = action.id.clone();
            let fg = cx.theme().foreground;
            let clear_tip = crate::i18n::t!("settings.keys_page.clear_custom").to_string();
            Button::new(SharedString::from(format!(
                "key-clear-custom-{}",
                action.id
            )))
            .accessibility_label(format!("{clear_tip}: {}", action.name))
            .size_7()
            .flex_shrink_0()
            .rounded_full()
            .text_color(cx.theme().muted_foreground)
            .hover(move |st| st.bg(pill_hover).text_color(fg))
            .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(clear_tip.clone())).into())
            .child(Icon::new(IconName::X).size_3p5())
            .on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| {
                    this.settings.set_custom_action_shortcut(&action_id, None);
                    this.persist(cx);
                });
            })
        });

        let control = div()
            .flex()
            .items_center()
            .gap_1p5()
            .children(clear_btn)
            .child(
                div()
                    .capture_key_down(cx.listener(Self::capture_key))
                    .child(cap),
            );
        super::layout::compact_row(cx, action.name.clone(), None, description, control)
    }

    fn toggle_recording(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.as_deref() == Some(id) {
            self.stop_recording(window, cx);
            return;
        }
        self.recording = Some(id.to_owned());
        self.record_error = None;
        window.focus(&self.record_focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = None;
        self.record_error = None;
        window.blur(cx);
        cx.notify();
    }

    fn capture_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.recording.clone() else {
            return;
        };
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let bare = keystroke.modifiers == gpui_kit::Modifiers::default();
        if keystroke.key == "escape" && bare {
            window.blur(cx);
            cx.stop_propagation();
            return;
        }
        if matches!(
            keystroke.key.as_str(),
            "ctrl" | "control" | "alt" | "shift" | "super" | "cmd" | "fn" | "platform" | "meta"
        ) {
            return;
        }
        let chord = canonical_chord(keystroke);
        let custom_target = id.strip_prefix(crate::model::settings::CUSTOM_TOOLBAR_PREFIX);
        let taken_builtin = self
            .settings
            .keys()
            .iter()
            .find(|(other, key)| other.as_str() != id && key.as_str() == chord)
            .map(|(other, _)| key_label(other));
        let taken_custom = self
            .settings
            .custom_actions()
            .iter()
            .find(|a| {
                custom_target != Some(a.id.as_str())
                    && a.shortcut.as_deref() == Some(chord.as_str())
            })
            .map(|a| a.name.clone());
        if let Some(other) = taken_builtin.or(taken_custom) {
            self.record_error =
                Some(crate::i18n::t!("settings.keys_page.conflict", action = other).to_string());
        } else if crate::actions::RESERVED_KEYS.contains(&chord.as_str()) {
            self.record_error = Some(crate::i18n::t!("settings.keys_page.reserved").to_string());
        } else {
            if let Some(custom_id) = custom_target {
                self.settings
                    .set_custom_action_shortcut(custom_id, Some(chord));
            } else {
                self.settings.set_key(&id, chord);
            }
            self.persist(cx);
            self.stop_recording(window, cx);
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsWindow, key_label};
    use crate::model::settings::Settings;
    use crate::ui::settings::Section;
    use gpui_kit::TestAppContext;

    #[test]
    fn every_configurable_key_has_a_localized_label() {
        for &(id, _) in crate::actions::CONFIGURABLE_KEYS {
            assert_ne!(
                key_label(id),
                id,
                "configurable shortcut '{id}' is missing a localized label in key_label()"
            );
        }
    }

    #[gpui_kit::test]
    fn recorder_refuses_enter_and_records_space_without_button_activation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let config = tempfile::tempdir().expect("test config directory");
        let path = config.path().join("settings.toml");
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = SettingsWindow::new(window, cx);
            view.section = Section::Keys;
            view.settings = Settings::default();
            view.path = Some(path.clone());
            view.error = None;
            view
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.toggle_recording("copy", window, cx));
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            view.update(cx, |view, _| {
                assert_eq!(view.recording.as_deref(), Some("copy"));
                assert!(view.record_error.is_some());
                assert_eq!(
                    view.settings.keys().get("copy").map(String::as_str),
                    Some("ctrl-c")
                );
            });
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("space");
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                assert!(view.recording.is_none());
                assert_eq!(
                    view.settings.keys().get("copy").map(String::as_str),
                    Some("space")
                );
                assert!(view.error.is_none());
            })
        });
        let (saved, warnings) = Settings::load(Some(&path));
        assert!(warnings.is_empty());
        assert_eq!(saved.keys().get("copy").map(String::as_str), Some("space"));
    }
}
