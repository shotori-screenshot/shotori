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
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(PendingSave { w, h, rgba });
}

fn take_pending() -> Option<PendingSave> {
    PENDING
        .get()?
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
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
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(handle);
}

/// Unmap every overlay window (idempotent). The caller passes its own
/// `window` — the current window removes itself directly.
pub fn close_overlays(window: &mut Window, cx: &mut App) {
    let current = window.window_handle();
    window.remove_window();
    let handles = OVERLAYS
        .get()
        .map(|h| std::mem::take(&mut *h.lock().unwrap_or_else(|poisoned| poisoned.into_inner())))
        .unwrap_or_default();
    for handle in handles {
        if handle == current {
            continue;
        }
        let _ = handle.update(cx, |_, w, _| w.remove_window());
    }
}

/// Unmap every overlay and quit once the destroys are flushed — the
/// shared teardown of every flow that hands off to something outside the
/// app loop (the portal save dialog, background OCR). Quitting the moment
/// the last window closes leaves frozen frames mapped on screen: the
/// compositor only learns of the surface destructions when the run loop
/// flushes the connection (measured), so quit waits ~150ms.
pub fn quit_after_close(window: &mut Window, cx: &mut App) {
    cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
    close_overlays(window, cx);
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Where the dialog starts: last chosen folder when `remember_last_dir` is
/// enabled, otherwise `[export].save_dir` (`~/Pictures/Shotori` by default,
/// created on demand because portals reject a non-existent default folder).
fn default_dir() -> PathBuf {
    let export = crate::model::settings::current().export();
    let dir = if export.remember_last_dir
        && let Some(last) = crate::model::settings::read_last_save_dir()
    {
        last
    } else {
        export
            .resolved_save_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    // Best-effort: an unwritable path degrades to the portal's own default
    drop(std::fs::create_dir_all(&dir));
    dir
}

fn filter_label(format: crate::model::settings::ExportFormat) -> std::borrow::Cow<'static, str> {
    use crate::model::settings::ExportFormat;
    match format {
        ExportFormat::Png => crate::i18n::t!("save.png_filter"),
        ExportFormat::Jpeg => crate::i18n::t!("save.jpeg_filter"),
        ExportFormat::Webp => crate::i18n::t!("save.webp_filter"),
    }
}

/// Suggested file name from `[export]` settings (template + format extension).
pub fn suggested_name_for(w: u32, h: u32) -> String {
    export::suggested_filename(w, h)
}

/// Run after the app loop exits: if a selection was stashed, open the native
/// save dialog, write the image, notify. Also honors the headless test bypass
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

    let export_cfg = crate::model::settings::current().export();
    let mut dialog = rfd::FileDialog::new()
        .set_title(crate::i18n::t!("save.dialog_title").as_ref())
        .set_directory(default_dir())
        .set_file_name(suggested_name_for(w, h))
        .add_filter(
            filter_label(export_cfg.format).as_ref(),
            export_cfg.format.filter_extensions(),
        )
        .set_can_create_directories(true);
    for fmt in crate::model::settings::ExportFormat::ALL {
        if fmt != export_cfg.format {
            dialog = dialog.add_filter(filter_label(fmt).as_ref(), fmt.filter_extensions());
        }
    }

    let Some(mut path) = dialog.save_file() else {
        println!("[shotori] save canceled");
        return;
    };

    // If the user drops the extension while renaming, append the configured format
    if path.extension().is_none() {
        path.set_extension(export_cfg.format.extension());
    }
    if export_cfg.remember_last_dir
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        crate::model::settings::write_last_save_dir(parent);
    }
    write_and_notify(&path, w, h, &rgba);
}

/// Direct save to `[export].save_dir` without opening a dialog (issue #42),
/// plus optional `copy_on_save` clipboard copy and desktop notification.
pub fn save_direct_and_notify(w: u32, h: u32, rgba: &[u8]) {
    let path = match export::save_to_default_dir(w, h, rgba) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[shotori] direct save failed: {e:#}");
            crate::notify::send(
                crate::i18n::t!("save.failed.title").as_ref(),
                crate::i18n::t!("save.failed.body").as_ref(),
            );
            return;
        }
    };
    println!("[shotori] saved {w}x{h} → {}", path.display());
    maybe_copy_on_save(w, h, rgba);
    crate::notify::saved(&path);
}

fn maybe_copy_on_save(w: u32, h: u32, rgba: &[u8]) {
    if !crate::model::settings::current().export().copy_on_save {
        return;
    }
    match export::encode_png_fast(w, h, rgba)
        .and_then(|png| crate::clipboard::copy_image(w, h, rgba, &png))
    {
        Ok(()) => println!("[shotori] also copied {w}x{h} to clipboard (copy_on_save)"),
        Err(e) => eprintln!("[shotori] copy_on_save failed: {e:#}"),
    }
}

fn write_and_notify(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    if let Err(e) = export::save_image(path, w, h, rgba) {
        eprintln!("[shotori] save failed: {e:#}");
        crate::notify::send(
            crate::i18n::t!("save.failed.title").as_ref(),
            crate::i18n::t!("save.failed.body").as_ref(),
        );
        return;
    }
    println!("[shotori] saved {w}x{h} → {}", path.display());
    maybe_copy_on_save(w, h, rgba);
    // The path is the thing users actually need — stdout is lost when
    // launched from a keybinding, so the notification is the feedback.
    // The thumbnail renders inside the notify child from this very file
    crate::notify::saved(path);
}
