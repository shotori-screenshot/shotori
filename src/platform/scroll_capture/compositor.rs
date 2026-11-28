//! Temporary compositor transparency & blur guard for long screenshots.
//!
//! Semi-transparent and blurred windows leave the stationary wallpaper or
//! underlying window visible while the foreground scrolls, which pulls the
//! stitcher toward `dy = 0` or smears background bands across the result.
//!
//! [`EffectsGuard::enter`] runs right before the seed frame is captured and
//! its [`Drop`] restores state immediately when the scroll session ends
//! (Copy, Save, Pin, Cancel, idle timeout, or failure — before a pinned
//! result keeps the process alive or a portal save dialog opens).
//!
//! Built-in `Auto` mode only touches compositors whose prior state can be
//! restored cleanly:
//! - **Hyprland** (`HYPRLAND_INSTANCE_SIGNATURE`): queries `j/getoption` for
//!   `decoration:blur:enabled` and `decoration:{active,inactive,fullscreen}_opacity`,
//!   sets only the modified options to opaque/unblurred, and restores the exact
//!   original values on drop.
//! - **niri** (`NIRI_SOCKET`): toggles `ToggleWindowRuleOpacity` on window(s)
//!   overlapping the capture rect (falling back to unpositioned active-workspace
//!   windows on niri ≤ 26.04) and toggles the exact same window IDs back on drop.
//! - **KDE KWin** (session D-Bus `org.kde.KWin /Effects`): if `blur` is loaded,
//!   unloads it for the capture and reloads it on drop.
//!
//! Compositors without a queryable/reversible IPC (e.g. SwayFX) and client-side
//! transparency (e.g. terminal self-rendered alpha) are handled via `Custom`
//! enter/exit shell commands.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use niri_ipc::{Action, Output, Reply, Request, Response, Window, Workspace};
use serde_json::Value;

use super::ScrollEffectsConfig;
use crate::model::session::ScrollRect;
use crate::model::settings::ScrollTransparencyMode;

/// Hard wall-clock cap for custom shell enter/exit commands so a hung user
/// hook never freezes scroll capture startup or exit.
const CUSTOM_CMD_TIMEOUT: Duration = Duration::from_secs(3);

/// Hard cap for KWin D-Bus calls (matches `windowsnap`'s 1 s IPC ceiling).
const DBUS_TIMEOUT: Duration = Duration::from_secs(1);

/// Environment variables passed to custom enter/exit shell commands.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandEnv {
    output: String,
    rect: ScrollRect,
}

impl CommandEnv {
    fn apply(&self, cmd: &mut Command) {
        cmd.env("SHOTORI_SCROLL_OUTPUT", &self.output)
            .env("SHOTORI_SCROLL_X", self.rect.x.to_string())
            .env("SHOTORI_SCROLL_Y", self.rect.y.to_string())
            .env("SHOTORI_SCROLL_W", self.rect.width.to_string())
            .env("SHOTORI_SCROLL_H", self.rect.height.to_string());
    }
}

/// Name of the detected built-in compositor supported by `Auto` mode, if any.
/// Used by the Settings UI to tell the user whether their current desktop is
/// handled automatically or needs `Custom` commands.
pub fn detected_builtin_compositor() -> Option<&'static str> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return Some("Hyprland");
    }
    if std::env::var_os(niri_ipc::socket::SOCKET_PATH_ENV).is_some() {
        return Some("niri");
    }
    if is_kde_session_env() {
        return Some("KDE KWin");
    }
    None
}

