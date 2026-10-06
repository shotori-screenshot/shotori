//! Hyprland backend: `HYPRLAND_INSTANCE_SIGNATURE` → request socket
//! (`j/clients` + `j/monitors`).
//!
//! `j/clients` lists every mapped window including inactive workspaces;
//! visibility is derived by keeping only windows whose workspace is the
//! ACTIVE workspace of their monitor (from `j/monitors`). The socket
//! path moved from `/tmp/hypr` to `$XDG_RUNTIME_DIR/hypr` across
//! versions — both are tried.
//!
//! Caveat: written from Hyprland's IPC docs, not tested on a
//! live session; the `at`/`size` fields have had scaled-monitor
//! physical-vs-logical quirks historically. Reports welcome.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use gpui_kit::{Bounds, point, px, size};
use serde_json::Value;

use super::SnapRect;

pub fn query() -> Option<Vec<SnapRect>> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());

    // One connection per request: the restructured IPC closes the socket
    // after each reply (measured live) — pipelining a second request on
    // the same stream fails
    let ask = |cmd: &str| -> Option<String> {
        [
            format!("{runtime}/hypr/{sig}/.socket.sock"),
            format!("/tmp/hypr/{sig}/.socket.sock"),
        ]
        .iter()
        .find_map(|p| {
            let mut s = UnixStream::connect(p).ok()?;
            request(&mut s, cmd).ok()
        })
    };

    let clients: Value = serde_json::from_str(&ask("j/clients")?).ok()?;
    let monitors: Value = serde_json::from_str(&ask("j/monitors")?).ok()?;
    Some(collect(&clients, &monitors))
}

/// Hyprland's request socket, tolerant across the 2026 IPC restructure:
/// newer builds match commands EXACTLY and a trailing '\n' turns every
/// exact command into "unknown request" (only prefix-matched ones like
/// `monitors` survive — measured live), so the command goes out bare
/// first. Older line-based servers wait for a newline — if the bare
/// attempt stays silent for 400ms, send one. Do NOT half-close the write
/// side: the new event loop treats the EOF as a disconnect and drops the
/// request (also measured). Replies arrive until EOF or timeout.
fn request(stream: &mut UnixStream, cmd: &str) -> std::io::Result<String> {
    let read_all = |s: &mut UnixStream| -> std::io::Result<String> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16384];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    };

    stream.write_all(cmd.as_bytes())?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(400)))?;
    let reply = read_all(stream)?;
    if !reply.is_empty() {
        return Ok(reply);
    }
    // legacy line-based server: it is still waiting for the newline
    stream.write_all(b"\n")?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(600)))?;
    read_all(stream)
}

