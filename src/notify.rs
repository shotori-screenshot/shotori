//! # Desktop notifications: fire-and-forget via a detached child process
//!
//! Why a child process: shotori calls `cx.quit()` right after saving / OCR.
//! A plain background thread would be killed mid-send when the process
//! exits; a detached child (`shotori --notify <summary> <body> [image] [saved path]`,
//! same pattern as the Linux clipboard daemon) outlives the parent and always
//! delivers. Failures are silent by design — a missing notification daemon
//! must never break a screenshot tool.
//!
//! Backends: freedesktop `org.freedesktop.Notifications` over D-Bus.
//!
//! Image previews: the freedesktop `image-path` hint with a `file://`-style
//! local path (verified against noctalia). Thumbnails are written to the
//! cache dir and must outlive the notification — cleaned up lazily (24h).
//!
//! Thumbnails render INSIDE the detached child, never in the parent:
//! the parent hands over full-resolution PNG bytes (stdin for copies,
//! the saved file itself for saves) and exits; the child — a short-lived
//! process anyway — does the decode + downscale + thumbnail encode. The
//! copy path additionally caches the full-resolution PNG so the
//! notification's Open action can show it.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The argv marker for the notify child (main.rs dispatches on this)
pub const NOTIFY_ARG: &str = "--notify";

/// Image-argument marker: the child renders the thumbnail itself from
/// the full-resolution PNG — read from the saved path when one is
/// passed, otherwise from stdin. The parent does no pixel work.
pub const STDIN_IMAGE: &str = "-";

/// Max thumbnail edge (px) — enough for any daemon's rendering, tiny file
const PREVIEW_MAX: u32 = 256;
/// Notification images older than this are removed on the next copy/save
const PREVIEW_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Queue a plain notification and return immediately.
pub fn send(summary: &str, body: &str) {
    spawn_child(summary, body, None, None);
}

/// Retain the full-resolution clipboard PNG so the notification can open
/// it, and queue the notification. The parent does NO pixel work: writing
/// the cache file is a plain byte copy; the thumbnail renders in the
/// detached child from the same file (see `STDIN_IMAGE`). Cache failures
/// must not turn a successful clipboard copy into an error.
pub fn copied(png: &[u8]) {
    let path = cache_dir().and_then(|dir| match write_clipboard_image(&dir, png) {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("[shotori] could not cache copied screenshot: {error}");
            None
        }
    });
    spawn_child(
        "Screenshot copied",
        "The image is ready to paste.",
        Some(std::path::Path::new(STDIN_IMAGE)),
        path.as_deref(),
    );
}

fn write_clipboard_image(dir: &std::path::Path, png: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    cleanup_old_previews(dir);
    // Exclusive creation also handles simultaneous screenshot sessions.
    let mut file = tempfile::Builder::new()
        .prefix("clipboard-")
        .suffix(".png")
        .tempfile_in(dir)?;
    file.write_all(png)?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    Ok(path)
}

/// Saving has a separate action target: never open the temporary thumbnail.
/// The thumbnail renders in the child from the file the user just saved —
/// the parent has nothing left to do (no pixels, no copy).
pub fn saved(path: &std::path::Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    spawn_child(
        "Screenshot saved",
        &path.to_string_lossy(),
        Some(std::path::Path::new(STDIN_IMAGE)),
        Some(&path),
    );
}

fn spawn_child(
    summary: &str,
    body: &str,
    image: Option<&std::path::Path>,
    open_path: Option<&std::path::Path>,
) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut cmd = Command::new(exe);
    cmd.arg(NOTIFY_ARG).arg(summary).arg(body);
    if image.is_some() || open_path.is_some() {
        cmd.arg(image.unwrap_or_else(|| std::path::Path::new("")));
    }
    if let Some(path) = open_path {
        cmd.arg(path);
    }
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // stderr inherited: real failures stay visible in the terminal
        .spawn();
    // Detached: dropping the handle leaves the child running; the parent
    // usually exits a moment later and the OS reaps it.
}

