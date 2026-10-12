//! # Tray mode: a resident launcher behind a system tray icon
//!
//! `shotori tray` keeps a tray icon alive (StatusNotifierItem, via
//! gpui-tray) and starts a **separate** `shotori gui` process on
//! activation. The screenshot session keeps
//! its run-and-exit lifecycle — the tray never shares a process with
//! the overlay, so `cx.quit()` at the end of a capture cannot tear the
//! tray down, and a wedged overlay can't take the launcher with it.
//!
//! The menu is deliberately minimal: Screenshot, Settings, Quit.
//! Every real interaction lives in the overlay; the tray is only a
//! way in.
//!
//! Quit mode is [`QuitMode::Explicit`]: the app has no windows, so the
//! default last-window-closed heuristic would exit immediately.

use gpui_kit::*;
use gpui_tray::{Icon, Tray};

actions!(tray, [TakeScreenshot, OpenSettings, QuitTray]);

/// Keeps the [`Tray`] alive for the process lifetime (gpui-tray removes
/// the native item on drop).
struct TrayGlobal(Tray);

impl Global for TrayGlobal {}

/// Run the tray app. Returns when the user quits from the menu.
pub fn run() {
    // The desktop launcher starts the tray, so a second launch (impatient
    // double-click, autostart race) would stack a second SNI icon —
    // hold the single-instance lock for the whole tray lifetime instead.
    let Some(_instance_lock) = instance_lock::acquire() else {
        eprintln!("[shotori] tray already running");
        return;
    };
    application().with_quit_mode(QuitMode::Explicit).run(|cx| {
        if let Err(e) = setup(cx) {
            eprintln!("[shotori] tray unavailable: {e:#}");
            eprintln!(
                "[shotori] hint: Linux needs a StatusNotifierItem host \
                     (waybar, KDE plasma, a GNOME appindicator extension)"
            );
            cx.quit();
        }
    });
}

fn setup(cx: &mut App) -> gpui_tray::Result<()> {
    cx.set_app_identity(crate::APP_ID, "Shotori");
    cx.on_action(take_screenshot)
        .on_action(open_settings)
        .on_action(quit_tray);

    let tray = Tray::builder()
        .icon(icon()?)
        .tooltip(crate::i18n::t!("tray.tooltip").as_ref())
        .menu(|_| {
            vec![
                MenuItem::action(crate::i18n::t!("tray.screenshot").as_ref(), TakeScreenshot),
                MenuItem::action(crate::i18n::t!("tray.settings").as_ref(), OpenSettings),
                MenuItem::separator(),
                MenuItem::action(crate::i18n::t!("tray.quit").as_ref(), QuitTray),
            ]
        })
        // Left-click / primary activation does the same as the menu item
        .on_activate(TakeScreenshot)
        .build(cx)?;
    cx.set_global(TrayGlobal(tray));
    println!("[shotori] tray running");
    Ok(())
}

/// Re-exec ourselves with a subcommand, detached — shared with the
/// overlay's settings handoff (see [`crate::launch_detached`]).
fn launch(subcommand: &str) {
    crate::launch_detached(subcommand);
}

fn take_screenshot(_: &TakeScreenshot, _: &mut App) {
    launch("gui");
}

fn open_settings(_: &OpenSettings, _: &mut App) {
    launch("settings");
}

fn quit_tray(_: &QuitTray, cx: &mut App) {
    let tray = cx.global::<TrayGlobal>().0.clone();
    if let Err(e) = tray.close(cx) {
        eprintln!("[shotori] tray: close failed: {e}");
    }
    cx.quit();
}

/// Decode the checked-in raster asset; tray hosts scale its RGBA pixels to
/// their own panel size, without relying on installed desktop icon themes.
fn icon() -> gpui_tray::Result<Icon> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/app/shotori-64.png"),
        image::ImageFormat::Png,
    )
    .map_err(|error| gpui_tray::Error::InvalidIcon(error.to_string()))?
    .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
}

mod instance_lock {
    //! Single-instance guard for tray mode. `flock` semantics: the lock
    //! belongs to the open file description, so the kernel releases it on
    //! process exit — crash-safe, no stale PID file to clean up.

    use std::fs::File;
    use std::path::{Path, PathBuf};

    /// Lock file lives in XDG_RUNTIME_DIR — user-private tmpfs, cleared
    /// on logout — with the portable temp dir as a fallback.
    fn default_path() -> PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("shotori-tray.lock")
    }

    fn acquire_at(path: &Path) -> Option<File> {
        let lock = File::create(path).ok()?;
        // try_lock is an inherent File method in edition 2024 (the
        // FileExt import would be flagged unused).
        lock.try_lock().ok()?;
        Some(lock)
    }

    pub fn acquire() -> Option<File> {
        acquire_at(&default_path())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn temp_lock_path(name: &str) -> PathBuf {
            std::env::temp_dir().join(format!("shotori-test-{name}-{}.lock", std::process::id()))
        }

        #[test]
        fn second_acquire_fails_while_first_is_held() {
            let path = temp_lock_path("mutex");
            let first = acquire_at(&path).expect("first acquire must succeed");
            assert!(
                acquire_at(&path).is_none(),
                "a second lock on the same file must be refused"
            );
            // Dropping the holder must free the lock for the next caller.
            drop(first);
            assert!(acquire_at(&path).is_some(), "lock must free after drop");
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn independent_files_do_not_conflict() {
            let a = acquire_at(&temp_lock_path("a")).expect("lock a");
            let b = acquire_at(&temp_lock_path("b")).expect("lock b must be independent");
            drop((a, b));
        }
    }
}
