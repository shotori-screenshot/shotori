//! # Custom action execution engine
//!
//! Runs a user-configured [`CustomAction`] in a child shell (`/bin/sh -c`)
//! with a standardized Unix I/O protocol:
//! - **Inputs**: temporary image file (`SHOTORI_IMAGE_PATH`), raw image bytes
//!   on stdin, OCR text (`SHOTORI_TEXT_PATH` + stdin), or metadata-only.
//! - **Metadata env vars**: `SHOTORI_WIDTH`, `SHOTORI_HEIGHT`,
//!   `SHOTORI_X`/`Y`/`W`/`H`, `SHOTORI_OUTPUT`, `SHOTORI_FORMAT`,
//!   `SHOTORI_ANNOTATED`, `SHOTORI_ACTION_ID`, `SHOTORI_ACTION_NAME`.
//! - **Outputs**: `copy_text`, `copy_image`, `pin`, `notify`, `json`, or
//!   `ignore`, with bounded pipe capture and timeout termination so a
//!   runaway script can never wedge or OOM Shotori.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{Bounds, Pixels};
use serde::Deserialize;

use crate::model::export;
use crate::model::settings::{CustomAction, CustomActionInput, CustomActionOutput, ExportFormat};

/// Upper bound on captured stdout bytes (64 MiB — plenty for a 4K PNG).
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
/// Upper bound on captured stderr bytes (64 KiB — only error summaries are read).
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Crop + geometry snapshot handed from the overlay to the custom action runner.
pub struct ActionPayload {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub bounds_logical: Bounds<Pixels>,
    pub output_name: String,
    pub annotated: bool,
}

/// Optional structured JSON response printed to stdout when `output = "json"`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct JsonResponse {
    pub clipboard_text: Option<String>,
    pub clipboard_image_path: Option<String>,
    pub pin_image_path: Option<String>,
    pub notify_title: Option<String>,
    pub notify_body: Option<String>,
    pub open_url: Option<String>,
}

/// Result handed back to the UI thread after background execution completes.
#[derive(Debug)]
pub enum ActionOutcome {
    /// Action completed and all side effects (clipboard / notification) were delivered.
    Done,
    /// Action produced an image that should be opened as a floating desktop Pin.
    PinImage {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// Action failed (spawn error, non-zero exit, timeout, or invalid output).
    Failed { title: String, reason: String },
}

/// Execute `action` synchronously (call from a background thread / executor).
pub fn execute(action: &CustomAction, payload: ActionPayload) -> ActionOutcome {
    let exec_result = run_process(action, &payload);
    let output = match exec_result {
        Ok(out) => out,
        Err(err) => {
            eprintln!("[shotori] custom action {:?} failed: {err:#}", action.id);
            return ActionOutcome::Failed {
                title: crate::i18n::t!(
                    "notify.custom_action.failed.title",
                    name = action.name.as_str()
                )
                .into_owned(),
                reason: crate::notify::short_error_reason(&err),
            };
        }
    };

    match apply_output(action, &output) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!(
                "[shotori] custom action {:?} output handling failed: {err:#}",
                action.id
            );
            ActionOutcome::Failed {
                title: crate::i18n::t!(
                    "notify.custom_action.failed.title",
                    name = action.name.as_str()
                )
                .into_owned(),
                reason: crate::notify::short_error_reason(&err),
            }
        }
    }
}

/// Raw stdout bytes from a successful (exit status 0) command run.
struct CommandOutput {
    stdout: Vec<u8>,
}

