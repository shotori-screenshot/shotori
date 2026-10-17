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
    CopySelection,
    SaveSelection,
    PinSelection,
    OcrSelection,
    SelectScreen,
    ScrollSelection,
    OpenSettings
]);

/// The configurable overlay commands: `(settings id, factory
/// keystroke)`, in display order. The settings page lists these;
/// settings.toml `[keys]` overrides entries by id. `escape` (quit the
/// overlay) and `enter` (secondary copy) stay fixed conventions and
/// are deliberately not listed — the escape hatch must survive every
/// rebinding, and the recorder refuses both anyway ([`RESERVED_KEYS`]).
pub const CONFIGURABLE_KEYS: &[(&str, &str)] = &[
    ("copy", "ctrl-c"),
    ("save", "ctrl-s"),
    ("pin", "ctrl-p"),
    ("ocr", "ctrl-o"),
    // the fifth exit: auto-scroll the selection into a long
    // screenshot (plain keys are all annotation tools, hence ctrl-l)
    ("scroll", "ctrl-l"),
    ("select_screen", "ctrl-a"),
    // the conventional accelerator; the toolbar's gear button
    // dispatches the same action
    ("settings", "ctrl-,"),
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
    CancelText,
    TogglePencil,
    ToggleHighlighter,
    ToggleMosaic,
    ToggleEraser,
    TogglePolyline,
    FinishPolyline,
    UndoAnnotation,
    RedoAnnotation,
    DeleteAnnotation,
    ClearAnnotations
]);

/// Keybindings, scoped to the `ShotoriOverlay` key context. Bound once
/// during startup in `main`. `keys` carries the configurable commands
/// resolved from the user's settings (`[keys]` overrides applied —
/// see [`resolve_keys`]); a missing id falls back to its factory
/// keystroke, so tests (and a truncated map) still bind every command.
pub fn bind_keys(cx: &mut App, keys: &BTreeMap<String, String>) {
    // Fixed conventions, scroll panel and pins — nothing here is
    // user-visible configuration
    cx.bind_keys([
        KeyBinding::new("escape", QuitOverlay, Some("ShotoriOverlay")),
        KeyBinding::new("enter", CopySelection, Some("ShotoriOverlay")),
        // the scroll preview panel is its own context (the overlays are
        // gone while it lives); shortcuts mirror the selection flow
        KeyBinding::new(
            "enter",
            crate::ui::scroll_bar::ScrollFinish,
            Some("ShotoriScroll"),
        ),
        KeyBinding::new(
            "ctrl-c",
            crate::ui::scroll_bar::ScrollFinish,
            Some("ShotoriScroll"),
        ),
        KeyBinding::new(
            "escape",
            crate::ui::scroll_bar::ScrollCancel,
            Some("ShotoriScroll"),
        ),
        KeyBinding::new(
            "ctrl-s",
            crate::ui::scroll_bar::ScrollSave,
            Some("ShotoriScroll"),
        ),
        // Space freezes sampling without ending the session (the panel
        // button dispatches the same action)
        KeyBinding::new(
            "space",
            crate::ui::scroll_bar::ScrollPause,
            Some("ShotoriScroll"),
        ),
        // pins are their own windows with their own context
        KeyBinding::new("shift-f10", crate::ui::pin::OpenPinMenu, Some("ShotoriPin")),
        // Esc in the menu only dismisses it; with no menu it closes the
        // topmost pin (the surface switches its key_context while a
        // menu is open, which is how both bindings coexist)
        KeyBinding::new(
            "escape",
            crate::ui::pin::DismissPinMenu,
            Some("ShotoriPinMenu"),
        ),
        KeyBinding::new("escape", crate::ui::pin::ClosePin, Some("ShotoriPin")),
        KeyBinding::new("enter", crate::ui::pin::ClosePin, Some("ShotoriPinMenu")),
    ]);
    // The configurable commands last: main binds these after
    // init_annotation_keybindings, and the keymap resolves
    // later-registered bindings first — a user override must be able
    // to shadow a plain-key annotation binding.
    cx.bind_keys(CONFIGURABLE_KEYS.iter().filter_map(|(id, factory)| {
        let keystroke = keys.get(*id).map(String::as_str).unwrap_or(factory);
        overlay_binding(id, keystroke)
    }));
}

/// The action behind a configurable id — the single place ids map to
/// dispatch targets. `None` = the id is not in [`CONFIGURABLE_KEYS`]
/// (registry drift is pinned by `tests::registry_sync`).
fn overlay_binding(id: &str, keystroke: &str) -> Option<KeyBinding> {
    let ctx = Some("ShotoriOverlay");
    Some(match id {
        "copy" => KeyBinding::new(keystroke, CopySelection, ctx),
        "save" => KeyBinding::new(keystroke, SaveSelection, ctx),
        "pin" => KeyBinding::new(keystroke, PinSelection, ctx),
        "ocr" => KeyBinding::new(keystroke, OcrSelection, ctx),
        "scroll" => KeyBinding::new(keystroke, ScrollSelection, ctx),
        "select_screen" => KeyBinding::new(keystroke, SelectScreen, ctx),
        "settings" => KeyBinding::new(keystroke, OpenSettings, ctx),
        _ => return None,
    })
}

/// Factory defaults as the full id → keystroke map — what a fresh
/// settings.toml resolves to, and what tests bind to stay hermetic.
pub fn default_keys() -> BTreeMap<String, String> {
    CONFIGURABLE_KEYS
        .iter()
        .map(|(id, key)| ((*id).to_owned(), (*key).to_owned()))
        .collect()
}

