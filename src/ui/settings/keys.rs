//! The Keys page: the shortcut recorder. One row per configurable
//! command, a key-cap click starts recording, the next chord rebinds
//! (or is refused inline). The chord canonicalization and display
//! helpers live with the page that uses them.

use gpui_kit::*;

use crate::ui::theme;

use super::{CONTENT_MAX, SettingsWindow, alpha, card_group, card_row, page_body, page_header};

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

/// Human-facing chord: `"ctrl-,"` → `"Ctrl+,"`, `"alt-escape"` →
/// `"Alt+Esc"`. Parses first so the `-` key ("ctrl--") splits right.
fn pretty_keystroke(keystroke: &str) -> String {
    let Ok(parsed) = gpui_kit::Keystroke::parse(keystroke) else {
        return keystroke.to_owned();
    };
    let m = &parsed.modifiers;
    let mut parts: Vec<String> = Vec::new();
    if m.control {
        parts.push("Ctrl".into());
    }
    if m.alt {
        parts.push("Alt".into());
    }
    if m.platform {
        parts.push("Super".into());
    }
    if m.shift {
        parts.push("Shift".into());
    }
    if m.function {
        parts.push("Fn".into());
    }
    parts.push(pretty_key(&parsed.key));
    parts.join("+")
}

/// One key's display name (mod handled by [`pretty_keystroke`]).
fn pretty_key(key: &str) -> String {
    match key {
        "escape" => "Esc".into(),
        "space" => "Space".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        single if single.chars().count() == 1 => single.to_uppercase(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

impl SettingsWindow {
    /// Shortcut editor: one row per configurable command. Clicking a
    /// key cap starts a recording; the next (non-modifier) keystroke
    /// rebinds the command, escape cancels, reserved keys and
    /// conflicts are refused inline. Writes through like every control
    /// here — the next screenshot binds from the file (each capture is
    /// a fresh process; there is no live rebinding to do).
    pub(super) fn page_keys(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut rows = Vec::with_capacity(crate::actions::CONFIGURABLE_KEYS.len());
        for (id, factory) in crate::actions::CONFIGURABLE_KEYS {
            rows.push(self.key_row(id, factory, cx));
        }
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .max_w(px(CONTENT_MAX))
            .child(page_header(
                crate::i18n::t!("settings.keys_page.title").as_ref(),
                crate::i18n::t!("settings.keys_page.blurb").as_ref(),
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.keys_page.group").as_ref()),
                rows,
            ));
        page_body("page-keys").child(body)
    }

    /// One shortcut row: the command's label (plus its factory
    /// keystroke once customized) on the left, the key cap on the
    /// right. Only the RECORDING row tracks focus — one shared handle,
    /// attached to whichever row is listening; without focus the
    /// keydown listener is dead weight on the others.
    fn key_row(&self, id: &str, factory: &str, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
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
            pretty_keystroke(&keystroke)
        };
        let description = if recording || !customized {
            None
        } else {
            Some(
                crate::i18n::t!(
                    "settings.keys_page.default",
                    keys = pretty_keystroke(factory)
                )
                .to_string(),
            )
        };
        let mut cap = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(110.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .border_1()
            .border_color(rgba(if recording {
                t.accent
            } else {
                t.toolbar_border
            }))
            .bg(rgba(if recording {
                alpha(t.toolbar_hover, 0x88)
            } else {
                0x00000000
            }))
            .text_color(rgba(if recording { t.accent } else { t.btn_text }))
            .hover(|s| s.border_color(rgba(t.accent)));
        if recording {
            // Only the RECORDING cap listens: one shared focus handle,
            // attached to whichever row is armed — without focus the
            // keydown listener is dead weight on the others.
            cap = cap
                .track_focus(&self.record_focus)
                .on_key_down(cx.listener(Self::capture_key));
        }
        let cap = cap
            .id(SharedString::from(format!("key-cap-{id}")))
            .on_click({
                let id = id.to_owned();
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.toggle_recording(&id, window, cx));
                }
            })
            .child(cap_label);
        card_row(key_label(id), description, cap)
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