fn run_process(action: &CustomAction, payload: &ActionPayload) -> anyhow::Result<CommandOutput> {
    let export_cfg = crate::model::settings::current().export();
    let format = export_cfg.format;
    let ext = format.extension();

    // Hold temp files until the child finishes so they are always cleaned up on drop.
    let mut _temp_image: Option<tempfile::NamedTempFile> = None;
    let mut _temp_text: Option<tempfile::NamedTempFile> = None;
    let mut stdin_bytes: Option<Vec<u8>> = None;
    let mut image_path_env: Option<PathBuf> = None;
    let mut text_path_env: Option<PathBuf> = None;

    match action.input {
        CustomActionInput::File => {
            let encoded = export::encode_image(
                payload.width,
                payload.height,
                &payload.rgba,
                format,
                export_cfg.png_compression,
                export_cfg.jpeg_quality,
            )
            .context("encoding selection image")?;
            let mut file = tempfile::Builder::new()
                .prefix("shotori-action-")
                .suffix(&format!(".{ext}"))
                .tempfile()
                .context("creating temporary image file")?;
            file.write_all(&encoded)
                .context("writing temporary image file")?;
            file.flush().context("flushing temporary image file")?;
            image_path_env = Some(file.path().to_path_buf());
            _temp_image = Some(file);
        }
        CustomActionInput::Stdin => {
            let encoded = export::encode_image(
                payload.width,
                payload.height,
                &payload.rgba,
                format,
                export_cfg.png_compression,
                export_cfg.jpeg_quality,
            )
            .context("encoding selection image for stdin")?;
            stdin_bytes = Some(encoded);
        }
        CustomActionInput::OcrText => {
            if crate::ocr::models_missing() {
                anyhow::bail!("OCR models are not downloaded yet — run OCR once first");
            }
            let text = crate::ocr::run_ocr(&payload.rgba, payload.width, payload.height)
                .context("running OCR before custom action")?;
            let mut file = tempfile::Builder::new()
                .prefix("shotori-ocr-")
                .suffix(".txt")
                .tempfile()
                .context("creating temporary OCR text file")?;
            file.write_all(text.as_bytes())
                .context("writing temporary OCR text file")?;
            file.flush().context("flushing temporary OCR text file")?;
            text_path_env = Some(file.path().to_path_buf());
            stdin_bytes = Some(text.into_bytes());
            _temp_text = Some(file);
        }
        CustomActionInput::None => {}
    }

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(&action.command);
    if let Some(dir) = action.resolved_working_dir()
        && dir.is_dir()
    {
        cmd.current_dir(dir);
    }

    cmd.env("SHOTORI_ACTION_ID", &action.id)
        .env("SHOTORI_ACTION_NAME", &action.name)
        .env("SHOTORI_WIDTH", payload.width.to_string())
        .env("SHOTORI_HEIGHT", payload.height.to_string())
        .env(
            "SHOTORI_X",
            f32::from(payload.bounds_logical.origin.x)
                .round()
                .to_string(),
        )
        .env(
            "SHOTORI_Y",
            f32::from(payload.bounds_logical.origin.y)
                .round()
                .to_string(),
        )
        .env(
            "SHOTORI_W",
            f32::from(payload.bounds_logical.size.width)
                .round()
                .to_string(),
        )
        .env(
            "SHOTORI_H",
            f32::from(payload.bounds_logical.size.height)
                .round()
                .to_string(),
        )
        .env("SHOTORI_OUTPUT", &payload.output_name)
        .env("SHOTORI_FORMAT", ext)
        .env(
            "SHOTORI_ANNOTATED",
            if payload.annotated { "1" } else { "0" },
        );

    if let Some(path) = &image_path_env {
        cmd.env("SHOTORI_IMAGE_PATH", path);
    }
    if let Some(path) = &text_path_env {
        cmd.env("SHOTORI_TEXT_PATH", path);
    }

    cmd.stdin(if stdin_bytes.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    // Run `/bin/sh` in its own process group so timeout termination kills
    // forked grandchildren too (e.g. Ubuntu's `dash`, which forks `sleep`
    // instead of exec-optimizing it, leaving pipe write ends open if only
    // the shell PID is killed).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `/bin/sh -c` for custom action {:?}", action.id))?;

    // Drain stdin, stdout, and stderr on helper threads so large payloads
    // never deadlock on full OS pipe buffers.
    let stdin_handle = stdin_bytes.and_then(|bytes| {
        child.stdin.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
                drop(pipe);
            })
        })
    });
    let stdout_handle = child
        .stdout
        .take()
        .map(|pipe| std::thread::spawn(move || read_bounded(pipe, MAX_STDOUT_BYTES)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_bounded(pipe, MAX_STDERR_BYTES)));

    let timeout = Duration::from_secs(u64::from(action.timeout_secs.max(1)));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().context("polling custom action child")? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                #[cfg(target_os = "linux")]
                if let Some(pgid) = rustix::process::Pid::from_raw(child.id() as i32)
                    && let Err(err) =
                        rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL)
                    && err != rustix::io::Errno::SRCH
                {
                    eprintln!("[shotori] custom action: killpg failed: {err}");
                }
                drop(child.kill());
                drop(child.wait());
                // Detach pipe-draining threads instead of blocking on them:
                // the process-group SIGKILL closes normal child pipe ends
                // immediately, and not joining ensures even a grandchild that
                // called setsid can never wedge the timeout path.
                drop(stdin_handle);
                drop(stdout_handle);
                drop(stderr_handle);
                anyhow::bail!("timed out after {}s", action.timeout_secs);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };

    if let Some(h) = stdin_handle {
        drop(h.join());
    }
    let stdout = stdout_handle
        .and_then(|h| h.join().ok())
        .transpose()?
        .unwrap_or_default();
    let stderr = stderr_handle
        .and_then(|h| h.join().ok())
        .transpose()?
        .unwrap_or_default();

    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr);
        let summary = first_meaningful_line(&stderr_text);
        if summary.is_empty() {
            anyhow::bail!("command exited with {status}");
        } else {
            anyhow::bail!("{summary} ({status})");
        }
    }

    Ok(CommandOutput { stdout })
}

