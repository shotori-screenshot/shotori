//! # CLI subcommand runners (`full`, `ocr`, `pin`, `outputs`) and shared
//! selection-target resolution
//!
//! Kept separate from `main.rs` so the binary entry point stays focused on
//! process dispatch and overlay window assembly, and so target resolution,
//! pin placement math, and output formatting are unit-tested without a
//! compositor.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::*;

use crate::args::{
    FullArgs, OcrArgs, OutputsArgs, PinArgs, PointSpec, RegionSpec, SelectionTarget,
};
use crate::model::session::ScreenshotSession;
use crate::platform::capture::{self, Capture, OutputInfo};
use crate::platform::windowsnap::{self, CompositorSnapshot};

/// Sleep before screen capture when `--delay` was requested.
/// Values are already validated as finite and `>= 0` by clap; capped at one
/// hour because `--delay` settles transient menus/popups, not cron jobs.
pub fn sleep_delay(delay: f32) {
    if delay > 0.0 && delay.is_finite() {
        std::thread::sleep(Duration::from_secs_f32(delay.min(3600.0)));
    }
}

/// Failure modes when resolving a [`SelectionTarget`] against captured screens.
#[derive(Debug, PartialEq)]
pub enum TargetError {
    UnknownOutput {
        requested: String,
        available: Vec<String>,
    },
    OutOfBounds {
        region: RegionSpec,
        desktop: Option<Bounds<Pixels>>,
    },
    NoFocusedWindow,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownOutput {
                requested,
                available,
            } => {
                write!(
                    f,
                    "unknown output '{requested}' (connected: {})",
                    if available.is_empty() {
                        "none".to_owned()
                    } else {
                        available.join(", ")
                    }
                )
            }
            Self::OutOfBounds { region, desktop } => match desktop {
                Some(d) => write!(
                    f,
                    "region {region} is outside the desktop bounds ({:.0},{:.0} {:.0}x{:.0})",
                    f32::from(d.origin.x),
                    f32::from(d.origin.y),
                    f32::from(d.size.width),
                    f32::from(d.size.height),
                ),
                None => write!(f, "region {region} does not overlap any connected output"),
            },
            Self::NoFocusedWindow => write!(
                f,
                "no focused window geometry available (unsupported compositor IPC or empty workspace)"
            ),
        }
    }
}

impl std::error::Error for TargetError {}

/// Apply a CLI [`SelectionTarget`] onto an initialized [`ScreenshotSession`].
pub fn apply_target(
    session: &mut ScreenshotSession,
    target: &SelectionTarget,
    snapshot: &CompositorSnapshot,
) -> Result<(), TargetError> {
    match target {
        SelectionTarget::AllScreens => {
            session.select_all_on(snapshot.focused_output.as_deref());
            Ok(())
        }
        SelectionTarget::FocusedScreen => {
            if let Some(focused) = snapshot.focused_output.as_deref()
                && session.select_output(focused)
            {
                return Ok(());
            }
            if let Some(win) = snapshot.focused_window()
                && let Some(dom) = session.dominant_output_for(win).map(str::to_owned)
                && session.select_output(&dom)
            {
                return Ok(());
            }
            let first = session.output_names().first().copied().map(str::to_owned);
            if let Some(first) = first
                && session.select_output(&first)
            {
                return Ok(());
            }
            Ok(())
        }
        SelectionTarget::Output(name) => {
            if session.select_output(name) {
                Ok(())
            } else {
                Err(TargetError::UnknownOutput {
                    requested: name.clone(),
                    available: session
                        .output_names()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                })
            }
        }
        SelectionTarget::Region(spec) => {
            let bounds = Bounds::new(
                point(px(spec.x), px(spec.y)),
                size(px(spec.width), px(spec.height)),
            );
            if session.select_bounds(bounds) {
                Ok(())
            } else {
                Err(TargetError::OutOfBounds {
                    region: *spec,
                    desktop: session.desktop_logical_bounds(),
                })
            }
        }
        SelectionTarget::FocusedWindow => {
            let Some(bounds) = snapshot.focused_window() else {
                return Err(TargetError::NoFocusedWindow);
            };
            if session.select_bounds(bounds) {
                Ok(())
            } else {
                Err(TargetError::NoFocusedWindow)
            }
        }
    }
}

