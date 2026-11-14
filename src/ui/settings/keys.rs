//! The Keys page: the shortcut recorder. One row per configurable
//! command, a key-cap click starts recording, the next chord rebinds
//! (or is refused inline). The chord canonicalization and display
//! helpers live with the page that uses them.

use gpui_kit::base::Button;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::{SettingsWindow, card_group, card_row, page_body, page_header};

/// The localized display name of a configurable shortcut id. The ids
/// live in `actions::CONFIGURABLE_KEYS`; the test below pins that
/// every one of them lands on a real label (the fallback returns the
/// raw id only so a future registry entry can't render as nothing).
fn key_label(id: &str) -> String {
    match id {
        "copy" => crate::i18n::t!("settings.keys_page.copy").to_string(),
        "save" => crate::i18n::t!("settings.keys_page.save").to_string(),
        "pin" => crate::i18n::t!("settings.keys_page.pin").to_string(),
        "ocr" => crate::i18n::t!("settings.keys_page.ocr").to_string(),
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

/// Canonical stored form of a captured chord (`"ctrl-alt-p"`). Built
/// from the event's modifiers — NOT `Keystroke`'s Display, which
/// prints platform glyphs (❖) on Linux that `Keystroke::parse` then
/// can't read back; these strings must round-trip through parse
/// (resolve_keys re-validates them on the next load).
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

impl SettingsWindow {
    /// Shortcut editor: one row per configurable command, grouped into
    /// capture exits and annotation tools/editing. Clicking a key cap
    /// starts a recording; the next (non-modifier) keystroke rebinds
    /// the command, escape cancels, reserved keys and conflicts are
    /// refused inline. Writes through like every control here — the
    /// next screenshot binds from the file.
    pub(super) fn page_keys(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut capture_rows = Vec::with_capacity(crate::actions::CAPTURE_KEYS.len());
        for (id, factory) in crate::actions::CAPTURE_KEYS {
            capture_rows.push(self.key_row(id, factory, cx));
        }
        let mut annot_rows = Vec::with_capacity(crate::actions::ANNOTATION_KEYS.len());
        for (id, factory) in crate::actions::ANNOTATION_KEYS {
            annot_rows.push(self.key_row(id, factory, cx));
        }
        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.keys_page.title",
                        Some("settings.keys_page.blurb"),
                    ))
                    .child(super::shell::reset_button(
                        "settings.keys_page.reset",
                        cx,
                        {
                            let entity = cx.entity();
                            move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.settings.reset_keys();
                                    this.persist(cx);
                                });
                            }
                        },
                    )),
            )
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.keys_page.group.capture").as_ref()),
                capture_rows,
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.keys_page.group.annotation").as_ref()),
                annot_rows,
            ));
        page_body("page-keys").child(body)
    }

    /// One shortcut row: the command's label (plus its factory
    /// keystroke once customized) on the left, the key cap on the
    /// right. Only the RECORDING row tracks focus — one shared handle,
    /// attached to whichever row is listening; without focus the
    /// keydown listener is dead weight on the others.
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
        let cap_id = SharedString::from(format!("key-cap-{id}"));
        let mut cap = Button::new(cap_id)
            .accessibility_label(key_label(id))
            .flex()
            .items_center()
            .justify_center()
            .min_w(rems(7.))
            .max_w_full()
            .min_h_8()
            .h_auto()
            .py_1()
            .when(recording, |cap| cap.min_w(rems(11.)))
            .px_3()
            .rounded(cx.theme().radius)
            .text_sm()
            .border_1()
            .border_color(if recording {
                cx.theme().ring
            } else {
                cx.theme().border
            })
            .bg(if recording {
                cx.theme().accent
            } else {
                cx.theme().background
            })
            .text_color(cx.theme().foreground)
            .focus_visible(|s| s.border_color(cx.theme().ring))
            .hover(|s| s.bg(cx.theme().secondary_hover));
        if recording {
            // Only the RECORDING cap listens: one shared focus handle,
            // attached to whichever row is armed — without focus the
            // keydown listener is dead weight on the others.
            cap = cap.track_focus(&self.record_focus);
        }
        let cap = cap
            .on_click({
                let id = id.to_owned();
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.toggle_recording(&id, window, cx));
                }
            })
            .child(cap_label);
        let cap = div()
            .capture_key_down(cx.listener(Self::capture_key))
            .child(cap);
        card_row(cx, key_label(id), None, description, cap)
    }

    /// Key-cap click: start recording (focusing the cap so the chord
    /// lands on its keydown listener), or — clicking the listening cap
    /// again — cancel.
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

    /// Leave recording mode (commit, cancel — and refocus nothing: the
    /// cap keeps no focus so a stray chord can't re-trigger it).
    fn stop_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = None;
        self.record_error = None;
        window.blur(cx);
        cx.notify();
    }

    /// The recording cap's keydown: canonicalize the chord, then
    /// commit it, refuse it (reserved / taken — inline message, still
    /// recording) or wait it out (bare modifiers, so Ctrl then C can
    /// be pressed as one chord). Escape cancels. stop_propagation
    /// keeps the window's own Esc binding from closing the window
    /// mid-recording.
    fn capture_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.recording.clone() else {
            return;
        };
        // Capture before Button handles Enter/Space as activation, otherwise
        // binding one of those keys would merely toggle the recorder off.
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let bare = keystroke.modifiers == gpui_kit::Modifiers::default();
        if keystroke.key == "escape" && bare {
            // Cancel WITHOUT clearing `recording` here: the CloseSettings
            // action still fires after this listener (see the guard on
            // the root), and it consumes `recording` to tell "this Esc
            // was the recorder's, not the window's". Re-render happens
            // via that path.
            window.blur(cx);
            cx.stop_propagation();
            return;
        }
        // Plain modifier keys (ctrl alone, alt alone …): hold the
        // recording until a real key joins the chord
        if matches!(
            keystroke.key.as_str(),
            "ctrl" | "control" | "alt" | "shift" | "super" | "cmd" | "fn" | "platform" | "meta"
        ) {
            return;
        }
        let chord = canonical_chord(keystroke);
        let taken = self
            .settings
            .keys()
            .iter()
            .find(|(other, key)| other.as_str() != id && key.as_str() == chord)
            .map(|(other, _)| other.clone());
        if let Some(other) = taken {
            self.record_error = Some(
                crate::i18n::t!("settings.keys_page.conflict", action = key_label(&other))
                    .to_string(),
            );
        } else if crate::actions::RESERVED_KEYS.contains(&chord.as_str()) {
            self.record_error = Some(crate::i18n::t!("settings.keys_page.reserved").to_string());
        } else {
            self.settings.set_key(&id, chord);
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