fn read_bounded<R: Read>(mut reader: R, cap: usize) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        let room = cap.saturating_sub(buf.len());
        if room > 0 {
            buf.extend_from_slice(&chunk[..n.min(room)]);
        }
        // Keep draining until EOF so the child never blocks on SIGPIPE/EAGAIN.
    }
    Ok(buf)
}

fn first_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Detect whether `text` is a standalone HTTP(S) URL suitable for `xdg-open`.
pub fn detect_single_url(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && !trimmed.chars().any(char::is_whitespace)
    {
        Some(trimmed)
    } else {
        None
    }
}

/// Decode arbitrary PNG/JPEG/WebP/etc. bytes into tightly packed RGBA8 pixels.
fn decode_image_bytes(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    anyhow::ensure!(!bytes.is_empty(), "command produced empty image output");
    let img = image::load_from_memory(bytes).context("decoding image output")?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    anyhow::ensure!(w > 0 && h > 0, "decoded image has zero dimensions");
    Ok((w, h, rgba.into_raw()))
}

fn apply_output(action: &CustomAction, output: &CommandOutput) -> anyhow::Result<ActionOutcome> {
    match action.output {
        CustomActionOutput::Ignore => Ok(ActionOutcome::Done),
        CustomActionOutput::CopyText => {
            let text = String::from_utf8(output.stdout.clone())
                .context("command stdout is not valid UTF-8")?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                crate::notify::custom_message(
                    &action.name,
                    crate::i18n::t!("notify.custom_action.empty_output").as_ref(),
                    None,
                );
                return Ok(ActionOutcome::Done);
            }
            crate::clipboard::copy_text(trimmed.to_owned())
                .context("copying custom action stdout to clipboard")?;
            let url = detect_single_url(trimmed);
            crate::notify::custom_text_copied(&action.name, trimmed, url);
            Ok(ActionOutcome::Done)
        }
        CustomActionOutput::CopyImage => {
            let (w, h, rgba) = decode_image_bytes(&output.stdout)?;
            let png = export::encode_png_fast(w, h, &rgba)?;
            crate::clipboard::copy_image(w, h, &rgba, &png)
                .context("copying custom action image to clipboard")?;
            crate::notify::copied(&png);
            Ok(ActionOutcome::Done)
        }
        CustomActionOutput::Pin => {
            let (w, h, rgba) = decode_image_bytes(&output.stdout)?;
            Ok(ActionOutcome::PinImage {
                width: w,
                height: h,
                rgba,
            })
        }
        CustomActionOutput::Notify => {
            let text = String::from_utf8(output.stdout.clone())
                .context("command stdout is not valid UTF-8")?;
            let trimmed = text.trim();
            let body = if trimmed.is_empty() {
                crate::i18n::t!("notify.custom_action.completed").into_owned()
            } else {
                crate::notify::preview_snippet(trimmed)
            };
            let url = detect_single_url(trimmed);
            crate::notify::custom_message(&action.name, &body, url);
            Ok(ActionOutcome::Done)
        }
        CustomActionOutput::Json => {
            let text = String::from_utf8(output.stdout.clone())
                .context("command JSON stdout is not valid UTF-8")?;
            let resp: JsonResponse = serde_json::from_str(text.trim())
                .context("parsing JSON response from custom action stdout")?;
            apply_json_response(action, resp)
        }
    }
}

