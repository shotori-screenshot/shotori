//! # Shotori entry point: assembly, keybindings, window creation
//!
//! Module layout (avoiding a god file):
//! - `capture`: screen freeze (wlr-screencopy; multi-output, dedicated
//!   wayland connection)
//! - `display`: capture ↔ gpui display matching and waiting
//! - `clipboard`: clipboard copy (zwlr_data_control + resident daemon)
//! - `overlay`: overlay assembly (one window per screen)
//! - `hud` / `toolbar`: the overlay's visual pieces
//! - `selection` / `export` / `image_util`: pure logic
//! - `ocr`: selection OCR (rapidocr-core; model set configurable in
//!   settings.toml)
//! - `theme`: visual constants

use gpui_kit::*;

use clap::Parser as _;
use shotori::actions::QuitOverlay;
use shotori::clipboard;
use shotori::platform::capture;
use shotori::platform::display;
use shotori::ui::overlay::Overlay;

fn main() {
    let boot = std::time::Instant::now();
    shotori::boot_mark("main entry");

    // Notification child: `shotori --notify <summary> <body>` (see notify.rs)
    if std::env::args().nth(1).as_deref() == Some(shotori::notify::NOTIFY_ARG) {
        std::process::exit(shotori::notify::notify_main());
    }

    // Microbenchmarks: `shotori --bench [name]` (see bench.rs) — must run
    // before clap (free-form argument) and before any GUI/theme init
    if std::env::args().nth(1).as_deref() == Some(shotori::bench::BENCH_ARG) {
        std::process::exit(shotori::bench::bench_main());
    }

    // E2E perf suites: `shotori --perf [suite...]` (see perf.rs) —
    // developer-only, compiled behind the `perf` cargo feature
    #[cfg(feature = "perf")]
    if std::env::args().nth(1).as_deref() == Some(shotori::perf::PERF_ARG) {
        std::process::exit(shotori::perf::perf_main());
    }

    // Clipboard daemon (Linux only): the background resident process
    // behind the copy action (see the resident-offer model in
    // clipboard.rs).
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some(clipboard::DAEMON_ARG) {
        if let Err(e) = clipboard::daemon_main() {
            eprintln!("[shotori] clipboard daemon exiting: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    // Command line (after the two internal child-process entry points
    // above, which take free-form trailing arguments and must not be
    // flag-parsed). clap handles --help/--version and bad arguments.
    let args = shotori::args::Cli::parse();

    // Locale pickup runs before EVERY later branch (tray, settings
    // window, overlay) — they all return before `settings::init` would
    // run, so the language gets its own early read of the file.
    shotori::i18n::init(args.no_config);

    // Tray mode: a resident launcher, not a screenshot session — bail
    // out before theme init / capture. Returns when the user quits the
    // tray.
    if matches!(args.command, Some(shotori::args::Command::Tray)) {
        shotori::tray::run();
        return;
    }

    // ⓪ Theme resolution (--theme / --print-theme / XDG config file);
    // must run before any window opens. --print-theme exits inside.
    shotori::ui::theme::load::init(&args);

    // ⓪" The settings window: a regular toplevel, not a capture
    // session — nothing below (settings pickup, capture, overlays)
    // applies. The window loads and reports the file itself.
    if matches!(args.command, Some(shotori::args::Command::Settings)) {
        shotori::ui::settings::run();
        return;
    }

    // ⓪''' Settings pickup (settings.toml in the config dir; --no-config
    // skips it, like the theme). Errors are warnings, never fatal.
    shotori::model::settings::init(&args);

    // ⓪' Non-interactive full capture (`shotori full`) — no overlay
    if let Some(shotori::args::Command::Full {
        clipboard,
        path,
        delay,
    }) = &args.command
    {
        full_capture(*clipboard, path.clone(), *delay);
    }

    // ① Freeze all screens (must complete before the overlays appear)
    let caps = match capture::capture_all_outputs() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[shotori] capture failed: {e:#}");
            std::process::exit(1);
        }
    };
    println!(
        "[shotori] frozen {} screen(s): {} ({})",
        caps.len(),
        caps.iter()
            .map(|c| format!(
                "{} {}x{}{}",
                c.output_name,
                c.width,
                c.height,
                if c.rotated() { " (rotated)" } else { "" }
            ))
            .collect::<Vec<_>>()
            .join(" · "),
        boot.elapsed().as_millis()
    );
    shotori::boot_mark("capture done (screencopy frozen)");

    // Warm the shared OCR engine once per screenshot session.
    std::thread::Builder::new()
        .name("shotori-ocr-warmup".into())
        .spawn(shotori::ocr::warmup)
        .ok();

    // Window snapping: ask the compositor (niri / sway / Hyprland IPC)
    // where every visible window is. None = compositor without a
    // supported IPC; the feature silently turns off (see windowsnap).
    let snaps = shotori::platform::windowsnap::query();
    println!(
        "[shotori] window snap: {}",
        match &snaps {
            Some(s) if !s.is_empty() => format!("{} window(s)", s.len()),
            Some(_) => "no visible windows".to_owned(),
            None => "unavailable on this compositor".to_owned(),
        }
    );
    shotori::boot_mark("windowsnap done");

    // ② Overlays (one per screen)
    gpui_kit::application()
        .with_assets(shotori::ui::toolbar::ToolbarSource)
        .run(move |cx| {
            shotori::boot_mark("gpui run entered");
            cx.set_app_identity(shotori::APP_ID, "Shotori");
            gpui_kit::base::init(cx);
            // The component design system's global state: the inline
            // color picker (the selected swatch's editor) comes from
            // gpui-component and needs its theme registry.
            gpui_kit::component::init(cx);
            shotori::ui::ocr_setup::init(cx);
            shotori::actions::init_annotation_keybindings(cx);

            // Keybindings are scoped by key_context (see actions.rs);
            // the configurable commands honor settings.toml [keys]
            let (settings, warnings) = shotori::model::settings::Settings::load(
                shotori::model::settings::config_path().as_deref(),
            );
            for warning in &warnings {
                eprintln!("[shotori] settings: {warning}");
            }
            shotori::actions::bind_keys(cx, settings.keys());
            // Backstop: Esc still exits if the overlay somehow loses focus.
            // Note: dispatch_action inside a window does NOT bubble up here
            // (actions stop at the focus path — measured); the overlay's own
            // QuitOverlay handler is the real exit implementation; this line
            // only guards the rare focus-loss case
            cx.on_action(|_: &QuitOverlay, cx| cx.quit());

            // Window creation lives in an async task: displays() is always
            // empty during synchronous startup (upstream zed#46378) and
            // becomes usable after the first event-loop pass — match each
            // capture to a display_id and open windows here
            cx.spawn(async move |cx| {
                let targets = display::await_display_ids(caps, cx).await;
                shotori::boot_mark("displays matched");
                #[cfg(feature = "perf")]
                let perf_warm = std::env::var_os("SHOTORI_PERF_WARM").is_some();
                #[cfg(feature = "perf")]
                let mut warm_reopen = None;
                cx.update(|cx| {
                    let targets: Vec<_> = targets.into_iter()
                        .map(|(cap, did)| (std::sync::Arc::new(cap), did))
                        .collect();
                    let session = cx.new(|_| {
                        shotori::model::session::ScreenshotSession::new(
                            targets.iter().map(|(cap, _)| cap.clone()).collect(),
                            snaps.unwrap_or_default(),
                        )
                    });
                    // Smart region detection: analyze each frozen output
                    // once on the background executor (pure pixel work,
                    // panic-free by contract — the pipeline indexes
                    // through checked accessors). Results land in the
                    // session per output; hover/click snapping lights up
                    // silently wherever the analysis found candidates.
                    // Until then the compositor snap rects alone serve
                    // the same interaction.
                    {
                        let caps: Vec<std::sync::Arc<shotori::platform::capture::Capture>> =
                            targets.iter().map(|(cap, _)| cap.clone()).collect();
                        let session = session.clone();
                        cx.spawn(async move |cx| {
                            let results = cx
                                .background_executor()
                                .spawn(async move {
                                    caps.iter()
                                        .filter(|cap| {
                                            cap.width >= 3 && cap.height >= 3 && !cap.rgba.is_empty()
                                        })
                                        .map(|cap| {
                                            (
                                                cap.output_name.clone(),
                                                shotori::model::regiondetect::RegionDetector::analyze(
                                                    &cap.rgba,
                                                    cap.width,
                                                    cap.height,
                                                ),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .await;
                            cx.update(|cx| {
                                session.update(cx, |session, cx| {
                                    for (name, detector) in results {
                                        println!(
                                            "[shotori] smart regions: {} candidate(s) on {name}",
                                            detector.candidate_count()
                                        );
                                        session.set_region_detector(&name, detector);
                                    }
                                    cx.notify();
                                });
                            });
                        })
                        .detach();
                    }
                    shotori::boot_mark("region detection dispatched");
                    for (cap, did) in targets {
                        if did.is_none() {
                            eprintln!(
                                "[shotori] warning: {} matched no display, placement left to the compositor",
                                cap.output_name
                            );
                        }
                        // True logical size (fractional scale + transform)
                        // for the layer surface request; see window_options
                        let (lw, lh) = cap.logical_size_f32();
                        let logical = size(px(lw), px(lh));
                        // The GLOBAL logical rect (the session's space):
                        // pins span outputs through it (see ui::pin)
                        shotori::ui::pin::register_output(
                            Bounds::new(
                                point(px(cap.logical_pos.0 as f32), px(cap.logical_pos.1 as f32)),
                                logical,
                            ),
                            did,
                        );
                        // perf warm-window suite: remember the first
                        // target so a second overlay can be opened later
                        #[cfg(feature = "perf")]
                        if perf_warm && warm_reopen.is_none() {
                            warm_reopen = Some((cap.clone(), did, session.clone(), logical));
                        }
                        let handle = cx
                            .open_window(
                                Overlay::window_options(did, logical),
                                |window, cx| {
                                    cx.new(|cx| {
                                        Overlay::new(cap, session.clone(), window, cx)
                                    })
                                },
                            )
                            .expect("failed to open layer-shell window");
                        // Needed by the save flow to unmap every overlay
                        // before the file dialog takes over the screen
                        shotori::save_dialog::register_overlay(handle.into());
                    }
                    shotori::boot_mark("windows opened (layer-shell requested)");
                });

                // perf warm-window suite: open a second overlay once the
                // first has rendered — its construct+render time shows
                // what a resident process would pay per shot (no wgpu init)
                #[cfg(feature = "perf")]
                if let Some((cap, did, session, logical)) = warm_reopen {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(400))
                        .await;
                    cx.update(|cx| {
                        shotori::boot_mark("warm reopen requested");
                        let handle = cx
                            .open_window(
                                Overlay::window_options(did, logical),
                                |window, cx| {
                                    cx.new(|cx| Overlay::new(cap, session, window, cx))
                                },
                            )
                            .expect("failed to open warm layer-shell window");
                        shotori::save_dialog::register_overlay(handle.into());
                    });
                }
            })
            .detach();
        });

    // ③ Save flow: Ctrl+S stashed a selection and quit the overlay — now
    // that the app loop (and the covering layer-shell surface) is gone, the
    // native save dialog can take over the screen. No-op on every other
    // exit path.
    shotori::save_dialog::complete_pending();

    // Startup-suite children report their peak RSS on the way out
    #[cfg(feature = "perf")]
    shotori::perf::report_child_rss();
}

/// `shotori full`: capture every screen with no overlay. The session
/// machinery is reused as-is (`select_all` builds the union selection,
/// `crop_original` walks the same cross-screen export path as the
/// interactive flow), so density/gap semantics cannot drift between
/// modes. Clipboard when no `--path` is given; a directory `--path`
/// gets the dialog-style timestamped name.
fn full_capture(clipboard: bool, path: Option<std::path::PathBuf>, delay: f32) -> ! {
    use shotori::model::session::ScreenshotSession;

    if delay > 0. {
        std::thread::sleep(std::time::Duration::from_secs_f32(delay));
    }
    let caps = match capture::capture_all_outputs() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[shotori] capture failed: {e:#}");
            shotori::notify::send(
                "Couldn’t capture screen",
                "Try taking the screenshot again.",
            );
            std::process::exit(1);
        }
    };
    let first = caps[0].output_name.clone();
    let mut session = ScreenshotSession::new(
        caps.into_iter().map(std::sync::Arc::new).collect(),
        Vec::new(),
    );
    session.select_all();
    let Some((w, h, rgba)) = session.crop_original(&first) else {
        eprintln!("[shotori] full capture produced nothing");
        std::process::exit(1);
    };

    let mut ok = true;
    // Clipboard: the default, and alongside --path when asked explicitly.
    // Fast-tier PNG: the bytes go through the daemon's pipe, not a download
    if clipboard || path.is_none() {
        let png = match shotori::model::export::encode_png_fast(w, h, &rgba) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[shotori] clipboard: {e:#}");
                ok = false;
                Vec::new()
            }
        };
        if !png.is_empty()
            && let Err(e) = clipboard::copy_image(w, h, &rgba, &png)
        {
            eprintln!("[shotori] clipboard: {e:#}");
            ok = false;
        } else if !png.is_empty() {
            shotori::notify::copied(&png);
        }
    }
    if let Some(p) = path {
        let file = if p.is_dir() {
            p.join(shotori::save_dialog::suggested_name())
        } else {
            p
        };
        match shotori::model::export::save_png(&file, w, h, &rgba) {
            // The thumbnail renders in the notify child from the saved file
            Ok(()) => shotori::notify::saved(&file),
            Err(e) => {
                eprintln!("[shotori] save: {e:#}");
                ok = false;
            }
        }
    }
    std::process::exit(i32::from(!ok));
}