/// Whether `keystroke` is a chord a keyboard can actually produce.
/// [`Keystroke::parse`] alone is too permissive to gate on — it
/// accepts any tail as the key name ("oops" parses), which would let a
/// hand-edited file silently eat a command — so the key itself must
/// also look real: a single character, an f-key, or one of the common
/// named keys. ([`Keystroke::parse`] stays the authority on the
/// modifier syntax; the recorder never produces garbage chords.)
pub fn keystroke_valid(keystroke: &str) -> bool {
    let Ok(parsed) = Keystroke::parse(keystroke) else {
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
    // keystroke → id, to keep two commands off one binding (the
    // keymap would silently let one shadow the other)
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, keystroke) in overrides {
        let Some(factory) = keys.get(id) else {
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
        if let Some(other) = claimed.get(keystroke.as_str()) {
            errors.push(format!(
                "{}: shortcuts {id} and {other} share {keystroke:?} — {id} keeps {factory:?}",
                path.display()
            ));
            continue;
        }
        claimed.insert(keystroke, id);
        keys.insert(id.clone(), keystroke.clone());
    }
    keys
}

/// All valid shortcut ids, comma-joined for error messages.
fn known_key_ids() -> String {
    CONFIGURABLE_KEYS
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Annotation keybindings ("v" select, "r" rectangle, "e" ellipse, "l"
/// line, "a" arrow, "m" mosaic, "h" highlighter, "b" pencil, "n"
/// number, "p" polyline, "t" text, "d" eraser; undo/redo; Ctrl+Shift+Del
/// clears every placed annotation; Enter finishes a polyline). Also
/// scoped to `ShotoriOverlay`, plus the PolylineDrawing sub-context.
pub fn init_annotation_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("v", ToggleSelect, Some("ShotoriOverlay")),
        KeyBinding::new("r", ToggleRectangle, Some("ShotoriOverlay")),
        KeyBinding::new("e", ToggleEllipse, Some("ShotoriOverlay")),
        KeyBinding::new("l", ToggleLine, Some("ShotoriOverlay")),
        KeyBinding::new("a", ToggleArrow, Some("ShotoriOverlay")),
        KeyBinding::new("d", ToggleEraser, Some("ShotoriOverlay")),
        KeyBinding::new("m", ToggleMosaic, Some("ShotoriOverlay")),
        KeyBinding::new("h", ToggleHighlighter, Some("ShotoriOverlay")),
        KeyBinding::new("b", TogglePencil, Some("ShotoriOverlay")),
        KeyBinding::new("t", ToggleText, Some("ShotoriOverlay")),
        KeyBinding::new("escape", CancelText, Some("ShotoriTextEditing")),
        KeyBinding::new("n", ToggleNumber, Some("ShotoriOverlay")),
        KeyBinding::new("p", TogglePolyline, Some("ShotoriOverlay")),
        KeyBinding::new("enter", FinishPolyline, Some("PolylineDrawing")),
        KeyBinding::new("ctrl-z", UndoAnnotation, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-y", RedoAnnotation, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-shift-z", RedoAnnotation, Some("ShotoriOverlay")),
        KeyBinding::new("delete", DeleteAnnotation, Some("ShotoriOverlay")),
        KeyBinding::new("backspace", DeleteAnnotation, Some("ShotoriOverlay")),
        // Ctrl+Shift mirrors the Delete/Backspace pair without touching
        // the plain keys' shape-granular delete (issue #15)
        KeyBinding::new(
            "ctrl-shift-delete",
            ClearAnnotations,
            Some("ShotoriOverlay"),
        ),
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearAnnotations,
            Some("ShotoriOverlay"),
        ),
    ]);
}

#[cfg(test)]
mod tests {
    // Explicit imports, not `use super::*`: actions.rs's own
    // `use gpui_kit::*` glob carries GPUI's #[test] macro under the
    // test-support feature, and shadowing Rust's #[test] with it
    // recurses the compiler (see the AGENTS.md note).
    use super::{
        CONFIGURABLE_KEYS, Path, RESERVED_KEYS, default_keys, keystroke_valid, overlay_binding,
        resolve_keys,
    };

    #[test]
    fn registry_ids_have_bindings() {
        // `overlay_binding`'s match and CONFIGURABLE_KEYS must cover the
        // same ids — this pins the sync without a runtime panic path.
        for (id, _) in CONFIGURABLE_KEYS {
            assert!(
                overlay_binding(id, "ctrl-f1").is_some(),
                "configurable id {id:?} has no action binding"
            );
        }
    }

    #[test]
    fn resolve_applies_overrides_and_degrades_garbage() {
        let mut overrides = default_keys();
        overrides.insert("copy".into(), "ctrl-y".into());
        overrides.insert("save".into(), "not a key!!".into());
        overrides.insert("legacy".into(), "ctrl-1".into()); // unknown id
        overrides.insert("ocr".into(), "escape".into()); // reserved
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &overrides, &mut errors);
        assert_eq!(keys.get("copy").unwrap(), "ctrl-y");
        assert_eq!(keys.get("save").unwrap(), "ctrl-s", "invalid → default");
        assert_eq!(keys.get("ocr").unwrap(), "ctrl-o", "reserved → default");
        assert_eq!(keys.get("pin").unwrap(), "ctrl-p", "untouched → default");
        assert_eq!(errors.len(), 3, "one warning per degraded entry");
    }

    #[test]
    fn resolve_rejects_two_commands_on_one_keystroke() {
        let mut overrides = default_keys();
        overrides.insert("save".into(), "ctrl-c".into()); // copy keeps ctrl-c
        let mut errors = Vec::new();
        let keys = resolve_keys(Path::new("s.toml"), &overrides, &mut errors);
        assert_eq!(keys.get("save").unwrap(), "ctrl-s");
        assert_eq!(errors.len(), 1);
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
}