fn apply_json_response(action: &CustomAction, resp: JsonResponse) -> anyhow::Result<ActionOutcome> {
    if let Some(text) = resp
        .clipboard_text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        crate::clipboard::copy_text(text.to_owned())
            .context("copying JSON clipboard_text to clipboard")?;
    }
    if let Some(img_path) = resp
        .clipboard_image_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let bytes = std::fs::read(img_path)
            .with_context(|| format!("reading clipboard_image_path {img_path:?}"))?;
        let (w, h, rgba) = decode_image_bytes(&bytes)?;
        let png = export::encode_png_fast(w, h, &rgba)?;
        crate::clipboard::copy_image(w, h, &rgba, &png)
            .context("copying JSON clipboard_image_path to clipboard")?;
    }
    let pin_outcome = if let Some(pin_path) = resp
        .pin_image_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let bytes = std::fs::read(pin_path)
            .with_context(|| format!("reading pin_image_path {pin_path:?}"))?;
        let (w, h, rgba) = decode_image_bytes(&bytes)?;
        Some(ActionOutcome::PinImage {
            width: w,
            height: h,
            rgba,
        })
    } else {
        None
    };

    let open_url = resp
        .open_url
        .as_deref()
        .and_then(detect_single_url)
        .or_else(|| resp.clipboard_text.as_deref().and_then(detect_single_url));
    let title = resp
        .notify_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&action.name);
    let body = resp
        .notify_body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            resp.clipboard_text
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(crate::notify::preview_snippet)
        });
    if let Some(body) = body {
        crate::notify::custom_message(title, &body, open_url);
    }

    Ok(pin_outcome.unwrap_or(ActionOutcome::Done))
}

