//! # Shared actions: the vocabulary of every selection operation
//!
//! Actions are the contract between three consumers: the keybindings
//! (bound once in `main`), the toolbar buttons (`ui::toolbar` dispatches
//! the exact same actions) and the handlers (`ui::overlay`). Defining
//! them here — not inside the overlay — keeps that triangle acyclic.
//!
//! The OCR-setup dialog keeps its own two actions local to
//! `ui::ocr_setup`; they are dialog-internal.

use gpui_kit::*;

gpui_kit::actions!([
    QuitOverlay,
    CopySelection,
    SaveSelection,
    PinSelection,
    OcrSelection,
    SelectScreen
]);

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
/// during startup in `main`.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", QuitOverlay, Some("ShotoriOverlay")),
        KeyBinding::new("enter", CopySelection, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-a", SelectScreen, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-c", CopySelection, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-s", SaveSelection, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-p", PinSelection, Some("ShotoriOverlay")),
        KeyBinding::new("ctrl-o", OcrSelection, Some("ShotoriOverlay")),
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