/// Child entry point: `shotori --notify <summary> <body> [image] [saved path]`
/// → show → exit. `image` is either a ready thumbnail path or `-`
/// (`STDIN_IMAGE`), which renders the thumbnail here from the full-resolution
/// PNG at `saved path` — the parent process does no pixel work.
pub fn notify_main() -> i32 {
    let mut args = std::env::args_os().skip(2);
    let (Some(summary), Some(body)) = (args.next(), args.next()) else {
        eprintln!("[shotori] notify child: usage: --notify <summary> <body> [image] [saved path]");
        return 1;
    };
    let image = args.next().filter(|s| !s.is_empty());
    let open_path = args.next();
    let is_marker = image
        .as_deref()
        .is_some_and(|img| img == std::ffi::OsStr::new(STDIN_IMAGE));
    let rendered = if is_marker {
        open_path
            .as_deref()
            .and_then(|p| render_preview(std::path::Path::new(p)))
    } else {
        None
    };
    let image_path = match image.as_deref() {
        Some(_) if is_marker => rendered.as_deref(),
        Some(img) => Some(std::path::Path::new(img)),
        None => None,
    };
    match show(
        &summary.to_string_lossy(),
        &body.to_string_lossy(),
        image_path,
        open_path.as_deref().map(std::path::Path::new),
    ) {
        Ok(()) => 0,
        Err(e) => {
            // No daemon on the bus, disabled, … — not fatal for
            // the caller (the action already succeeded), just report it.
            eprintln!("[shotori] notification not delivered: {e}");
            1
        }
    }
}

// ── Linux: org.freedesktop.Notifications over D-Bus ──────────────────

#[cfg(target_os = "linux")]
fn show(
    summary: &str,
    body: &str,
    image: Option<&std::path::Path>,
    open_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let mut n = notify_rust::Notification::new();
    n.appname("Shotori")
        .summary(summary)
        .icon(crate::APP_ID)
        .hint(notify_rust::Hint::DesktopEntry(crate::APP_ID.into()))
        .body(&escape_markup(body))
        .timeout(notify_rust::Timeout::Milliseconds(3500));
    // An explicit app icon remains available for cargo installs too. Keep it
    // separate from image-path, which belongs to the screenshot thumbnail.
    if let Some(dir) = cache_dir() {
        match cache_app_icon(&dir) {
            Ok(path) => {
                n.icon(&path.to_string_lossy());
            }
            Err(error) => eprintln!("[shotori] could not cache app icon: {error}"),
        }
    }
    if let Some(path) = image {
        n.image_path(&format!("file://{}", path.display()));
    }
    if open_path.is_some() {
        // Clicking the notification body invokes the freedesktop
        // "default" action — the standard open gesture and the ONLY
        // control we ship. An explicit button would duplicate exactly
        // what a click already does.
        n.action("default", "View image")
            .timeout(notify_rust::Timeout::Milliseconds(10000));
    }
    let handle = n.show()?;
    if let Some(path) = open_path {
        // A watchdog bounds this child's lifetime: notification daemons
        // with persistent histories (quickshell, notification centers)
        // may never emit `NotificationClosed`, and `wait_for_action`
        // would then block forever — every screenshot would leave an
        // orphan process holding its fds and D-Bus connection (#23).
        // The action popup times out at 10s; the 12s grace lets a
        // last-moment click win the race, and any exit path reaps the
        // thread with the process. Best-effort: if the thread cannot
        // spawn, waiting on the daemon (the old behavior) is the
        // fallback.
        std::thread::Builder::new()
            .name("shotori-notify-watchdog".into())
            .spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(12));
                std::process::exit(0);
            })
            .ok();
        handle.wait_for_action(|action| {
            if action == "default"
                && let Err(error) = open_image(path)
            {
                eprintln!("[shotori] could not open saved screenshot: {error}");
                send(
                    "Couldn’t open image",
                    "Open the saved file from your file manager.",
                );
            }
        });
    }
    Ok(())
}

