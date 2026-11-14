//! # Shared actions: the vocabulary of every selection operation
//!
//! Actions are the contract between three consumers: the keybindings
//! (bound once in `main`), the toolbar buttons (`ui::toolbar` dispatches
//! the exact same actions) and the handlers (`ui::overlay`). Defining
//! them here — not inside the overlay — keeps that triangle acyclic.
//!
//! The OCR-setup dialog keeps its own two actions local to
//! `ui::ocr_setup`; they are dialog-internal.

use std::collections::BTreeMap;
use std::path::Path;

use gpui_kit::*;

gpui_kit::actions!([
    QuitOverlay,
    EscapeOverlay,
    CopySelection,
    SaveSelection,
    PinSelection,
    OcrSelection,
    SelectScreen,
    ScrollSelection,
    OpenSettings
]);

/// Capture & exit shortcuts: `(settings id, factory keystroke)`, in display order.
pub const CAPTURE_KEYS: &[(&str, &str)] = &[
    ("copy", "ctrl-c"),
    ("save", "ctrl-s"),
    ("pin", "ctrl-p"),
    ("ocr", "ctrl-o"),
    // the fifth exit: fold the selection into a long screenshot as the
    // user scrolls it (plain keys are all annotation tools, hence ctrl-l)
    ("scroll", "ctrl-l"),
    ("select_screen", "ctrl-a"),
    // the conventional accelerator; the toolbar's gear button
    // dispatches the same action
    ("settings", "ctrl-,"),
];

/// Annotation tool & canvas-editing shortcuts: `(settings id, factory keystroke)`.
pub const ANNOTATION_KEYS: &[(&str, &str)] = &[
    ("tool_select", "v"),
    ("tool_rectangle", "r"),
    ("tool_ellipse", "e"),
    ("tool_line", "l"),
    ("tool_polyline", "p"),
    ("tool_arrow", "a"),
    ("tool_number", "n"),
    ("tool_pencil", "b"),
    ("tool_highlighter", "h"),
    ("tool_mosaic", "m"),
    ("tool_magnifier", "g"),
    ("tool_spotlight", "shift-h"),
    ("tool_eraser", "d"),
    ("tool_text", "t"),
    ("undo", "ctrl-z"),
    ("redo", "ctrl-y"),
    ("delete", "delete"),
    ("clear", "ctrl-shift-delete"),
];

/// Every configurable overlay command: `(settings id, factory
/// keystroke)`, in display order (`CAPTURE_KEYS` followed by
/// `ANNOTATION_KEYS`). The settings page lists these; settings.toml
/// `[keys]` overrides entries by id. `escape` (quit the overlay) and
/// `enter` (secondary copy) stay fixed conventions and are deliberately
/// not listed — the escape hatch must survive every rebinding, and the
/// recorder refuses both anyway ([`RESERVED_KEYS`]).
pub const CONFIGURABLE_KEYS: &[(&str, &str)] = &[
    ("copy", "ctrl-c"),
    ("save", "ctrl-s"),
    ("pin", "ctrl-p"),
    ("ocr", "ctrl-o"),
    ("scroll", "ctrl-l"),
    ("select_screen", "ctrl-a"),
    ("settings", "ctrl-,"),
    ("tool_select", "v"),
    ("tool_rectangle", "r"),
    ("tool_ellipse", "e"),
    ("tool_line", "l"),
    ("tool_polyline", "p"),
    ("tool_arrow", "a"),
    ("tool_number", "n"),
    ("tool_pencil", "b"),
    ("tool_highlighter", "h"),
    ("tool_mosaic", "m"),
    ("tool_magnifier", "g"),
    ("tool_spotlight", "shift-h"),
    ("tool_eraser", "d"),
    ("tool_text", "t"),
    ("undo", "ctrl-z"),
    ("redo", "ctrl-y"),
    ("delete", "delete"),
    ("clear", "ctrl-shift-delete"),
];

/// Keystrokes the shortcut editor refuses (the overlay's
/// muscle-memory keys): escape must keep quitting the overlay — and is
/// what cancels a recording — and enter stays the secondary copy
/// trigger. Bare modifier presses never reach this check (the recorder
/// waits them out while a chord is being typed).
pub const RESERVED_KEYS: &[&str] = &["escape", "enter"];

