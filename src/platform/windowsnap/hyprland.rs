//! Hyprland backend: `HYPRLAND_INSTANCE_SIGNATURE` → request socket
//! (`j/clients` + `j/monitors`).
//!
//! `j/clients` lists every mapped window including inactive workspaces
//! and inactive group tabs; visibility is derived by dropping `"hidden": true`
//! windows and keeping only windows whose workspace matches their monitor's
//! `activeWorkspace` or currently open `specialWorkspace` (from `j/monitors`).
//! Stacking recency combines compositor layer (open scratchpad above
//! pinned/floating above tiled) with `focusHistoryID` so overlapping unfocused
//! windows hit-test in Z-order. The socket path moved from `/tmp/hypr` to
//! `$XDG_RUNTIME_DIR/hypr` across versions — both are tried.
//!
//! Caveat: written from Hyprland's IPC docs, not tested on a
//! live session; the `at`/`size` fields have had scaled-monitor
//! physical-vs-logical quirks historically. Reports welcome.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

use super::{CompositorSnapshot, SnapRect};

pub fn query() -> Option<CompositorSnapshot> {
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
    Some(CompositorSnapshot {
        rects: collect(&clients, &monitors),
        focused_output: focused_monitor(&monitors),
    })
}

fn focused_monitor(monitors: &Value) -> Option<String> {
    monitors
        .as_array()?
        .iter()
        .find(|m| m.get("focused").and_then(Value::as_bool).unwrap_or(false))
        .and_then(|m| m.get("name").and_then(Value::as_str))
        .map(str::to_owned)
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
    const MAX_PAYLOAD: usize = 256 << 20;
    let read_all = |s: &mut UnixStream| -> std::io::Result<String> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16384];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len().saturating_add(n) > MAX_PAYLOAD {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "Hyprland IPC reply exceeds sanity cap",
                        ));
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
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

    stream.set_write_timeout(Some(Duration::from_millis(400)))?;
    stream.write_all(cmd.as_bytes())?;
    stream.set_read_timeout(Some(Duration::from_millis(400)))?;
    let reply = read_all(stream)?;
    if !reply.is_empty() {
        return Ok(reply);
    }
    // legacy line-based server: it is still waiting for the newline
    stream.write_all(b"\n")?;
    stream.set_read_timeout(Some(Duration::from_millis(600)))?;
    read_all(stream)
}