/// `shotori full`: non-interactive screen capture (all screens, focused screen,
/// named output, region, or focused window) to clipboard, file, and/or stdout.
pub fn run_full(args: &FullArgs) -> i32 {
    sleep_delay(args.delay);

    let target = args.target();
    let snapshot = if target.uses_compositor_ipc() {
        windowsnap::query_snapshot().unwrap_or_default()
    } else {
        CompositorSnapshot::default()
    };

    let caps = match capture::capture_all_outputs() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[shotori] capture failed: {e:#}");
            if !args.no_notify {
                crate::notify::send(
                    "Couldn’t capture screen",
                    "Try taking the screenshot again.",
                );
            }
            return 1;
        }
    };
    let fallback_host = caps[0].output_name.clone();
    let mut session = ScreenshotSession::new(
        caps.into_iter().map(Arc::new).collect(),
        snapshot.rects.clone(),
    );
    if let Err(err) = apply_target(&mut session, &target, &snapshot) {
        eprintln!("[shotori] {err}");
        return 1;
    }
    let host = session
        .active_output_name()
        .unwrap_or(&fallback_host)
        .to_owned();
    let Some((w, h, rgba)) = session.crop_original(&host) else {
        eprintln!("[shotori] capture produced an empty region");
        return 1;
    };

    let path_is_stdout = args.path.as_deref() == Some(Path::new("-"));
    let file_dest = args.path.as_deref().filter(|p| *p != Path::new("-"));
    let to_stdout = args.stdout || path_is_stdout;
    let to_clipboard = args.clipboard || (!to_stdout && file_dest.is_none());

    let mut ok = true;
    let fast_png = if to_clipboard || to_stdout {
        match crate::model::export::encode_png_fast(w, h, &rgba) {
            Ok(png) => Some(png),
            Err(e) => {
                eprintln!("[shotori] PNG encode failed: {e:#}");
                ok = false;
                None
            }
        }
    } else {
        None
    };

    if to_clipboard && let Some(png) = fast_png.as_deref() {
        if let Err(e) = crate::clipboard::copy_image(w, h, &rgba, png) {
            eprintln!("[shotori] clipboard: {e:#}");
            ok = false;
        } else if !args.no_notify {
            crate::notify::copied(png);
        }
    }

    if to_stdout && let Some(png) = fast_png.as_deref() {
        let mut out = std::io::stdout().lock();
        if let Err(e) = out.write_all(png).and_then(|()| out.flush()) {
            eprintln!("[shotori] stdout: {e}");
            ok = false;
        }
    }

    if let Some(p) = file_dest {
        let file = if p.is_dir() {
            p.join(crate::save_dialog::suggested_name_for(w, h))
        } else {
            p.to_path_buf()
        };
        match crate::model::export::save_image(&file, w, h, &rgba) {
            Ok(()) => {
                if !args.no_notify {
                    crate::notify::saved(&file);
                }
            }
            Err(e) => {
                eprintln!("[shotori] save: {e:#}");
                ok = false;
            }
        }
    }

    i32::from(!ok)
}