// In-canvas annotation actions (shared by the overlay's keybindings and
// the toolbar's tool buttons, like everything else in this module)
gpui_kit::actions!([
    ToggleSelect,
    ToggleRectangle,
    ToggleEllipse,
    ToggleLine,
    ToggleArrow,
    ToggleNumber,
    ToggleText,
    TogglePencil,
    ToggleHighlighter,
    ToggleMosaic,
    ToggleMagnifier,
    ToggleSpotlight,
    ToggleEraser,
    TogglePolyline,
    FinishPolyline,
    UndoAnnotation,
    RedoAnnotation,
    DeleteAnnotation,
    ClearAnnotations
]);

// Scroll-capture session actions (`ShotoriScroll` key context). Defined
// here so `actions` remains a pure leaf with zero `crate::*` imports.
gpui_kit::actions!(
    scroll,
    [
        ScrollFinish,
        ScrollCancel,
        ScrollSave,
        ScrollPin,
        ScrollPause
    ]
);

// Floating-pin window actions (`ShotoriPin` / `ShotoriPinMenu` contexts).
gpui_kit::actions!(
    pin,
    [ClosePin, DismissPinMenu, OpenPinMenu, CopyPin, ResetPinZoom]
);

/// Keyboard translation in desktop logical pixels.
#[derive(Action, Clone, PartialEq, serde::Deserialize)]
#[action(no_json)]
pub(crate) struct Nudge {
    pub(crate) dx: i32,
    pub(crate) dy: i32,
}

/// Keybindings, scoped to the `ShotoriOverlay` key context. Bound once
/// during startup in `main`. `keys` carries the configurable commands
/// resolved from the user's settings (`[keys]` overrides applied —
/// see [`resolve_keys`]); a missing id falls back to its factory
/// keystroke, so tests (and a truncated map) still bind every command.
///
/// Returns the ids whose chord could not be registered (an override
/// that fails gpui's parser, or a gpui context-syntax change) so
/// `main` can warn: a dropped binding means a command with no key at
/// all, which is exactly the failure that must not be silent.
pub fn bind_keys(cx: &mut App, keys: &BTreeMap<String, String>) -> Vec<&'static str> {
    // Fixed conventions, scroll panel and pins — nothing here is
    // user-visible configuration
    cx.bind_keys([
        KeyBinding::new("escape", EscapeOverlay, Some("ShotoriOverlay")),
        KeyBinding::new("enter", CopySelection, Some("ShotoriOverlay")),
        // the scroll preview panel is its own context (the overlays are
        // gone while it lives); shortcuts mirror the selection flow
        KeyBinding::new("enter", ScrollFinish, Some("ShotoriScroll")),
        KeyBinding::new("ctrl-c", ScrollFinish, Some("ShotoriScroll")),
        KeyBinding::new("escape", ScrollCancel, Some("ShotoriScroll")),
        KeyBinding::new("ctrl-s", ScrollSave, Some("ShotoriScroll")),
        KeyBinding::new("ctrl-p", ScrollPin, Some("ShotoriScroll")),
        // Space freezes sampling without ending the session (the panel
        // button dispatches the same action)
        KeyBinding::new("space", ScrollPause, Some("ShotoriScroll")),
        // pins are their own windows with their own context
        KeyBinding::new("shift-f10", OpenPinMenu, Some("ShotoriPin")),
        // Esc in the menu only dismisses it; with no menu it closes the
        // topmost pin (the surface switches its key_context while a
        // menu is open, which is how both bindings coexist)
        KeyBinding::new("escape", DismissPinMenu, Some("ShotoriPinMenu")),
        KeyBinding::new("escape", ClosePin, Some("ShotoriPin")),
        KeyBinding::new("enter", ClosePin, Some("ShotoriPinMenu")),
    ]);
    bind_fixed_annotation_keys(cx);
    // The configurable commands last: the keymap resolves
    // later-registered bindings first — a user override must be able
    // to shadow secondary convenience aliases (backspace, ctrl-shift-z).
    //
    // Resolved into a Vec first: `KeyBinding::load` takes `&App` while
    // `cx.bind_keys` holds the mutable borrow for the duration of the
    // call.
    let mut unbound: Vec<&'static str> = Vec::new();
    let configurable: Vec<KeyBinding> = CONFIGURABLE_KEYS
        .iter()
        .filter_map(|(id, factory)| {
            let keystroke = keys.get(*id).map(String::as_str).unwrap_or(factory);
            match overlay_binding(cx, id, keystroke) {
                Some(binding) => Some(binding),
                None => {
                    unbound.push(*id);
                    None
                }
            }
        })
        .collect();
    cx.bind_keys(configurable);
    unbound
}