/// clients + monitors → visible window rects. Pure, unit-tested.
fn collect(clients: &Value, monitors: &Value) -> Vec<SnapRect> {
    // active workspace id per monitor, keyed both ways the schema has
    // spelled the client's "monitor" field (int id, then str name)
    let mut active_by_id: Vec<(i64, i64)> = Vec::new();
    let mut active_by_name: Vec<(String, i64)> = Vec::new();
    if let Some(mons) = monitors.as_array() {
        for m in mons {
            let Some(active) = m.pointer("/activeWorkspace/id").and_then(Value::as_i64) else {
                continue;
            };
            if let Some(id) = m.get("id").and_then(Value::as_i64) {
                active_by_id.push((id, active));
            }
            if let Some(name) = m.get("name").and_then(Value::as_str) {
                active_by_name.push((name.to_owned(), active));
            }
        }
    }
    let Some(list) = clients.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            // `visible` (mapped && acceptsInput && alphaNonZero) is the
            // modern filter — source-verified in src/ipc/s1/Commands.cpp;
            // older versions only carry `mapped`
            let visible = c
                .get("visible")
                .and_then(Value::as_bool)
                .or_else(|| c.get("mapped").and_then(Value::as_bool))
                .unwrap_or(false);
            if !visible {
                return None;
            }
            let ws_id = match c.pointer("/workspace/id").and_then(Value::as_i64) {
                Some(id) if id > 0 => id, // special workspaces are negative
                _ => return None,
            };
            // only windows on the active workspace of their monitor
            let on_active = match c.get("monitor") {
                Some(Value::Number(n)) => n
                    .as_i64()
                    .and_then(|id| active_by_id.iter().find(|(m, _)| *m == id))
                    .is_some_and(|(_, active)| *active == ws_id),
                Some(Value::String(name)) => active_by_name
                    .iter()
                    .find(|(m, _)| m == name)
                    .is_some_and(|(_, active)| *active == ws_id),
                _ => false,
            };
            if !on_active {
                return None;
            }
            let at = c.get("at").and_then(Value::as_array)?;
            let sz = c.get("size").and_then(Value::as_array)?;
            let (x, y) = (
                at.first().and_then(Value::as_f64).unwrap_or_default(),
                at.get(1).and_then(Value::as_f64).unwrap_or_default(),
            );
            let (w, h) = (
                sz.first().and_then(Value::as_f64).unwrap_or(0.),
                sz.get(1).and_then(Value::as_f64).unwrap_or(0.),
            );
            if w <= 0. || h <= 0. {
                return None;
            }
            Some(SnapRect {
                bounds: Bounds {
                    origin: point(px(x as f32), px(y as f32)),
                    size: size(px(w as f32), px(h as f32)),
                },
                app_id: c
                    .get("class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                focused: false, // clients JSON carries no focus flag
                recency: 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*` (gpui test-macro shadowing; see
    // selection.rs)
    use super::collect;
    use serde_json::Value;

    const MONITORS: &str = r#"[
        {"id": 0, "name": "DP-1", "x": 0, "y": 0,
         "activeWorkspace": {"id": 1, "name": "1"}},
        {"id": 1, "name": "HDMI-A-1", "x": 1920, "y": 0,
         "activeWorkspace": {"id": 3, "name": "3"}}
    ]"#;

    #[test]
    fn keeps_only_active_workspace_windows_per_monitor() {
        let monitors: Value = serde_json::from_str(MONITORS).unwrap();
        // monitor spelled as int id (older schema) for the first two,
        // as a name (newer schema) for the third
        let clients: Value = serde_json::from_str(
            r#"[
                {"class": "kitty", "mapped": true, "visible": true, "monitor": 0,
                 "workspace": {"id": 1, "name": "1"},
                 "at": [10, 20], "size": [940, 1040]},
                {"class": "hidden-on-ws2", "mapped": true, "visible": true, "monitor": 0,
                 "workspace": {"id": 2, "name": "2"},
                 "at": [10, 20], "size": [940, 1040]},
                {"class": "firefox", "mapped": true, "visible": true, "monitor": "HDMI-A-1",
                 "workspace": {"id": 3, "name": "3"},
                 "at": [1930, 10], "size": [1900, 1060]},
                {"class": "unmapped", "mapped": false, "visible": false, "monitor": 0,
                 "workspace": {"id": 1, "name": "1"},
                 "at": [0, 0], "size": [100, 100]},
                {"class": "legacy-no-visible-field", "mapped": true, "monitor": 0,
                 "workspace": {"id": 1, "name": "1"},
                 "at": [0, 0], "size": [50, 60]},
                {"class": "special", "mapped": true, "visible": true, "monitor": 0,
                 "workspace": {"id": -99, "name": "special:magic"},
                 "at": [0, 0], "size": [100, 100]}
            ]"#,
        )
        .unwrap();

        let rects = collect(&clients, &monitors);
        assert_eq!(
            rects.len(),
            3,
            "inactive-ws, unmapped and special dropped; legacy mapped-only kept"
        );
        assert_eq!(rects[0].app_id, "kitty");
        assert_eq!(
            (
                f32::from(rects[0].bounds.origin.x),
                f32::from(rects[0].bounds.origin.y)
            ),
            (10., 20.)
        );
        assert_eq!(rects[1].app_id, "firefox");
        assert_eq!(f32::from(rects[1].bounds.origin.x), 1930.);
        // legacy schema (no `visible` key) falls back to `mapped`
        assert_eq!(rects[2].app_id, "legacy-no-visible-field");
        assert_eq!(
            (
                f32::from(rects[2].bounds.size.width),
                f32::from(rects[2].bounds.size.height)
            ),
            (50., 60.)
        );
    }

    #[test]
    fn empty_or_malformed_inputs_yield_nothing() {
        let monitors: Value = serde_json::from_str(MONITORS).unwrap();
        let empty: Value = serde_json::from_str("[]").unwrap();
        assert!(collect(&empty, &monitors).is_empty());
        let garbage: Value = serde_json::from_str("{}").unwrap();
        assert!(collect(&garbage, &monitors).is_empty());
    }
}