/// `shotori ocr`: recognize text from an image file / stdin or screen capture.
pub fn run_ocr(args: &OcrArgs) -> i32 {
    let (w, h, rgba) = if let Some(path) = &args.image {
        match load_rgba_path(path) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("[shotori] ocr: {e:#}");
                return 1;
            }
        }
    } else {
        sleep_delay(args.delay);
        let target = args.screen_target();
        let snapshot = if target.uses_compositor_ipc() {
            windowsnap::query_snapshot().unwrap_or_default()
        } else {
            CompositorSnapshot::default()
        };
        let caps = match capture::capture_all_outputs() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[shotori] capture failed: {e:#}");
                return 1;
            }
        };
        let fallback_host = caps[0].output_name.clone();
        let mut session = ScreenshotSession::new(
            caps.into_iter().map(Arc::new).collect(),
            snapshot.rects.clone(),
        );
        if let Err(err) = apply_target(&mut session, &target, &snapshot) {
            eprintln!("[shotori] {err}");
            return 1;
        }
        let host = session
            .active_output_name()
            .unwrap_or(&fallback_host)
            .to_owned();
        let Some(crop) = session.crop_original(&host) else {
            eprintln!("[shotori] ocr: capture produced an empty region");
            return 1;
        };
        crop
    };

    if crate::ocr::models_missing() {
        if !args.download {
            eprintln!(
                "[shotori] OCR models are not downloaded yet (stored in {}).\n\
                 Pass `--download` to download them now, or open `shotori settings ocr`.",
                crate::ocr::model_dir_display()
            );
            return 1;
        }
        if let Err(e) = download_ocr_models_cli() {
            eprintln!("[shotori] OCR model download failed: {e:#}");
            return 1;
        }
    }

    let text = match crate::ocr::run_ocr(&rgba, w, h) {
        Ok(t) => crate::ocr::normalize_recognized_text(
            t,
            crate::model::settings::current().ocr_join_lines(),
        ),
        Err(e) => {
            eprintln!("[shotori] OCR failed: {e:#}");
            return 1;
        }
    };

    let path_is_stdout = args.path.as_deref() == Some(Path::new("-"));
    let file_dest = args.path.as_deref().filter(|p| *p != Path::new("-"));
    let to_stdout = args.stdout || path_is_stdout || (!args.clipboard && file_dest.is_none());

    let mut ok = true;
    if to_stdout {
        let mut out = std::io::stdout().lock();
        let res = if text.is_empty() {
            out.flush()
        } else {
            out.write_all(text.as_bytes())
                .and_then(|()| out.write_all(b"\n"))
                .and_then(|()| out.flush())
        };
        if let Err(e) = res {
            eprintln!("[shotori] stdout: {e}");
            ok = false;
        }
    }

    if args.clipboard {
        if text.is_empty() {
            if !args.no_notify {
                crate::notify::send(
                    crate::i18n::t!("notify.ocr.empty.title").as_ref(),
                    crate::i18n::t!("notify.ocr.empty.body").as_ref(),
                );
            }
        } else if let Err(e) = crate::clipboard::copy_text(text.clone()) {
            eprintln!("[shotori] OCR clipboard copy failed: {e:#}");
            ok = false;
        } else if !args.no_notify {
            let preview: String = text
                .replace('\n', " ")
                .chars()
                .filter(|c| !c.is_control())
                .take(60)
                .collect();
            crate::notify::ocr_copied(&text, &preview);
        }
    }

    if let Some(file) = file_dest {
        let payload = if text.is_empty() {
            String::new()
        } else {
            format!("{text}\n")
        };
        if let Err(e) = std::fs::write(file, payload) {
            eprintln!("[shotori] write {}: {e}", file.display());
            ok = false;
        }
    }

    i32::from(!ok)
}