/// The action behind a configurable id — the single place ids map to
/// dispatch targets. `None` = the id is not in [`CONFIGURABLE_KEYS`]
/// (registry drift is pinned by `tests::registry_ids_have_bindings`).
fn overlay_action(id: &str) -> Option<Box<dyn Action>> {
    Some(match id {
        "copy" => Box::new(CopySelection),
        "save" => Box::new(SaveSelection),
        "pin" => Box::new(PinSelection),
        "ocr" => Box::new(OcrSelection),
        "scroll" => Box::new(ScrollSelection),
        "select_screen" => Box::new(SelectScreen),
        "settings" => Box::new(OpenSettings),
        "tool_select" => Box::new(ToggleSelect),
        "tool_rectangle" => Box::new(ToggleRectangle),
        "tool_ellipse" => Box::new(ToggleEllipse),
        "tool_line" => Box::new(ToggleLine),
        "tool_polyline" => Box::new(TogglePolyline),
        "tool_arrow" => Box::new(ToggleArrow),
        "tool_number" => Box::new(ToggleNumber),
        "tool_pencil" => Box::new(TogglePencil),
        "tool_highlighter" => Box::new(ToggleHighlighter),
        "tool_mosaic" => Box::new(ToggleMosaic),
        "tool_magnifier" => Box::new(ToggleMagnifier),
        "tool_spotlight" => Box::new(ToggleSpotlight),
        "tool_eraser" => Box::new(ToggleEraser),
        "tool_text" => Box::new(ToggleText),
        "undo" => Box::new(UndoAnnotation),
        "redo" => Box::new(RedoAnnotation),
        "delete" => Box::new(DeleteAnnotation),
        "clear" => Box::new(ClearAnnotations),
        _ => return None,
    })
}

/// Binds one configurable id's chord. The chord is runtime text (a
/// settings.toml override, the recorder, or the factory literal), and
/// gpui-pre 0.3.10 narrowed `KeyBinding::new` to `&'static str` — its
/// `Keystroke::parse` no longer takes an OS, so a literal `secondary-`
/// is a hard error now. `KeyBinding::load` is the dynamic entry point:
/// it resolves `secondary-` from the host OS.
fn overlay_binding(cx: &App, id: &str, keystroke: &str) -> Option<KeyBinding> {
    let action = overlay_action(id)?;
    // The context is a compile-time literal, so a parse failure means
    // gpui changed its context syntax — drop the binding instead of
    // registering it context-free (`tests::overlay_context_parses`
    // fails first, which is the signal to fix it).
    let predicate = KeyBindingContextPredicate::parse("ShotoriOverlay").ok()?;
    // Same reasoning one level down: `resolve_keys` already rejected
    // unparseable chords, so an error here is unreachable for shipped
    // settings — skip rather than panic on a hand-edited file.
    KeyBinding::load(keystroke, action, Some(predicate.into()), false, None, cx).ok()
}

/// Factory defaults as the full id → keystroke map — what a fresh
/// settings.toml resolves to, and what tests bind to stay hermetic.
pub fn default_keys() -> BTreeMap<String, String> {
    CONFIGURABLE_KEYS
        .iter()
        .map(|(id, key)| ((*id).to_owned(), (*key).to_owned()))
        .collect()
}

/// The host OS gpui's chord parser resolves `secondary-` against.
/// `Keystroke::parse_internal` special-cases only `Mac` (every other OS
/// maps `secondary-` to `ctrl-`), so this must agree with the host on
/// that one arm. The bindings themselves resolve through
/// `cx.platform_info().os` (`KeyBinding::load`); this mirror exists for
/// the validation/display paths that have no `App` (`keystroke_valid`,
/// the recorder's pretty-print, the e2e injector).
fn host_os() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Mac
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

