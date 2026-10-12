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