fn download_ocr_models_cli() -> anyhow::Result<()> {
    let progress = Arc::new(crate::ocr::DownloadProgress::for_selected_set());
    crate::ocr::spawn_download(progress.clone());
    let mut last_idx = 0u8;
    while progress.is_running() {
        let idx = progress.file_idx.load(Ordering::Relaxed);
        if idx != 0 && idx != last_idx {
            last_idx = idx;
            eprintln!(
                "[shotori] downloading OCR model {}/{}: {}…",
                idx,
                progress.file_count,
                crate::ocr::asset_name(idx)
            );
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    if progress.finished_ok() {
        eprintln!("[shotori] OCR model download complete");
        Ok(())
    } else {
        anyhow::bail!("{}", progress.error())
    }
}

/// `shotori pin`: float an image file / stdin or screen capture as an
/// always-on-top desktop pin. Forwards to an existing pin owner when one is
/// already running on this Wayland session; otherwise becomes the owner and
/// stays resident until the last pin closes.
pub fn run_pin(args: &PinArgs) -> i32 {
    let (spec, geometry_caps) = match build_pin_payload(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[shotori] pin: {e:#}");
            return 1;
        }
    };
    let (pw, ph) = (spec.w, spec.h);
    let prepared = match crate::ui::pin::prepare(spec) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[shotori] pin: {e:#}");
            return 1;
        }
    };

    #[cfg(target_os = "linux")]
    if matches!(prepared, crate::ui::pin::Prepared::Forwarded) {
        eprintln!("[shotori] pinned {pw}x{ph}");
        return 0;
    }

    let exit_code = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let exit_out = exit_code.clone();
    application()
        .with_assets(crate::ui::toolbar::ToolbarSource)
        .run(move |cx| {
            cx.set_app_identity(crate::APP_ID, "Shotori");
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::actions::bind_keys(cx, crate::model::settings::current().keys())
                .into_iter()
                .for_each(|id| eprintln!("[shotori] keys: {id} could not be bound"));

            let exit_code = exit_out.clone();
            cx.spawn(async move |cx| {
                let targets = crate::platform::display::await_display_ids(geometry_caps, cx).await;
                cx.update(|cx| {
                    for (cap, did) in targets {
                        let (lw, lh) = cap.logical_size_f32();
                        let logical = size(px(lw), px(lh));
                        let physical_scale = (cap.width as f32 / lw.max(1.)).max(1.);
                        crate::ui::pin::register_output(
                            Bounds::new(
                                point(px(cap.logical_pos.0 as f32), px(cap.logical_pos.1 as f32)),
                                logical,
                            ),
                            did,
                            physical_scale,
                        );
                    }
                    if let Err(e) = crate::ui::pin::open(prepared, cx) {
                        eprintln!("[shotori] pin failed: {e:#}");
                        exit_code.store(1, Ordering::Relaxed);
                        cx.quit();
                    } else {
                        eprintln!("[shotori] pinned {pw}x{ph}");
                    }
                });
            })
            .detach();
        });
    exit_code.load(Ordering::Relaxed)
}

fn build_pin_payload(args: &PinArgs) -> anyhow::Result<(crate::ui::pin::PinSpec, Vec<Capture>)> {
    if let Some(path) = &args.image {
        let (w, h, rgba) = load_rgba_path(path)?;
        let infos = capture::probe_outputs()?;
        let target_info = if let Some(requested) = &args.output {
            infos.iter().find(|o| o.name == *requested).ok_or_else(|| {
                TargetError::UnknownOutput {
                    requested: requested.clone(),
                    available: infos.iter().map(|o| o.name.clone()).collect(),
                }
            })?
        } else {
            let focused = windowsnap::query_snapshot().and_then(|s| s.focused_output);
            focused
                .as_deref()
                .and_then(|name| infos.iter().find(|o| o.name == name))
                .unwrap_or(&infos[0])
        };
        let rect = compute_image_pin_bounds(w, h, target_info, args.at);
        let caps = infos
            .into_iter()
            .map(OutputInfo::into_geometry_capture)
            .collect();
        return Ok((crate::ui::pin::PinSpec { w, h, rgba, rect }, caps));
    }

    let target = args
        .capture_target()
        .context("missing pin source (expected an image path or a capture target flag)")?;
    sleep_delay(args.delay);
    let snapshot = if target.uses_compositor_ipc() {
        windowsnap::query_snapshot().unwrap_or_default()
    } else {
        CompositorSnapshot::default()
    };
    let caps = capture::capture_all_outputs()?;
    let fallback_host = caps[0].output_name.clone();
    let arc_caps: Vec<Arc<Capture>> = caps.into_iter().map(Arc::new).collect();
    let mut session = ScreenshotSession::new(arc_caps.clone(), snapshot.rects.clone());
    apply_target(&mut session, &target, &snapshot)?;
    let host = session
        .active_output_name()
        .unwrap_or(&fallback_host)
        .to_owned();
    let mut crop = session
        .crop_for_pin(&host)
        .context("capture produced an empty pin crop")?;
    if let Some(at) = args.at {
        crop.bounds.origin = point(px(at.x), px(at.y));
    }
    // Strip full-screen pixel buffers before keeping geometry shells for the
    // long-lived pin owner — only the cropped pin pixels stay resident.
    let geometry_caps = arc_caps
        .into_iter()
        .map(|c| Capture {
            output_name: c.output_name.clone(),
            logical_pos: c.logical_pos,
            logical_size: c.logical_size,
            scale: c.scale,
            transform: c.transform,
            width: c.width,
            height: c.height,
            rgba: Vec::new(),
        })
        .collect();
    drop(session);
    Ok((
        crate::ui::pin::PinSpec {
            w: crop.width,
            h: crop.height,
            rgba: crop.rgba,
            rect: crop.bounds,
        },
        geometry_caps,
    ))
}

/// Compute initial desktop logical bounds for a local image pinned onto `output`.
/// Images smaller than 85% of the output display at crisp 1:1 device pixels;
/// oversized images scale down proportionally so the whole image fits on-screen
/// (scroll-zooming still accesses the full-resolution raster).
pub fn compute_image_pin_bounds(
    img_w: u32,
    img_h: u32,
    output: &OutputInfo,
    at: Option<PointSpec>,
) -> Bounds<Pixels> {
    let scale = output.effective_scale();
    let out_w = (output.logical_size.0.max(1)) as f32;
    let out_h = (output.logical_size.1.max(1)) as f32;
    let base_w = (img_w.max(1) as f32 / scale).max(1.);
    let base_h = (img_h.max(1) as f32 / scale).max(1.);
    let max_w = (out_w * 0.85).max(1.);
    let max_h = (out_h * 0.85).max(1.);
    let fit = (max_w / base_w).min(max_h / base_h).min(1.0);
    let pin_w = (base_w * fit).round().max(1.);
    let pin_h = (base_h * fit).round().max(1.);
    let origin = match at {
        Some(p) => point(px(p.x), px(p.y)),
        None => point(
            px(output.logical_pos.0 as f32 + ((out_w - pin_w) * 0.5).round()),
            px(output.logical_pos.1 as f32 + ((out_h - pin_h) * 0.5).round()),
        ),
    };
    Bounds::new(origin, size(px(pin_w), px(pin_h)))
}

/// `shotori outputs`: list connected Wayland outputs in plain text or JSON.
pub fn run_outputs(args: &OutputsArgs) -> i32 {
    let outputs = match capture::probe_outputs() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[shotori] could not query Wayland outputs: {e:#}");
            return 1;
        }
    };
    let focused = windowsnap::query_snapshot().and_then(|s| s.focused_output);
    let text = format_outputs(&outputs, focused.as_deref(), args.json);
    let mut out = std::io::stdout().lock();
    if let Err(e) = out
        .write_all(text.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
    {
        eprintln!("[shotori] stdout: {e}");
        return 1;
    }
    0
}

/// Format probed output metadata as human-readable rows or pretty JSON.
pub fn format_outputs(outputs: &[OutputInfo], focused: Option<&str>, json: bool) -> String {
    if json {
        let items: Vec<serde_json::Value> = outputs
            .iter()
            .map(|o| {
                let is_focused = focused == Some(o.name.as_str());
                let scale = (o.effective_scale() * 100.0).round() / 100.0;
                let refresh_hz = o.refresh_mhz.map(|mhz| (mhz as f64 / 10.0).round() / 100.0);
                serde_json::json!({
                    "name": o.name,
                    "focused": is_focused,
                    "make": o.make,
                    "model": o.model,
                    "description": o.description,
                    "physical_size": {
                        "width": o.physical_size.0,
                        "height": o.physical_size.1,
                    },
                    "logical_pos": {
                        "x": o.logical_pos.0,
                        "y": o.logical_pos.1,
                    },
                    "logical_size": {
                        "width": o.logical_size.0,
                        "height": o.logical_size.1,
                    },
                    "scale": scale,
                    "wl_output_scale": o.wl_scale as i32,
                    "transform": o.transform.as_str(),
                    "refresh_hz": refresh_hz,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into());
    }

    let name_w = outputs.iter().map(|o| o.name.len()).max().unwrap_or(4);
    outputs
        .iter()
        .map(|o| {
            let marker = if focused == Some(o.name.as_str()) {
                '*'
            } else {
                ' '
            };
            let phys = if o.physical_size.0 > 0 && o.physical_size.1 > 0 {
                format!("{}x{}", o.physical_size.0, o.physical_size.1)
            } else {
                "?x?".to_owned()
            };
            let (lx, ly) = o.logical_pos;
            let (lw, lh) = o.logical_size;
            let scale = o.effective_scale();
            let scale_num = format!("{scale:.2}");
            let scale_str = format!(
                "{}x",
                scale_num.trim_end_matches('0').trim_end_matches('.')
            );
            let refresh = o
                .refresh_mhz
                .map(|m| format!("  {:.0}Hz", m as f64 / 1000.0))
                .unwrap_or_default();
            let label = if !o.description.is_empty() {
                format!("  ({})", o.description)
            } else if !o.make.is_empty() || !o.model.is_empty() {
                format!("  ({} {})", o.make, o.model).replace("  ", " ")
            } else {
                String::new()
            };
            format!(
                "{:<name_w$} {marker}  {phys:<10}  logical {lw}x{lh}{lx:+}{ly:+}  scale {scale_str}  {}{refresh}{label}",
                o.name,
                o.transform.as_str(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn load_rgba_path(path: &Path) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let bytes = if path == Path::new("-") {
        let mut buf = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut buf)
            .context("reading image from stdin")?;
        buf
    } else {
        std::fs::read(path).with_context(|| format!("reading {}", path.display()))?
    };
    decode_rgba_bytes(&bytes).with_context(|| format!("decoding image {}", path.display()))
}

fn decode_rgba_bytes(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let rgba = image::load_from_memory(bytes)
        .context("unsupported or corrupt image data")?
        .into_rgba8();
    let (w, h) = rgba.dimensions();
    anyhow::ensure!(w > 0 && h > 0, "image has zero dimensions");
    Ok((w, h, rgba.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::{
        TargetError, apply_target, compute_image_pin_bounds, decode_rgba_bytes, format_outputs,
    };
    use crate::args::{PointSpec, RegionSpec, SelectionTarget};
    use crate::model::session::ScreenshotSession;
    use crate::platform::capture::{Capture, OutputInfo, Transform};
    use crate::platform::windowsnap::{CompositorSnapshot, SnapRect};
    use gpui_kit::{Bounds, point, px, size};
    use std::sync::Arc;

    fn screen(name: &str, pos: (i32, i32), scale: f32) -> Arc<Capture> {
        let mut cap = Capture::for_test(pos, scale);
        cap.output_name = name.into();
        cap.width = (200. * scale) as u32;
        cap.height = (100. * scale) as u32;
        cap.rgba = vec![128; (cap.width * cap.height * 4) as usize];
        Arc::new(cap)
    }

    fn dual_session() -> ScreenshotSession {
        ScreenshotSession::new(
            vec![screen("DP-1", (0, 0), 1.), screen("eDP-1", (200, 0), 2.)],
            Vec::new(),
        )
    }

    #[test]
    fn apply_target_resolves_focused_screen_output_region_and_window() {
        let snap = CompositorSnapshot {
            rects: vec![SnapRect::from_xywh(
                220.,
                10.,
                120.,
                60.,
                "kitty".into(),
                true,
                1,
            )],
            focused_output: Some("eDP-1".into()),
        };

        let mut s = dual_session();
        apply_target(&mut s, &SelectionTarget::FocusedScreen, &snap).unwrap();
        assert_eq!(s.active_output_name(), Some("eDP-1"));
        assert_eq!(
            s.selection().bounds(),
            Some(Bounds::new(
                point(px(200.), px(0.)),
                size(px(200.), px(100.))
            ))
        );

        let mut s = dual_session();
        apply_target(&mut s, &SelectionTarget::FocusedWindow, &snap).unwrap();
        assert_eq!(s.active_output_name(), Some("eDP-1"));
        assert_eq!(
            s.selection().bounds(),
            Some(Bounds::new(
                point(px(220.), px(10.)),
                size(px(120.), px(60.))
            ))
        );

        let mut s = dual_session();
        assert_eq!(
            apply_target(&mut s, &SelectionTarget::Output("HDMI-9".into()), &snap),
            Err(TargetError::UnknownOutput {
                requested: "HDMI-9".into(),
                available: vec!["DP-1".into(), "eDP-1".into()],
            })
        );

        let mut s = dual_session();
        assert!(matches!(
            apply_target(
                &mut s,
                &SelectionTarget::Region(RegionSpec {
                    x: 9000.,
                    y: 9000.,
                    width: 50.,
                    height: 50.,
                }),
                &snap,
            ),
            Err(TargetError::OutOfBounds { .. })
        ));

        let mut s = dual_session();
        assert_eq!(
            apply_target(
                &mut s,
                &SelectionTarget::FocusedWindow,
                &CompositorSnapshot::default()
            ),
            Err(TargetError::NoFocusedWindow)
        );
    }

    #[test]
    fn image_pin_bounds_centers_at_native_dpi_and_caps_oversized_images() {
        let info = OutputInfo {
            name: "DP-1".into(),
            description: "Dell U2723QE".into(),
            make: "Dell".into(),
            model: "U2723QE".into(),
            logical_pos: (1920, 0),
            physical_size: (3840, 2160),
            logical_size: (1920, 1080),
            wl_scale: 2.0,
            transform: Transform::Normal,
            refresh_mhz: Some(60_000),
        };
        // 400x200 physical px on a 2x monitor → 200x100 logical px, centered
        let small = compute_image_pin_bounds(400, 200, &info, None);
        assert_eq!(
            small,
            Bounds::new(point(px(1920. + 860.), px(490.)), size(px(200.), px(100.)))
        );

        // Explicit --at overrides origin while preserving DPI-scaled size
        let placed = compute_image_pin_bounds(400, 200, &info, Some(PointSpec { x: -50., y: 30. }));
        assert_eq!(
            placed,
            Bounds::new(point(px(-50.), px(30.)), size(px(200.), px(100.)))
        );

        // Oversized 8000x6000 image fits inside 85% of 1920x1080
        let huge = compute_image_pin_bounds(8000, 6000, &info, None);
        assert!(f32::from(huge.size.width) <= 1920. * 0.85 + 1.0);
        assert!(f32::from(huge.size.height) <= 1080. * 0.85 + 1.0);
    }

    #[test]
    fn format_outputs_renders_table_and_json() {
        let outs = [
            OutputInfo {
                name: "DP-1".into(),
                description: "Primary".into(),
                make: "Dell".into(),
                model: "P27".into(),
                logical_pos: (0, 0),
                physical_size: (3840, 2160),
                logical_size: (2560, 1440),
                wl_scale: 2.0,
                transform: Transform::Normal,
                refresh_mhz: Some(144_000),
            },
            OutputInfo {
                name: "DP-2".into(),
                description: String::new(),
                make: String::new(),
                model: String::new(),
                logical_pos: (-720, -100),
                physical_size: (1080, 1920),
                logical_size: (720, 1280),
                wl_scale: 2.0,
                transform: Transform::Rot90,
                refresh_mhz: None,
            },
        ];
        let plain = format_outputs(&outs, Some("DP-1"), false);
        assert!(plain.contains("DP-1 *"));
        assert!(plain.contains("logical 2560x1440+0+0"));
        assert!(plain.contains("scale 1.5x"));
        assert!(plain.contains("logical 720x1280-720-100"));
        assert!(plain.contains("90"));

        let js: serde_json::Value =
            serde_json::from_str(&format_outputs(&outs, Some("DP-1"), true)).unwrap();
        assert_eq!(js[0]["name"], "DP-1");
        assert_eq!(js[0]["focused"], true);
        assert_eq!(js[0]["scale"], 1.5);
        assert_eq!(js[1]["focused"], false);
        assert_eq!(js[1]["transform"], "90");
    }

    #[test]
    fn decode_rgba_bytes_roundtrips_encoded_png() {
        let src = vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 0, 255,
        ];
        let png = crate::model::export::encode_png_fast(2, 2, &src).unwrap();
        let (w, h, decoded) = decode_rgba_bytes(&png).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(decoded, src);
    }
}