/// Parses a chord that is not a compile-time literal (settings.toml,
/// the recorder, the e2e key injector). gpui-pre 0.3.10 narrowed
/// [`Keystroke::parse`] to `&'static str` — literals keep using it,
/// everything runtime goes through `parse_with_os` with [`host_os`].
pub fn parse_keystroke(source: &str) -> std::result::Result<Keystroke, InvalidKeystrokeError> {
    Keystroke::parse_with_os(source, host_os())
}

/// Whether `keystroke` is a chord a keyboard can actually produce.
/// [`Keystroke::parse`] alone is too permissive to gate on — it
/// accepts any tail as the key name ("oops" parses), which would let a
/// hand-edited file silently eat a command — so the key itself must
/// also look real: a single character, an f-key, or one of the common
/// named keys. ([`Keystroke::parse`] stays the authority on the
/// modifier syntax; the recorder never produces garbage chords.)
pub fn keystroke_valid(keystroke: &str) -> bool {
    let Ok(parsed) = parse_keystroke(keystroke) else {
        return false;
    };
    let key = parsed.key.as_str();
    key.chars().count() == 1
        || (key.len() >= 2
            && key.len() <= 3
            && key.starts_with('f')
            && key[1..].bytes().all(|b| b.is_ascii_digit()))
        || matches!(
            key,
            "escape"
                | "enter"
                | "space"
                | "tab"
                | "backspace"
                | "delete"
                | "insert"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "up"
                | "down"
                | "left"
                | "right"
                | "menu"
        )
}

/// Resolve the user's `[keys]` overrides against the factory defaults.
/// Unknown ids, unparseable keystrokes, reserved keys and duplicate
/// bindings all degrade to the default with a warning — the same
/// contract as unknown toolbar ids (a hand-edit, or a file written by
/// a different shotori version, never bricks the shortcuts).
pub fn resolve_keys(
    path: &Path,
    overrides: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut keys = default_keys();
    let mut valid: Vec<(&str, &str, &str)> = Vec::new();
    for (id, keystroke) in overrides {
        let Some(&(static_id, factory)) = CONFIGURABLE_KEYS.iter().find(|(k, _)| *k == id) else {
            errors.push(format!(
                "{}: unknown shortcut {id:?} (skipped; known ids: {})",
                path.display(),
                known_key_ids()
            ));
            continue;
        };
        if !keystroke_valid(keystroke) {
            errors.push(format!(
                "{}: shortcut {id} = {keystroke:?} is not a valid keystroke (using {factory:?})",
                path.display()
            ));
            continue;
        }
        if RESERVED_KEYS.contains(&keystroke.as_str()) {
            errors.push(format!(
                "{}: shortcut {id} = {keystroke:?} is reserved (using {factory:?})",
                path.display()
            ));
            continue;
        }
        valid.push((static_id, keystroke.as_str(), factory));
    }
    // keystroke → id, to keep two commands off one binding (including
    // clashes between a partial `[keys]` table and untouched defaults).
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
    for &(id, factory) in CONFIGURABLE_KEYS {
        if !valid.iter().any(|(vid, _, _)| *vid == id) {
            claimed.insert(factory, id);
        }
    }
    for (id, keystroke, factory) in valid {
        if let Some(&other) = claimed.get(keystroke) {
            errors.push(format!(
                "{}: shortcuts {id} and {other} share {keystroke:?} — {id} keeps {factory:?}",
                path.display()
            ));
            claimed.insert(factory, id);
            continue;
        }
        claimed.insert(keystroke, id);
        keys.insert(id.to_owned(), keystroke.to_owned());
    }
    keys
}