/// Extension helper to keep format naming consistent with `[export] format`.
#[allow(dead_code)]
fn _assert_export_format(_: ExportFormat) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings::{CustomActionOrigin, CustomActionSource};
    use gpui_kit::{point, px, size};

    fn synthetic_payload() -> ActionPayload {
        // 2x2 solid red RGBA8
        let rgba = vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ];
        ActionPayload {
            width: 2,
            height: 2,
            rgba,
            bounds_logical: Bounds::new(point(px(10.), px(20.)), size(px(100.), px(50.))),
            output_name: "DP-1".into(),
            annotated: true,
        }
    }

    fn test_action(
        command: &str,
        input: CustomActionInput,
        output: CustomActionOutput,
    ) -> CustomAction {
        CustomAction {
            id: "test-act".into(),
            name: "Test Action".into(),
            icon: "sparkles".into(),
            command: command.into(),
            working_dir: None,
            input,
            source: CustomActionSource::Annotated,
            output,
            close_overlay: true,
            timeout_secs: 5,
            shortcut: None,
            origin: CustomActionOrigin::Settings,
        }
    }

    #[test]
    fn file_input_injects_metadata_env_vars_and_cleans_temp_file() {
        let out_dir = tempfile::tempdir().unwrap();
        let report_file = out_dir.path().join("report.txt");
        let cmd = format!(
            "printf '%s|%s|%s|%s|%s|%s' \"$SHOTORI_IMAGE_PATH\" \"$SHOTORI_WIDTH\" \"$SHOTORI_HEIGHT\" \"$SHOTORI_OUTPUT\" \"$SHOTORI_ANNOTATED\" \"$SHOTORI_X\" > {:?} && test -s \"$SHOTORI_IMAGE_PATH\"",
            report_file
        );
        let action = test_action(&cmd, CustomActionInput::File, CustomActionOutput::Ignore);
        let outcome = execute(&action, synthetic_payload());
        assert!(matches!(outcome, ActionOutcome::Done), "{outcome:?}");

        let report = std::fs::read_to_string(&report_file).unwrap();
        let parts: Vec<&str> = report.split('|').collect();
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[1], "2");
        assert_eq!(parts[2], "2");
        assert_eq!(parts[3], "DP-1");
        assert_eq!(parts[4], "1");
        assert_eq!(parts[5], "10");
        assert!(
            !std::path::Path::new(parts[0]).exists(),
            "temporary image file must be cleaned up after action finishes"
        );
    }

    #[test]
    fn stdin_input_to_pin_output_round_trips_image_bytes() {
        // Echo stdin image bytes straight back to stdout → PinImage
        let action = test_action("cat", CustomActionInput::Stdin, CustomActionOutput::Pin);
        let outcome = execute(&action, synthetic_payload());
        match outcome {
            ActionOutcome::PinImage {
                width,
                height,
                rgba,
            } => {
                assert_eq!((width, height), (2, 2));
                assert_eq!(rgba.len(), 16);
                assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
            }
            other => panic!("expected PinImage, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_exit_reports_first_stderr_line() {
        let action = test_action(
            "echo 'upload token expired' >&2; exit 2",
            CustomActionInput::None,
            CustomActionOutput::Ignore,
        );
        match execute(&action, synthetic_payload()) {
            ActionOutcome::Failed { reason, .. } => {
                assert!(
                    reason.contains("upload token expired"),
                    "stderr reason missing: {reason}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn timeout_kills_hung_command() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        // Compound command defeats bash's single-command exec optimization,
        // forcing a forked grandchild just like Ubuntu's dash does.
        let cmd = format!("sh -c 'echo $$ > {:?}; exec sleep 10'; echo done", pid_file);
        let mut action = test_action(&cmd, CustomActionInput::None, CustomActionOutput::Ignore);
        action.timeout_secs = 1;
        let started = Instant::now();
        match execute(&action, synthetic_payload()) {
            ActionOutcome::Failed { reason, .. } => {
                assert!(reason.contains("timed out"), "unexpected reason: {reason}");
                assert!(
                    started.elapsed() < Duration::from_secs(4),
                    "timeout took too long: {:?}",
                    started.elapsed()
                );
            }
            other => panic!("expected timeout failure, got {other:?}"),
        }
        // Verify the forked grandchild was killed along with the process group.
        #[cfg(target_os = "linux")]
        {
            let text = std::fs::read_to_string(&pid_file)
                .expect("grandchild pid file written before 1s timeout");
            let raw: i32 = text.trim().parse().expect("valid grandchild pid");
            let pid = rustix::process::Pid::from_raw(raw).expect("non-zero pid");
            let mut alive = true;
            for _ in 0..20 {
                if rustix::process::test_kill_process(pid).is_err() {
                    alive = false;
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(!alive, "grandchild pid {raw} survived timeout kill");
        }
    }

    #[test]
    fn json_response_parses_and_handles_pin_image_path() {
        let dir = tempfile::tempdir().unwrap();
        let png_path = dir.path().join("out.png");
        let payload = synthetic_payload();
        export::save_png(&png_path, payload.width, payload.height, &payload.rgba).unwrap();

        let cmd = format!(
            "printf '{{\"pin_image_path\":{:?}}}'",
            png_path.to_string_lossy()
        );
        let action = test_action(&cmd, CustomActionInput::None, CustomActionOutput::Json);
        match execute(&action, payload) {
            ActionOutcome::PinImage { width, height, .. } => {
                assert_eq!((width, height), (2, 2));
            }
            other => panic!("expected PinImage from JSON, got {other:?}"),
        }
    }

    #[test]
    fn detect_single_url_accepts_http_and_rejects_multiline_or_non_url() {
        assert_eq!(
            detect_single_url("  https://cdn.example.com/a.png\n"),
            Some("https://cdn.example.com/a.png")
        );
        assert_eq!(
            detect_single_url("http://localhost:8080/img"),
            Some("http://localhost:8080/img")
        );
        assert_eq!(detect_single_url("https://a.com https://b.com"), None);
        assert_eq!(detect_single_url("![alt](https://a.com/a.png)"), None);
        assert_eq!(detect_single_url("file:///etc/passwd"), None);
    }
}
