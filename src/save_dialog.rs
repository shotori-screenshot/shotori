//! # Save flow: system "Save as" dialog (xdg-desktop-portal)
//!
//! Ctrl+S hands the selection to the desktop's native file picker instead of
//! writing to a fixed path. Order matters: the overlay is a layer-shell
//! surface on the Overlay layer with exclusive keyboard — a portal dialog is
//! a regular window and would be covered by it. So the overlay quits first,
//! then the main thread (after the app loop exits — we deliberately do not
//! rely on gpui keeping the process alive with zero windows) opens the
//! dialog, writes the PNG and fires the notification.
//!
//! Handoff overlay → main thread: a static `Mutex<Option<PendingSave>>`. The
//! overlay action crops, stashes RGBA pixels and calls `cx.quit()`;
//! `complete_pending()` (invoked from `main` after the run loop returns)
//! takes them.
//!
//! Backends: rfd's `xdg-portal` feature talks to
//! `org.freedesktop.portal.FileChooser` via ashpd (pure Rust, no GTK link
//! time); if the portal is unreachable it falls back to zenity. Cancel (or
//! every backend failing) yields `None` → the save is abandoned silently.

use std::path::PathBuf;

use gpui_kit::{AnyWindowHandle, App, Window};

use crate::model::export;

/// A cropped selection waiting for the user to pick a save location.
pub struct PendingSave {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// Overlay → main-thread handoff. Shared by every overlay window (one per
/// screen) and drained exactly once, from `main`.
static PENDING: std::sync::OnceLock<std::sync::Mutex<Option<PendingSave>>> =
    std::sync::OnceLock::new();

/// Stash a cropped selection; the caller then quits the overlay.
pub fn stash(w: u32, h: u32, rgba: Vec<u8>) {
    let slot = PENDING.get_or_init(|| std::sync::Mutex::new(None));
    *slot.lock().unwrap() = Some(PendingSave { w, h, rgba });
}

fn take_pending() -> Option<PendingSave> {
    PENDING.get()?.lock().unwrap().take()
}

// ── Overlay teardown ─────────────────────────────────────────────────
// The save dialog is a regular window; the overlays are layer-shell
// surfaces on the Overlay layer with exclusive keyboard — until unmapped
// they would sit on top of the dialog and starve it of input. Gotchas
// (both measured):
// 1. `handle.update()` on the window whose action handler is currently
//    running is a no-op — the current window removes itself through its
//    own `window` reference.
// 2. The compositor only learns of the destructions when the run loop
//    flushes the connection, so the caller delays `cx.quit()` by ~150ms
//    (see save_selection) instead of quitting the moment the last window
//    closes.

static OVERLAYS: std::sync::OnceLock<std::sync::Mutex<Vec<AnyWindowHandle>>> =
    std::sync::OnceLock::new();

/// Remember an overlay window (main registers one per screen).
pub fn register_overlay(handle: AnyWindowHandle) {
    OVERLAYS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .push(handle);
}

/// Unmap every overlay window (idempotent). The caller passes its own
/// `window` — the current window removes itself directly.
pub fn close_overlays(window: &mut Window, cx: &mut App) {
    let current = window.window_handle();
    window.remove_window();
    let handles = OVERLAYS
        .get()
        .map(|h| std::mem::take(&mut *h.lock().unwrap()))
        .unwrap_or_default();
    for handle in handles {
        if handle == current {
            continue;
        }
        let _ = handle.update(cx, |_, w, _| w.remove_window());
    }
}

/// Where the dialog starts: `~/Pictures/Shotori` (created on demand — the
/// portal rejects a non-existent default folder).
fn default_dir() -> PathBuf {
    let dir = export::save_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Best-effort: an unwritable HOME degrades to the portal's own default
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Suggested file name, e.g. `Shotori_2026-09-26_18-40-12_789.png`
/// (milliseconds distinguish rapid consecutive saves). The dialog asks
/// before overwriting, so no collision suffixing is needed.
/// Also used by the `full` subcommand's directory saves.
pub fn suggested_name() -> String {
    format!(
        "Shotori_{}.png",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S_%3f")
    )
}

/// Run after the app loop exits: if a selection was stashed, open the native
/// save dialog, write the PNG, notify. Also honors the headless test bypass
/// `SHOTORI_DEBUG_SAVE_PATH=<file>` (skips the dialog entirely — the e2e
/// harness has no way to click a portal dialog).
pub fn complete_pending() {
    let Some(PendingSave { w, h, rgba }) = take_pending() else {
        return;
    };

    // Headless e2e / scripting: write straight to the given path
    if let Ok(fixed) = std::env::var("SHOTORI_DEBUG_SAVE_PATH") {
        write_and_notify(std::path::Path::new(&fixed), w, h, &rgba);
        return;
    }

    let picked = rfd::FileDialog::new()
        .set_title(crate::i18n::t!("save.dialog_title").as_ref())
        .set_directory(default_dir())
        .set_file_name(suggested_name())
        .add_filter(crate::i18n::t!("save.png_filter").as_ref(), &["png"])
        .set_can_create_directories(true)
        .save_file();

    let Some(mut path) = picked else {
        println!("[shotori] save canceled");
        return;
    };

    // The user may drop the extension while renaming; the content is PNG
    // either way
    if path.extension().is_none() {
        path.set_extension("png");
    }
    write_and_notify(&path, w, h, &rgba);
}

fn write_and_notify(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    if let Err(e) = export::save_png(path, w, h, rgba) {
        eprintln!("[shotori] save failed: {e:#}");
        crate::notify::send(
            crate::i18n::t!("save.failed.title").as_ref(),
            crate::i18n::t!("save.failed.body").as_ref(),
        );
        return;
    }
    println!("[shotori] saved {w}x{h} → {}", path.display());
    // The path is the thing users actually need — stdout is lost when
    // launched from a keybinding, so the notification is the feedback.
    // The thumbnail renders inside the notify child from this very file
    crate::notify::saved(path);
}