/// Human-facing chord: `"ctrl-,"` → `"Ctrl+,"`, `"alt-escape"` →
/// `"Alt+Esc"`. Parses first so the `-` key (`"ctrl--"`) splits right.
pub fn pretty_keystroke(keystroke: &str) -> String {
    let Ok(parsed) = parse_keystroke(keystroke) else {
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

/// All valid shortcut ids, comma-joined for error messages.
fn known_key_ids() -> String {
    CONFIGURABLE_KEYS
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fixed non-configurable annotation bindings: arrow nudges, sub-context
/// escape/enter, and secondary convenience aliases (`ctrl-shift-z`,
/// `backspace`, `ctrl-shift-backspace`). Bound inside [`bind_keys`]
/// BEFORE `CONFIGURABLE_KEYS` so user overrides shadow them cleanly.
fn bind_fixed_annotation_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", Nudge { dx: -1, dy: 0 }, Some("ShotoriOverlay")),
        KeyBinding::new("right", Nudge { dx: 1, dy: 0 }, Some("ShotoriOverlay")),
        KeyBinding::new("up", Nudge { dx: 0, dy: -1 }, Some("ShotoriOverlay")),
        KeyBinding::new("down", Nudge { dx: 0, dy: 1 }, Some("ShotoriOverlay")),
        KeyBinding::new(
            "shift-left",
            Nudge { dx: -10, dy: 0 },
            Some("ShotoriOverlay"),
        ),
        KeyBinding::new(
            "shift-right",
            Nudge { dx: 10, dy: 0 },
            Some("ShotoriOverlay"),
        ),
        KeyBinding::new("shift-up", Nudge { dx: 0, dy: -10 }, Some("ShotoriOverlay")),
        KeyBinding::new(
            "shift-down",
            Nudge { dx: 0, dy: 10 },
            Some("ShotoriOverlay"),
        ),
        KeyBinding::new("escape", EscapeOverlay, Some("ShotoriTextEditing")),
        KeyBinding::new("enter", FinishPolyline, Some("PolylineDrawing")),
        KeyBinding::new("ctrl-shift-z", RedoAnnotation, Some("ShotoriOverlay")),
        KeyBinding::new("backspace", DeleteAnnotation, Some("ShotoriOverlay")),
        // Ctrl+Shift mirrors the Delete/Backspace pair without touching
        // the plain keys' shape-granular delete (issue #15)
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearAnnotations,
            Some("ShotoriOverlay"),
        ),
    ]);
}

/// Factory annotation keybindings for standalone headless tests that do
/// not call [`bind_keys`]. Production startup uses [`bind_keys`] alone so
/// user overrides in `[keys]` replace factory tool chords rather than
/// leaving the factory chord bound alongside the custom one.
pub fn init_annotation_keybindings(cx: &mut App) {
    bind_fixed_annotation_keys(cx);
    let defaults: Vec<KeyBinding> = ANNOTATION_KEYS
        .iter()
        .filter_map(|(id, factory)| overlay_binding(cx, id, factory))
        .collect();
    cx.bind_keys(defaults);
}

#[cfg(test)]
mod tests {
    // Explicit imports, not `use super::*`: actions.rs's own
    // `use gpui_kit::*` glob carries GPUI's #[test] macro under the
    // test-support feature, and shadowing Rust's #[test] with it
    // recurses the compiler (see the AGENTS.md note).
    use super::{
        ANNOTATION_KEYS, CAPTURE_KEYS, CONFIGURABLE_KEYS, Path, RESERVED_KEYS, default_keys,
        keystroke_valid, overlay_action, overlay_binding, pretty_keystroke, resolve_keys,
    };

    #[test]
    fn registry_ids_have_bindings() {
        // `overlay_action`'s match and CONFIGURABLE_KEYS must cover the
        // same ids — this pins the sync without a runtime panic path.
        for (id, _) in CONFIGURABLE_KEYS {
            assert!(
                overlay_action(id).is_some(),
                "configurable id {id:?} has no action binding"
            );
        }
        let grouped: Vec<_> = CAPTURE_KEYS
            .iter()
            .chain(ANNOTATION_KEYS.iter())
            .copied()
            .collect();
        assert_eq!(
            grouped.as_slice(),
            CONFIGURABLE_KEYS,
            "CAPTURE_KEYS + ANNOTATION_KEYS must match CONFIGURABLE_KEYS"
        );
        assert_eq!(pretty_keystroke("ctrl-,"), "Ctrl+,");
        assert_eq!(pretty_keystroke("shift-h"), "Shift+H");
        assert_eq!(pretty_keystroke("ctrl-shift-delete"), "Ctrl+Shift+Delete");
    }

    #[test]
    fn overlay_context_parses() {
        // `overlay_binding` derives the binding's key context from this
        // literal; a gpui syntax change would otherwise silently unbind
        // every configurable command.
        assert!(gpui_kit::KeyBindingContextPredicate::parse("ShotoriOverlay").is_ok());
    }

