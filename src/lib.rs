//! # Shotori — a Wayland-first screenshot tool built with gpui-kit
//!
//! ## Layout
//!
//! ```text
//! main.rs / actions.rs        entry point, shared action contract
//! ├─ model/                   pure logic & state (unit-tested)
//! │  ├─ selection             Idle → Dragging → Selected lifecycle
//! │  ├─ session               state shared by all overlays (multi-monitor
//! │  │                        selections, hover, click-snap, cropping)
//! │  ├─ export                crop → PNG → disk/clipboard bytes
//! │  ├─ placement             chrome geometry (label, toolbar, scroll deck)
//! │  ├─ regiondetect          pixel-based smart-selection candidates
//! │  ├─ scroll_sheet          long-screenshot stitching & preview window
//! │  └─ settings              persistent user settings in settings.toml
//! ├─ platform/                compositor & desktop integration
//! │  ├─ capture/              screen freeze: wlr-screencopy; pixels is
//! │  │                        pure format/rotation processing
//! │  ├─ display               capture ↔ gpui display matching
//! │  ├─ scroll_capture        live region sampling & long-capture coordinator
//! │  └─ windowsnap/           window-rect backends: niri / sway /
//! │                           Hyprland IPC
//! ├─ ui/                      gpui windows & elements
//! │  ├─ overlay               overlay assembly (one per screen):
//! │  │                        layer-shell surfaces
//! │  ├─ hud / toolbar         selection visuals & buttons
//! │  ├─ pin                   always-on-top floating crops across outputs
//! │  ├─ scroll_bar            long-screenshot region frame & preview deck
//! │  ├─ ocr_setup             first-run OCR model dialog
//! │  ├─ settings              the settings toplevel (`shotori settings`)
//! │  ├─ e2e                   SHOTORI_DEBUG_* backdoors for headless tests
//! │  └─ theme / image_util    constants; RGBA → RenderImage
//! ├─ annotation/             in-canvas annotations: shapes / arrow / number /
//! │                          pencil / highlighter / mosaic+blur / magnifier /
//! │                          spotlight / eraser / text
//! └─ clipboard / notify / ocr / save_dialog / tray / i18n
//!      the post-selection exits & desktop surfaces: clipboard (resident
//!      daemon), notifications (D-Bus), OCR engine, native save dialog,
//!      StatusNotifierItem tray — plus the UI string tables (rust-i18n,
//!      shared engine with gpui-component)
//! ```
//!
//! Dependency direction: `ui → model`, `model → platform` (session holds
//! captures and snap rects); `platform` and `ui` never reach back up
//! (save for `platform::scroll_capture` driving `model::scroll_sheet`).
//! `actions` is referenced by everyone but references no one — it is the
//! shared vocabulary between keybindings (main), buttons (toolbar) and
//! handlers (overlay, scroll_bar, pin).

// The i18n table lives at the CRATE ROOT (lib.rs) — rust-i18n's `t!`
// expands to `crate::_rust_i18n_translate`, so the declaration must be
// in the root module for every `crate::i18n::t!` / `use ...::t` caller
// to resolve. Same shape as gpui-component's own lib.rs.
// Cargo does not track files read inside proc-macro expansion, so a
// locales/ui.yml edit alone would NOT rebuild — cargo run would
// silently embed the stale table (the "still shows {{}}" trap). The
// include below puts the file on cargo's dependency graph.
const _LOCALES_UI_YML: &str = include_str!("../locales/ui.yml");

rust_i18n::i18n!("locales", fallback = "en");

/// Matches the desktop file basename and the installed hicolor icon name.
pub const APP_ID: &str = "shotori";

pub mod actions;

pub mod args;

/// In-binary microbenchmarks (`shotori --bench`); see bench.rs
pub mod bench;

pub mod model;
/// Developer-only E2E perf suites (`shotori --perf`); behind the
/// `perf` feature — see perf.rs
#[cfg(feature = "perf")]
pub mod perf;
pub mod platform;
pub mod ui;

// in-canvas annotation engine (private module tree: strokes, filters,
// numbering — driven by the overlay's keybindings)
mod annotation;

pub mod clipboard;
pub mod i18n;
pub mod notify;
pub mod ocr;
pub mod render_image;
pub mod save_dialog;
pub mod tray;

/// Boot timing trace for startup profiling, gated on `SHOTORI_BOOT=1`.
/// The epoch is the first call (main entry); every `boot_mark` prints
/// cumulative milliseconds since then, so consecutive labels read as a
/// phase breakdown of "exec to selectable layer".
pub fn boot_mark(label: &str) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    if std::env::var_os("SHOTORI_BOOT").is_none() {
        return;
    }
    let epoch = *EPOCH.get_or_init(Instant::now);
    eprintln!(
        "[boot] {:>8.1} ms  {}",
        epoch.elapsed().as_secs_f64() * 1e3,
        label
    );
}

/// Re-exec ourselves with a subcommand, detached: the child outlives
/// this call and this process (dropping the handle leaves it running —
/// same pattern as the notify child). The tray uses it to launch
/// sessions; the overlay's settings button uses it to hand off to the
/// settings window before the capture process exits.
pub fn launch_detached(subcommand: &str) {
    use std::process::Command;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("[shotori] cannot locate own executable: {e}");
            return;
        }
    };
    match Command::new(exe).arg(subcommand).spawn() {
        Ok(mut child) => {
            // Reap in a detached thread: the tray is a resident process,
            // so every launched session would otherwise pile up as a
            // zombie when it exits. If this process dies first, the
            // child is reparented to init, which reaps it — the thread
            // dying with the process is harmless here (unlike the
            // notify/clipboard children, whose WORK must outlive the
            // parent; a reaper thread has nothing to outlive).
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("[shotori] reaping detached child failed: {e}");
                }
            });
            println!("[shotori] launched `{subcommand}`");
        }
        Err(e) => eprintln!("[shotori] failed to launch `{subcommand}`: {e}"),
    }
}