/// clients + monitors → visible window rects. Pure, unit-tested.
fn collect(clients: &Value, monitors: &Value) -> Vec<SnapRect> {
    #[derive(Clone, Copy)]
    struct MonitorWorkspaces {
        active: Option<i64>,
        open_special: Option<i64>,
    }

    // Active and open-special workspace ids per monitor, keyed both ways
    // the schema has spelled the client's "monitor" field (int id, str name).
    let mut mon_by_id: Vec<(i64, MonitorWorkspaces)> = Vec::new();
    let mut mon_by_name: Vec<(String, MonitorWorkspaces)> = Vec::new();
    if let Some(mons) = monitors.as_array() {
        for m in mons {
            let active = m
                .pointer("/activeWorkspace/id")
                .and_then(Value::as_i64)
                .filter(|&id| id != 0);
            let open_special = m
                .pointer("/specialWorkspace/id")
                .and_then(Value::as_i64)
                .filter(|&id| id != 0);
            if active.is_none() && open_special.is_none() {
                continue;
            }
            let ws = MonitorWorkspaces {
                active,
                open_special,
            };
            if let Some(id) = m.get("id").and_then(Value::as_i64) {
                mon_by_id.push((id, ws));
            }
            if let Some(name) = m.get("name").and_then(Value::as_str) {
                mon_by_name.push((name.to_owned(), ws));
            }
        }
    }
    let Some(list) = clients.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            // Inactive tabs inside a window group (`dwindle`/`master` groups)
            // report `"hidden": true` (including in older schemas that only
            // carry `"mapped": true`).
            if c.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
                return None;
            }
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
                Some(id) if id != 0 => id,
                _ => return None,
            };
            // Keep windows on either the monitor's active workspace or its
            // currently open special workspace (scratchpad overlay).
            let mon_ws = match c.get("monitor") {
                Some(Value::Number(n)) => n
                    .as_i64()
                    .and_then(|id| mon_by_id.iter().find(|(m, _)| *m == id))
                    .map(|(_, ws)| *ws),
                Some(Value::String(name)) => mon_by_name
                    .iter()
                    .find(|(m, _)| m == name)
                    .map(|(_, ws)| *ws),
                _ => None,
            }?;
            let on_open_special = mon_ws.open_special == Some(ws_id);
            if mon_ws.active != Some(ws_id) && !on_open_special {
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
            let focus_id = c.get("focusHistoryID").and_then(Value::as_i64);
            let pinned_or_floating = c.get("pinned").and_then(Value::as_bool).unwrap_or(false)
                || c.get("floating").and_then(Value::as_bool).unwrap_or(false);
            // Encode compositor Z-layer above `focusHistoryID` MRU rank so
            // open scratchpads (2) > pinned/floating (1) > tiled (0), and
            // within a layer more recently focused windows beat older ones.
            let tier: u32 = if on_open_special || ws_id < 0 {
                2
            } else if pinned_or_floating {
                1
            } else {
                0
            };
            let focus_rank: u32 = match focus_id {
                Some(id) if id >= 0 => u32::MAX - (id as u64).min(u64::from(u32::MAX - 1)) as u32,
                _ => 0,
            };
            let recency = (u64::from(tier) << 32) | u64::from(focus_rank);
            Some(SnapRect::from_xywh(
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                c.get("class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                focus_id == Some(0),
                recency,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*` (gpui test-macro shadowing; see
    // selection.rs)
    use super::{collect, focused_monitor};
    use serde_json::Value;

    const MONITORS: &str = r#"[
        {"id": 0, "name": "DP-1", "x": 0, "y": 0, "focused": false,
         "activeWorkspace": {"id": 1, "name": "1"}},
        {"id": 1, "name": "HDMI-A-1", "x": 1920, "y": 0, "focused": true,
         "activeWorkspace": {"id": 3, "name": "3"}}
    ]"#;

    #[test]
    fn keeps_only_active_workspace_windows_per_monitor() {
        let monitors: Value = serde_json::from_str(MONITORS).unwrap();
        assert_eq!(focused_monitor(&monitors).as_deref(), Some("HDMI-A-1"));
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

    #[test]
    fn open_special_workspace_collected_when_open_and_dropped_when_closed() {
        let clients: Value = serde_json::from_str(
            r#"[
                {"class": "tiled-base", "mapped": true, "visible": true, "monitor": 0,
                 "workspace": {"id": 1, "name": "1"},
                 "at": [0, 0], "size": [1920, 1080], "focusHistoryID": 1},
                {"class": "scratchpad-open", "mapped": true, "visible": true, "monitor": "DP-1",
                 "workspace": {"id": -99, "name": "special:magic"},
                 "at": [300, 200], "size": [900, 600], "focusHistoryID": 0},
                {"class": "other-scratchpad", "mapped": true, "visible": true, "monitor": 0,
                 "workspace": {"id": -98, "name": "special:music"},
                 "at": [400, 250], "size": [700, 500], "focusHistoryID": 2}
            ]"#,
        )
        .unwrap();

        let monitors_open: Value = serde_json::from_str(
            r#"[{"id": 0, "name": "DP-1", "x": 0, "y": 0, "focused": true,
                "activeWorkspace": {"id": 1, "name": "1"},
                "specialWorkspace": {"id": -99, "name": "special:magic"}}]"#,
        )
        .unwrap();
        let rects_open = collect(&clients, &monitors_open);
        let ids_open: Vec<&str> = rects_open.iter().map(|r| r.app_id.as_str()).collect();
        assert_eq!(ids_open, vec!["tiled-base", "scratchpad-open"]);

        let monitors_closed: Value = serde_json::from_str(
            r#"[{"id": 0, "name": "DP-1", "x": 0, "y": 0, "focused": true,
                "activeWorkspace": {"id": 1, "name": "1"},
                "specialWorkspace": {"id": 0, "name": ""}}]"#,
        )
        .unwrap();
        let rects_closed = collect(&clients, &monitors_closed);
        let ids_closed: Vec<&str> = rects_closed.iter().map(|r| r.app_id.as_str()).collect();
        assert_eq!(ids_closed, vec!["tiled-base"]);
    }

    #[test]
    fn hidden_group_tab_windows_are_dropped() {
        let monitors: Value = serde_json::from_str(MONITORS).unwrap();
        let clients: Value = serde_json::from_str(
            r#"[
                {"class": "active-tab", "mapped": true, "visible": true, "hidden": false,
                 "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [10, 20], "size": [900, 1000]},
                {"class": "inactive-tab-modern", "mapped": true, "visible": true, "hidden": true,
                 "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [10, 20], "size": [900, 1000]},
                {"class": "inactive-tab-legacy", "mapped": true, "hidden": true,
                 "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [10, 20], "size": [900, 1000]}
            ]"#,
        )
        .unwrap();

        let rects = collect(&clients, &monitors);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].app_id, "active-tab");
    }

    #[test]
    fn recency_orders_special_above_floating_mru_above_tiled() {
        use super::super::hit_test;
        use gpui_kit::{point, px};

        // Focus sits on HDMI-A-1 (`focusHistoryID: 0`), so every window on DP-1
        // is unfocused and hit-testing relies purely on stacking `recency`.
        let monitors: Value = serde_json::from_str(
            r#"[
                {"id": 0, "name": "DP-1", "x": 0, "y": 0, "focused": false,
                 "activeWorkspace": {"id": 1, "name": "1"},
                 "specialWorkspace": {"id": -99, "name": "special:magic"}},
                {"id": 1, "name": "HDMI-A-1", "x": 1920, "y": 0, "focused": true,
                 "activeWorkspace": {"id": 3, "name": "3"},
                 "specialWorkspace": {"id": 0, "name": ""}}
            ]"#,
        )
        .unwrap();
        let clients: Value = serde_json::from_str(
            r#"[
                {"class": "tiled-under", "mapped": true, "visible": true, "floating": false,
                 "pinned": false, "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [0, 0], "size": [1920, 1080], "focusHistoryID": 2},
                {"class": "float-small-buried", "mapped": true, "visible": true, "floating": true,
                 "pinned": false, "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [200, 200], "size": [200, 150], "focusHistoryID": 3},
                {"class": "float-large-top", "mapped": true, "visible": true, "floating": true,
                 "pinned": false, "monitor": 0, "workspace": {"id": 1, "name": "1"},
                 "at": [100, 100], "size": [700, 600], "focusHistoryID": 1},
                {"class": "scratchpad-overlay", "mapped": true, "visible": true, "floating": false,
                 "pinned": false, "monitor": 0, "workspace": {"id": -99, "name": "special:magic"},
                 "at": [500, 400], "size": [800, 500], "focusHistoryID": 4}
            ]"#,
        )
        .unwrap();

        let rects = collect(&clients, &monitors);
        assert_eq!(rects.len(), 4);
        let tiled = &rects[0];
        let buried_float = &rects[1];
        let top_float = &rects[2];
        let special = &rects[3];
        assert!(
            special.recency > top_float.recency
                && top_float.recency > buried_float.recency
                && buried_float.recency > tiled.recency
        );

        // At (250, 250): tiled, buried small float, and larger top float all
        // overlap — the larger top float (`focusHistoryID: 1`) must win.
        let hit_float = hit_test(&rects, point(px(250.), px(250.))).expect("hit float");
        assert_eq!(rects[hit_float].app_id, "float-large-top");

        // At (600, 500): open scratchpad also overlaps `float-large-top` and
        // `tiled-under` — scratchpad layer wins despite older focusHistoryID.
        let hit_special = hit_test(&rects, point(px(600.), px(500.))).expect("hit scratchpad");
        assert_eq!(rects[hit_special].app_id, "scratchpad-overlay");
    }
}