    /// The dynamic bind path (`KeyBinding::load`, not the literal
    /// `KeyBinding::new`) must produce bindings the keymap indexes by
    /// action — a silent drop would leave a command with no key at all,
    /// and no other test would notice.
    #[gpui_kit::test]
    fn configurable_chords_bind_through_the_dynamic_path(cx: &mut gpui_kit::TestAppContext) {
        let keys = default_keys();
        cx.update(|cx| {
            let bindings: Vec<_> = CONFIGURABLE_KEYS
                .iter()
                .filter_map(|(id, factory)| {
                    let keystroke = keys.get(*id).map(String::as_str).unwrap_or(factory);
                    overlay_binding(cx, id, keystroke)
                })
                .collect();
            assert_eq!(
                bindings.len(),
                CONFIGURABLE_KEYS.len(),
                "every configurable id must produce a binding"
            );
            let keymap = gpui_kit::Keymap::new(bindings);
            for (id, chord) in CONFIGURABLE_KEYS {
                let action = overlay_action(id).expect("action for a configurable id");
                assert_eq!(
                    keymap.bindings_for_action(action.as_ref()).count(),
                    1,
                    "{id}: {chord} did not reach its action"
                );
            }
        });
    }

    #[test]
    fn resolve_applies_overrides_and_degrades_garbage() {
        let mut overrides = default_keys();
        overrides.insert("copy".into(), "ctrl-b".into());
        overrides.insert("save".into(), "not a key!!".into());
        overrides.insert("legacy".into(), "ctrl-1".into()); // unknown id
        overrides.insert("ocr".into(), "escape".into()); // reserved
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &overrides, &mut errors);
        assert_eq!(keys.get("copy").unwrap(), "ctrl-b");
        assert_eq!(keys.get("save").unwrap(), "ctrl-s", "invalid → default");
        assert_eq!(keys.get("ocr").unwrap(), "ctrl-o", "reserved → default");
        assert_eq!(keys.get("pin").unwrap(), "ctrl-p", "untouched → default");
        assert_eq!(errors.len(), 3, "one warning per degraded entry");
    }

    #[test]
    fn resolve_rejects_two_commands_on_one_keystroke_and_allows_swaps() {
        let mut overrides = default_keys();
        overrides.insert("save".into(), "ctrl-c".into()); // copy keeps ctrl-c
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &overrides, &mut errors);
        assert_eq!(keys.get("save").unwrap(), "ctrl-s");
        assert_eq!(errors.len(), 1);

        // Partial table clashing with an untouched default also warns & degrades
        let mut partial = std::collections::BTreeMap::new();
        partial.insert("save".into(), "ctrl-c".into());
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &partial, &mut errors);
        assert_eq!(keys.get("save").unwrap(), "ctrl-s");
        assert_eq!(errors.len(), 1);

        // Swapping two commands cleanly works without false-positive clash
        let mut swap = std::collections::BTreeMap::new();
        swap.insert("copy".into(), "ctrl-s".into());
        swap.insert("save".into(), "ctrl-c".into());
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &swap, &mut errors);
        assert_eq!(keys.get("copy").unwrap(), "ctrl-s");
        assert_eq!(keys.get("save").unwrap(), "ctrl-c");
        assert!(errors.is_empty(), "unexpected swap errors: {errors:?}");
    }

    #[test]
    fn every_factory_keystroke_parses_and_is_free_of_reserved_clashes() {
        for (id, keystroke) in CONFIGURABLE_KEYS {
            assert!(keystroke_valid(keystroke), "{id}: factory {keystroke:?}");
            assert!(
                !RESERVED_KEYS.contains(keystroke),
                "{id}: factory {keystroke:?} is reserved"
            );
        }
        // and no two factories collide with each other
        let keys = default_keys();
        let unique: std::collections::BTreeSet<_> = keys.values().collect();
        assert_eq!(unique.len(), keys.len());
    }

    #[test]
    fn actions_module_remains_dependency_free_within_crate() {
        let source = include_str!("actions.rs");
        let prod = source.split("#[cfg(test)]").next().unwrap_or(source);
        let forbidden = concat!("crate", "::");
        for (line_no, line) in prod.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains(forbidden),
                "actions.rs:{}: unexpected crate dependency `{line}`",
                line_no + 1
            );
        }
    }
}