fn is_kde_session_env() -> bool {
    is_kde_session(
        std::env::var_os("KDE_FULL_SESSION").is_some(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        std::env::var("XDG_SESSION_DESKTOP").ok().as_deref(),
    )
}

fn is_kde_session(
    kde_full_session: bool,
    current_desktop: Option<&str>,
    session_desktop: Option<&str>,
) -> bool {
    if kde_full_session {
        return true;
    }
    let mentions_kde = |s: &str| {
        let lower = s.to_ascii_lowercase();
        lower.contains("kde") || lower.contains("plasma")
    };
    current_desktop.is_some_and(mentions_kde) || session_desktop.is_some_and(mentions_kde)
}

/// What [`EffectsGuard::restore`] must undo when the scroll session ends.
enum RestoreAction {
    Hyprland(Vec<(&'static str, String)>),
    Niri(Vec<u64>),
    KWinBlur,
    Custom { command: String, env: CommandEnv },
}

/// RAII guard for compositor effects modified during a scroll session.
/// Dropping restores the original state at most once.
pub(super) struct EffectsGuard {
    /// True when enter actually changed compositor state or ran a start
    /// command, signalling that the engine should give the compositor a short
    /// repaint beat before grabbing the seed frame.
    entered: bool,
    restore: Option<RestoreAction>,
}

impl EffectsGuard {
    fn noop() -> Self {
        Self {
            entered: false,
            restore: None,
        }
    }

    /// True if `enter` changed compositor effects or ran an enter command.
    pub(super) fn is_active(&self) -> bool {
        self.entered
    }

    /// Apply the configured transparency/blur guard before capturing frame #1.
    /// Never fails the scroll session: IPC or command errors log to stderr and
    /// degrade to a no-op.
    pub(super) fn enter(output: &str, rect: ScrollRect, config: &ScrollEffectsConfig) -> Self {
        match config.mode {
            ScrollTransparencyMode::Off => Self::noop(),
            ScrollTransparencyMode::Auto => enter_auto(output, rect),
            ScrollTransparencyMode::Custom => enter_custom(
                output,
                rect,
                config.enter_command.as_deref(),
                config.exit_command.as_deref(),
            ),
        }
    }

    /// Undo any effects applied on [`Self::enter`]. Idempotent.
    pub(super) fn restore(&mut self) {
        let Some(action) = self.restore.take() else {
            return;
        };
        match action {
            RestoreAction::Hyprland(pairs) => restore_hyprland(&pairs),
            RestoreAction::Niri(ids) => restore_niri(&ids),
            RestoreAction::KWinBlur => restore_kwin_blur(),
            RestoreAction::Custom { command, env } => {
                run_shell_command("exit", &command, &env, CUSTOM_CMD_TIMEOUT);
            }
        }
    }
}

impl Drop for EffectsGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

// ── Auto dispatch ────────────────────────────────────────────────────

fn enter_auto(output: &str, rect: ScrollRect) -> EffectsGuard {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return enter_hyprland();
    }
    if std::env::var_os(niri_ipc::socket::SOCKET_PATH_ENV).is_some() {
        return enter_niri(output, rect);
    }
    if is_kde_session_env() {
        return enter_kwin_blur();
    }
    EffectsGuard::noop()
}

// ── Hyprland ─────────────────────────────────────────────────────────

const HYPR_BLUR: &str = "decoration:blur:enabled";
const HYPR_ACTIVE_OPACITY: &str = "decoration:active_opacity";
const HYPR_INACTIVE_OPACITY: &str = "decoration:inactive_opacity";
const HYPR_FULLSCREEN_OPACITY: &str = "decoration:fullscreen_opacity";

fn parse_hyprland_bool(json: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(json).ok()?;
    if let Some(b) = v.get("bool").and_then(Value::as_bool) {
        return Some(b);
    }
    v.get("int").and_then(Value::as_i64).map(|i| i != 0)
}

fn parse_hyprland_float(json: &str) -> Option<f64> {
    let v: Value = serde_json::from_str(json).ok()?;
    v.get("float")
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite())
}

/// Decide which Hyprland `decoration:*` options need temporarily overriding
/// and what value to restore for each. Pure, unit-tested.
fn plan_hyprland_overrides(
    blur_enabled: Option<bool>,
    active_opacity: Option<f64>,
    inactive_opacity: Option<f64>,
    fullscreen_opacity: Option<f64>,
) -> Vec<(&'static str, &'static str, String)> {
    let mut plan = Vec::new();
    if blur_enabled == Some(true) {
        plan.push((HYPR_BLUR, "false", "true".to_owned()));
    }
    for (opt, current) in [
        (HYPR_ACTIVE_OPACITY, active_opacity),
        (HYPR_INACTIVE_OPACITY, inactive_opacity),
        (HYPR_FULLSCREEN_OPACITY, fullscreen_opacity),
    ] {
        if let Some(val) = current
            && (0.0..0.9999).contains(&val)
        {
            plan.push((opt, "1.0", format!("{val:.6}")));
        }
    }
    plan
}

fn hyprland_keyword_ok(reply: &str) -> bool {
    let trimmed = reply.trim();
    !trimmed.is_empty() && !trimmed.to_ascii_lowercase().contains("err")
}

fn enter_hyprland() -> EffectsGuard {
    use crate::platform::windowsnap::hyprland::ipc_request;

    let blur = ipc_request(&format!("j/getoption {HYPR_BLUR}"))
        .as_deref()
        .and_then(parse_hyprland_bool);
    let active = ipc_request(&format!("j/getoption {HYPR_ACTIVE_OPACITY}"))
        .as_deref()
        .and_then(parse_hyprland_float);
    let inactive = ipc_request(&format!("j/getoption {HYPR_INACTIVE_OPACITY}"))
        .as_deref()
        .and_then(parse_hyprland_float);
    let fullscreen = ipc_request(&format!("j/getoption {HYPR_FULLSCREEN_OPACITY}"))
        .as_deref()
        .and_then(parse_hyprland_float);

    let plan = plan_hyprland_overrides(blur, active, inactive, fullscreen);
    if plan.is_empty() {
        return EffectsGuard::noop();
    }

    let mut applied = Vec::new();
    for (opt, disable_val, restore_val) in plan {
        match ipc_request(&format!("keyword {opt} {disable_val}")) {
            Some(reply) if hyprland_keyword_ok(&reply) => {
                applied.push((opt, restore_val));
            }
            other => {
                eprintln!(
                    "[shotori] scroll: Hyprland `keyword {opt} {disable_val}` did not succeed ({other:?})"
                );
            }
        }
    }
    if applied.is_empty() {
        return EffectsGuard::noop();
    }
    println!(
        "[shotori] scroll: temporarily disabled Hyprland transparency/blur ({} option(s))",
        applied.len()
    );
    EffectsGuard {
        entered: true,
        restore: Some(RestoreAction::Hyprland(applied)),
    }
}

fn restore_hyprland(pairs: &[(&'static str, String)]) {
    use crate::platform::windowsnap::hyprland::ipc_request;

    for (opt, orig_val) in pairs {
        if let Some(reply) = ipc_request(&format!("keyword {opt} {orig_val}")) {
            if !hyprland_keyword_ok(&reply) {
                eprintln!(
                    "[shotori] scroll: Hyprland restore `keyword {opt} {orig_val}` replied {reply:?}"
                );
            }
        } else {
            eprintln!("[shotori] scroll: Hyprland restore `keyword {opt} {orig_val}` failed");
        }
    }
}

// ── niri ─────────────────────────────────────────────────────────────

fn connect_niri() -> Option<BufReader<UnixStream>> {
    let path = std::env::var_os(niri_ipc::socket::SOCKET_PATH_ENV)?;
    let stream = UnixStream::connect(path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    Some(BufReader::new(stream))
}

fn niri_send(socket: &mut BufReader<UnixStream>, request: Request) -> Option<Reply> {
    let mut buf = serde_json::to_string(&request).ok()?;
    buf.push('\n');
    socket.get_mut().write_all(buf.as_bytes()).ok()?;
    buf.clear();
    socket.read_line(&mut buf).ok()?;
    serde_json::from_str(&buf).ok()
}

/// Choose which niri window IDs on `output_name`'s active workspace should have
/// `ToggleWindowRuleOpacity` toggled for `rect` (output-local logical px).
///
/// - When windows carry `tile_pos_in_workspace_view` (all floating windows,
///   plus visible tiled columns on niri > 26.04), any window intersecting
///   `rect` is selected.
/// - When no positioned window intersects `rect` (e.g. tiled columns on
///   niri ≤ 26.04 where `tile_pos_in_workspace_view` is `None`), falls back to
///   all unpositioned windows on `output_name`'s active workspace so side-by-side
///   or unfocused columns under the selection still turn opaque and restore
///   symmetrically.
fn select_niri_window_ids(
    windows: &[Window],
    workspaces: &[Workspace],
    outputs: &HashMap<String, Output>,
    output_name: &str,
    rect: ScrollRect,
) -> Vec<u64> {
    let active_ws_ids: Vec<u64> = workspaces
        .iter()
        .filter(|ws| ws.is_active && ws.output.as_deref() == Some(output_name))
        .map(|ws| ws.id)
        .collect();
    if active_ws_ids.is_empty() {
        return Vec::new();
    }
    let output_bounds = outputs
        .get(output_name)
        .and_then(|o| o.logical)
        .map(|l| (l.width as f64, l.height as f64));

    let rx0 = f64::from(rect.x);
    let ry0 = f64::from(rect.y);
    let rx1 = f64::from(rect.x.saturating_add(rect.width));
    let ry1 = f64::from(rect.y.saturating_add(rect.height));

    let mut intersecting = Vec::new();
    let mut unpositioned = Vec::new();

    for w in windows {
        let Some(ws_id) = w.workspace_id else {
            continue;
        };
        if !active_ws_ids.contains(&ws_id) {
            continue;
        }
        let layout = &w.layout;
        if let Some((vx, vy)) = layout.tile_pos_in_workspace_view {
            let (ww, wh) = layout.window_size;
            if ww <= 0 || wh <= 0 {
                continue;
            }
            let raw_x0 = vx + layout.window_offset_in_tile.0;
            let raw_y0 = vy + layout.window_offset_in_tile.1;
            let raw_x1 = raw_x0 + f64::from(ww);
            let raw_y1 = raw_y0 + f64::from(wh);
            let (x0, y0, x1, y1) = match output_bounds {
                Some((ow, oh)) => (
                    raw_x0.max(0.0),
                    raw_y0.max(0.0),
                    raw_x1.min(ow),
                    raw_y1.min(oh),
                ),
                None => (raw_x0, raw_y0, raw_x1, raw_y1),
            };
            if x1 > x0 && y1 > y0 && x0 < rx1 && x1 > rx0 && y0 < ry1 && y1 > ry0 {
                intersecting.push(w.id);
            }
        } else {
            unpositioned.push(w.id);
        }
    }

    if !intersecting.is_empty() {
        intersecting
    } else {
        unpositioned
    }
}

fn enter_niri(output: &str, rect: ScrollRect) -> EffectsGuard {
    let Some(mut socket) = connect_niri() else {
        return EffectsGuard::noop();
    };
    let windows = match niri_send(&mut socket, Request::Windows) {
        Some(Reply::Ok(Response::Windows(v))) => v,
        _ => return EffectsGuard::noop(),
    };
    let workspaces = match niri_send(&mut socket, Request::Workspaces) {
        Some(Reply::Ok(Response::Workspaces(v))) => v,
        _ => return EffectsGuard::noop(),
    };
    let outputs = match niri_send(&mut socket, Request::Outputs) {
        Some(Reply::Ok(Response::Outputs(v))) => v,
        _ => return EffectsGuard::noop(),
    };

    let targets = select_niri_window_ids(&windows, &workspaces, &outputs, output, rect);
    if targets.is_empty() {
        return EffectsGuard::noop();
    }

    let mut toggled = Vec::new();
    for id in targets {
        match niri_send(
            &mut socket,
            Request::Action(Action::ToggleWindowRuleOpacity { id: Some(id) }),
        ) {
            Some(Reply::Ok(_)) => toggled.push(id),
            other => {
                eprintln!(
                    "[shotori] scroll: niri ToggleWindowRuleOpacity(id={id}) failed: {other:?}"
                );
            }
        }
    }
    if toggled.is_empty() {
        return EffectsGuard::noop();
    }
    println!(
        "[shotori] scroll: toggled niri window-rule opacity for {} window(s)",
        toggled.len()
    );
    EffectsGuard {
        entered: true,
        restore: Some(RestoreAction::Niri(toggled)),
    }
}

fn restore_niri(ids: &[u64]) {
    let Some(mut socket) = connect_niri() else {
        eprintln!("[shotori] scroll: could not reconnect to niri to restore window opacity");
        return;
    };
    for &id in ids {
        if !matches!(
            niri_send(
                &mut socket,
                Request::Action(Action::ToggleWindowRuleOpacity { id: Some(id) }),
            ),
            Some(Reply::Ok(_))
        ) {
            eprintln!("[shotori] scroll: failed to restore niri opacity for window {id}");
        }
    }
}

// ── KDE KWin (session D-Bus) ─────────────────────────────────────────

fn run_with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(tx.send(f()));
    });
    rx.recv_timeout(timeout).ok()
}

fn enter_kwin_blur() -> EffectsGuard {
    let unloaded = run_with_timeout(DBUS_TIMEOUT, || -> Result<bool, zbus::Error> {
        let conn = zbus::blocking::Connection::session()?;
        let reply = conn.call_method(
            Some("org.kde.KWin"),
            "/Effects",
            Some("org.kde.kwin.Effects"),
            "isEffectLoaded",
            &("blur",),
        )?;
        let loaded: bool = reply.body().deserialize()?;
        if !loaded {
            return Ok(false);
        }
        conn.call_method(
            Some("org.kde.KWin"),
            "/Effects",
            Some("org.kde.kwin.Effects"),
            "unloadEffect",
            &("blur",),
        )?;
        Ok(true)
    });

    match unloaded {
        Some(Ok(true)) => {
            println!("[shotori] scroll: temporarily unloaded KDE KWin blur effect");
            EffectsGuard {
                entered: true,
                restore: Some(RestoreAction::KWinBlur),
            }
        }
        Some(Ok(false)) => EffectsGuard::noop(),
        Some(Err(e)) => {
            eprintln!("[shotori] scroll: KDE KWin blur check/unload failed: {e}");
            EffectsGuard::noop()
        }
        None => {
            eprintln!("[shotori] scroll: KDE KWin D-Bus call timed out");
            EffectsGuard::noop()
        }
    }
}

fn restore_kwin_blur() {
    let res = run_with_timeout(DBUS_TIMEOUT, || -> Result<(), zbus::Error> {
        let conn = zbus::blocking::Connection::session()?;
        conn.call_method(
            Some("org.kde.KWin"),
            "/Effects",
            Some("org.kde.kwin.Effects"),
            "loadEffect",
            &("blur",),
        )?;
        Ok(())
    });
    match res {
        Some(Ok(())) => {}
        Some(Err(e)) => eprintln!("[shotori] scroll: KDE KWin blur restore failed: {e}"),
        None => eprintln!("[shotori] scroll: KDE KWin blur restore timed out"),
    }
}

// ── Custom shell commands ────────────────────────────────────────────

fn enter_custom(
    output: &str,
    rect: ScrollRect,
    enter_cmd: Option<&str>,
    exit_cmd: Option<&str>,
) -> EffectsGuard {
    let env = CommandEnv {
        output: output.to_owned(),
        rect,
    };
    let enter_cmd = enter_cmd.map(str::trim).filter(|s| !s.is_empty());
    let exit_cmd = exit_cmd.map(str::trim).filter(|s| !s.is_empty());

    let entered = if let Some(cmd) = enter_cmd {
        run_shell_command("enter", cmd, &env, CUSTOM_CMD_TIMEOUT);
        true
    } else {
        false
    };
    let restore = exit_cmd.map(|cmd| RestoreAction::Custom {
        command: cmd.to_owned(),
        env,
    });
    EffectsGuard { entered, restore }
}

/// Run a user-supplied shell command via `/bin/sh -c` with a bounded timeout.
/// Returns `true` when the command exited cleanly with status 0.
fn run_shell_command(phase: &str, script: &str, env: &CommandEnv, timeout: Duration) -> bool {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    env.apply(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[shotori] scroll: failed to spawn {phase} command `{script}`: {e}");
            return false;
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return true;
                }
                let mut stderr_buf = String::new();
                if let Some(mut err) = child.stderr.take() {
                    use std::io::Read as _;
                    drop(err.read_to_string(&mut stderr_buf));
                }
                let detail = stderr_buf.trim();
                if detail.is_empty() {
                    eprintln!("[shotori] scroll: {phase} command `{script}` exited with {status}");
                } else {
                    eprintln!(
                        "[shotori] scroll: {phase} command `{script}` exited with {status}: {detail}"
                    );
                }
                return false;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    eprintln!(
                        "[shotori] scroll: {phase} command `{script}` timed out after {}ms — killing",
                        timeout.as_millis()
                    );
                    if let Some(pgid) = rustix::process::Pid::from_raw(child.id() as i32)
                        && let Err(err) =
                            rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL)
                        && err != rustix::io::Errno::SRCH
                    {
                        eprintln!("[shotori] scroll: killpg failed: {err}");
                    }
                    drop(child.kill());
                    drop(child.wait());
                    return false;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                eprintln!("[shotori] scroll: waiting for {phase} command `{script}` failed: {e}");
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_parsers_and_override_planner_handle_all_option_states() {
        assert_eq!(
            parse_hyprland_bool(r#"{"option":"decoration:blur:enabled","int":1,"str":"1"}"#),
            Some(true)
        );
        assert_eq!(
            parse_hyprland_bool(r#"{"option":"decoration:blur:enabled","int":0,"str":"0"}"#),
            Some(false)
        );
        assert_eq!(parse_hyprland_bool(r#"{"bool":true}"#), Some(true));
        assert_eq!(parse_hyprland_bool("not json"), None);

        assert_eq!(
            parse_hyprland_float(r#"{"option":"decoration:active_opacity","float":0.85}"#),
            Some(0.85)
        );
        assert_eq!(parse_hyprland_float(r#"{"float":"bad"}"#), None);

        // Already opaque and blur off → no overrides needed.
        assert!(plan_hyprland_overrides(Some(false), Some(1.0), Some(1.0), Some(1.0)).is_empty());

        // Blur on + translucent active/inactive + opaque fullscreen → only the 3 active ones.
        let plan = plan_hyprland_overrides(Some(true), Some(0.9), Some(0.8), Some(1.0));
        assert_eq!(
            plan,
            vec![
                (HYPR_BLUR, "false", "true".to_owned()),
                (HYPR_ACTIVE_OPACITY, "1.0", "0.900000".to_owned()),
                (HYPR_INACTIVE_OPACITY, "1.0", "0.800000".to_owned()),
            ]
        );
        assert!(hyprland_keyword_ok("ok\n"));
        assert!(!hyprland_keyword_ok(""));
        assert!(!hyprland_keyword_ok("config option does not exist error"));
    }

    #[test]
    fn niri_selection_picks_intersecting_positioned_windows_and_falls_back_on_legacy_tiles() {
        let outputs: HashMap<String, Output> = [(
            "HDMI-A-1".to_owned(),
            serde_json::from_str(
                r#"{"name":"HDMI-A-1","make":"","model":"","serial":null,
                    "physical_size":[540,300],"modes":[],"current_mode":1,"is_custom_mode":false,
                    "vrr_supported":false,"vrr_enabled":false,
                    "logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}}"#,
            )
            .unwrap(),
        )]
        .into_iter()
        .collect();
        let workspaces: Vec<Workspace> = vec![
            serde_json::from_str(
                r#"{"id":1,"idx":1,"name":"1","output":"HDMI-A-1","is_urgent":false,
                    "is_active":true,"is_focused":true,"active_window_id":10}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"id":2,"idx":2,"name":"2","output":"HDMI-A-1","is_urgent":false,
                    "is_active":false,"is_focused":false,"active_window_id":99}"#,
            )
            .unwrap(),
        ];

        // Modern niri (positioned tiled + floating) + inactive workspace window.
        let modern_windows: Vec<Window> = vec![
            serde_json::from_str(
                r#"{"id":10,"title":"left","app_id":"kitty","pid":1,"workspace_id":1,
                    "is_focused":true,"is_floating":false,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[900.0,1000.0],
                    "window_size":[900,1000],"tile_pos_in_workspace_view":[10.0,20.0],
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"id":11,"title":"right","app_id":"zen","pid":1,"workspace_id":1,
                    "is_focused":false,"is_floating":false,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":[2,1],"tile_size":[900.0,1000.0],
                    "window_size":[900,1000],"tile_pos_in_workspace_view":[950.0,20.0],
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"id":99,"title":"hidden","app_id":"foot","pid":1,"workspace_id":2,
                    "is_focused":false,"is_floating":false,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[900.0,1000.0],
                    "window_size":[900,1000],"tile_pos_in_workspace_view":[10.0,20.0],
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
        ];

        let rect_right = ScrollRect {
            x: 1000,
            y: 100,
            width: 500,
            height: 600,
        };
        assert_eq!(
            select_niri_window_ids(
                &modern_windows,
                &workspaces,
                &outputs,
                "HDMI-A-1",
                rect_right
            ),
            vec![11]
        );

        // Legacy niri <= 26.04: tiled windows report tile_pos_in_workspace_view = null.
        let legacy_windows: Vec<Window> = vec![
            serde_json::from_str(
                r#"{"id":20,"title":"tile1","app_id":"kitty","pid":1,"workspace_id":1,
                    "is_focused":true,"is_floating":false,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[900.0,1000.0],
                    "window_size":[900,1000],"tile_pos_in_workspace_view":null,
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"id":21,"title":"tile2","app_id":"zen","pid":1,"workspace_id":1,
                    "is_focused":false,"is_floating":false,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":[2,1],"tile_size":[900.0,1000.0],
                    "window_size":[900,1000],"tile_pos_in_workspace_view":null,
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"id":22,"title":"float-elsewhere","app_id":"imv","pid":1,"workspace_id":1,
                    "is_focused":false,"is_floating":true,"is_urgent":false,
                    "layout":{"pos_in_scrolling_layout":null,"tile_size":[200.0,200.0],
                    "window_size":[200,200],"tile_pos_in_workspace_view":[10.0,10.0],
                    "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            )
            .unwrap(),
        ];
        // Selection outside the floating window falls back to the unpositioned tiled windows.
        assert_eq!(
            select_niri_window_ids(
                &legacy_windows,
                &workspaces,
                &outputs,
                "HDMI-A-1",
                rect_right
            ),
            vec![20, 21]
        );
        // Selection inside the floating window targets only that floating window.
        let rect_float = ScrollRect {
            x: 20,
            y: 20,
            width: 100,
            height: 100,
        };
        assert_eq!(
            select_niri_window_ids(
                &legacy_windows,
                &workspaces,
                &outputs,
                "HDMI-A-1",
                rect_float
            ),
            vec![22]
        );
    }

    #[test]
    fn kde_session_detection_checks_env_hints() {
        assert!(is_kde_session(true, None, None));
        assert!(is_kde_session(false, Some("KDE"), None));
        assert!(is_kde_session(false, Some("ubuntu:Plasma"), None));
        assert!(is_kde_session(false, None, Some("plasmawayland")));
        assert!(!is_kde_session(false, Some("niri"), Some("niri")));
        assert!(!is_kde_session(false, None, None));
    }

    #[test]
    fn custom_commands_run_with_env_and_restore_exactly_once_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("events.log");
        let rect = ScrollRect {
            x: 12,
            y: 34,
            width: 560,
            height: 780,
        };
        let cfg = ScrollEffectsConfig {
            mode: ScrollTransparencyMode::Custom,
            enter_command: Some(format!(
                "echo \"enter:$SHOTORI_SCROLL_OUTPUT:$SHOTORI_SCROLL_X,$SHOTORI_SCROLL_Y,$SHOTORI_SCROLL_W,$SHOTORI_SCROLL_H\" >> '{}'",
                log.display()
            )),
            exit_command: Some(format!(
                "echo \"exit:$SHOTORI_SCROLL_OUTPUT\" >> '{}'",
                log.display()
            )),
        };

        let mut guard = EffectsGuard::enter("DP-1", rect, &cfg);
        assert!(guard.is_active());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "enter:DP-1:12,34,560,780\n"
        );

        // Explicit restore + subsequent drop must run exit_command only once.
        guard.restore();
        drop(guard);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "enter:DP-1:12,34,560,780\nexit:DP-1\n"
        );
    }

    #[test]
    fn off_mode_and_failing_custom_commands_never_panic() {
        let rect = ScrollRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let off = ScrollEffectsConfig {
            mode: ScrollTransparencyMode::Off,
            enter_command: Some("exit 1".into()),
            exit_command: Some("exit 1".into()),
        };
        let guard = EffectsGuard::enter("DP-1", rect, &off);
        assert!(!guard.is_active());
        drop(guard);

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("restored");
        let failing_enter = ScrollEffectsConfig {
            mode: ScrollTransparencyMode::Custom,
            enter_command: Some("echo oops >&2; exit 42".into()),
            exit_command: Some(format!("touch '{}'", marker.display())),
        };
        {
            let _guard = EffectsGuard::enter("DP-1", rect, &failing_enter);
        }
        assert!(
            marker.exists(),
            "exit_command must still run after a failing enter_command"
        );
    }

    #[test]
    fn hung_custom_command_is_killed_after_timeout() {
        let env = CommandEnv {
            output: "DP-1".into(),
            rect: ScrollRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
        };
        let start = Instant::now();
        let ok = run_shell_command("test", "sleep 10", &env, Duration::from_millis(80));
        assert!(!ok);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "hung command must be killed promptly, took {:?}",
            start.elapsed()
        );
    }
}