/// Atomic replacement prevents simultaneous notification children from reading
/// a partly written icon. This stable file is not a screenshot-cache entry.
#[cfg(target_os = "linux")]
fn cache_app_icon(dir: &std::path::Path) -> std::io::Result<PathBuf> {
    use std::io::Write;
    const PNG: &[u8] = include_bytes!("../assets/app/shotori-128.png");
    let path = dir.join("shotori-icon.png");
    if std::fs::read(&path).is_ok_and(|bytes| bytes == PNG) {
        return Ok(path);
    }
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(PNG)?;
    file.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

#[cfg(target_os = "linux")]
fn open_image(path: &std::path::Path) -> anyhow::Result<()> {
    let status = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()?;
    anyhow::ensure!(status.success(), "image opener exited with {status}");
    Ok(())
}

/// Notification daemons may parse body markup, including OCR text and filenames.
#[cfg(target_os = "linux")]
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Preview thumbnails ────────────────────────────────────────────────

fn cache_dir() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(d) => PathBuf::from(d),
        Err(_) => PathBuf::from(std::env::var("HOME").ok()?).join(".cache"),
    };
    let dir = base.join("shotori");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Child-side: load the full-resolution PNG, downscale it to a cached
/// thumbnail and return the thumbnail's path. Runs in the detached child,
/// so the parent never pays for the decode + downscale + re-encode.
fn render_preview(png_path: &std::path::Path) -> Option<PathBuf> {
    let img = image::open(png_path).ok()?.to_rgba8();
    let thumb = image::imageops::thumbnail(&img, PREVIEW_MAX, PREVIEW_MAX);
    let dir = cache_dir()?;
    cleanup_old_previews(&dir);
    let path = dir.join(format!(
        "preview-{}.png",
        chrono::Local::now().format("%Y%m%d%H%M%S%3f")
    ));
    thumb.save(&path).ok()?;
    Some(path)
}

/// Best-effort removal of previews past their TTL. The file must outlive
/// the notification that references it — 24h is orders of magnitude more
/// than any notification timeout.
fn cleanup_old_previews(dir: &std::path::Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > PREVIEW_TTL);
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_preview = name.starts_with("preview-") || name.starts_with("clipboard-");
        if stale && is_preview {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn app_icon_cache_repairs_corruption_and_survives_thumbnail_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = super::cache_app_icon(dir.path()).unwrap();
        assert_eq!(image::open(&path).unwrap().width(), 128);
        std::fs::write(&path, b"interrupted or old icon").unwrap();
        assert_eq!(super::cache_app_icon(dir.path()).unwrap(), path);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            include_bytes!("../assets/app/shotori-128.png")
        );
        let old =
            std::time::SystemTime::now() - super::PREVIEW_TTL - std::time::Duration::from_secs(60);
        std::fs::File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        super::cleanup_old_previews(dir.path());
        assert!(path.exists());
        assert!(super::cache_app_icon(&dir.path().join("missing-directory")).is_err());
    }

    #[test]
    fn cache_cleanup_removes_only_expired_notification_images() {
        let dir = tempfile::tempdir().unwrap();
        let old =
            std::time::SystemTime::now() - super::PREVIEW_TTL - std::time::Duration::from_secs(60);
        for name in ["clipboard-old.png", "preview-old.png", "saved.png"] {
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let fresh = super::write_clipboard_image(dir.path(), b"new").unwrap();
        assert!(fresh.exists());
        assert!(dir.path().join("saved.png").exists());
        assert!(!dir.path().join("clipboard-old.png").exists());
        assert!(!dir.path().join("preview-old.png").exists());
    }

    #[test]
    fn clipboard_cache_retains_full_png_and_uses_unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        let rgba = [30, 90, 180, 255].repeat(640 * 360);
        let png = crate::model::export::encode_png(640, 360, &rgba).unwrap();
        let first = super::write_clipboard_image(dir.path(), &png).unwrap();
        let second = super::write_clipboard_image(dir.path(), &png).unwrap();
        assert_ne!(first, second);
        let image = image::open(&first).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (640, 360));
        assert_eq!(image.into_raw(), rgba);
        assert_eq!(std::fs::read(second).unwrap(), png);
    }

    #[test]
    fn filenames_and_recognized_text_are_literal_notification_content() {
        assert_eq!(
            super::escape_markup("截图 <b>A&B</b>.png"),
            "截图 &lt;b&gt;A&amp;B&lt;/b&gt;.png"
        );
        assert_eq!(super::escape_markup("x > y\nA & B"), "x &gt; y\nA &amp; B");
    }
}
