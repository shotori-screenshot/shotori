//! sway backend: `$SWAYSOCK` → i3 IPC `GET_TREE`, rect space = global
//! logical (same space as the outputs' positions; sway heritage, stable
//! for a decade).
//!
//! The tree contains EVERYTHING (all workspaces, scratchpad); leaf
//! containers carry a `visible` flag that is false for windows on
//! inactive workspaces and hidden scratchpad windows — exactly the
//! visibility filter we need. Written from the i3/sway IPC docs; tested
//! against JSON fixtures, not a live sway (reports welcome).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::Value;

use super::SnapRect;

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_TREE: u32 = 4;

pub fn query() -> Option<Vec<SnapRect>> {
    let path = std::env::var("SWAYSOCK").ok()?;
    let mut stream = UnixStream::connect(path).ok()?;
    // The query wrapper has a 1 s budget on the main thread, but a hung
    // i3-IPC socket would also leak this helper thread forever — cap the
    // read so the thread can always exit (hyprland.rs does the same).
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
    let payload = ipc_call(&mut stream, GET_TREE).ok()?;
    let tree: Value = serde_json::from_slice(&payload).ok()?;
    let mut rects = Vec::new();
    walk(&tree, &mut rects);
    Some(rects)
}

/// One i3-IPC round trip: request header → reply header + JSON payload.
/// Integers use native endianness per the protocol spec — and the socket
/// is local, so native here is the compositor's native too.
fn ipc_call(stream: &mut UnixStream, msg_type: u32) -> std::io::Result<Vec<u8>> {
    let mut msg = Vec::with_capacity(14);
    msg.extend_from_slice(MAGIC);
    msg.extend_from_slice(&0u32.to_ne_bytes());
    msg.extend_from_slice(&msg_type.to_ne_bytes());
    stream.write_all(&msg)?;

    let mut header = [0u8; 14];
    stream.read_exact(&mut header)?;
    if &header[..6] != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad i3-ipc magic",
        ));
    }
    let len = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
    // The length arrives over the socket, so treat it as unbounded until
    // checked: a wedged or broken compositor must not make us allocate
    // up to 4 GiB before the first payload byte. Real GET_TREE replies
    // are kilobytes; 256 MiB is a floor nobody hits.
    const MAX_PAYLOAD: usize = 256 << 20;
    if len > MAX_PAYLOAD {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "i3-ipc payload exceeds sanity cap",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

/// Depth-first over `nodes` (tiled) and `floating_nodes`; collect visible
/// leaf windows. Pure, unit-tested.
fn walk(node: &Value, out: &mut Vec<SnapRect>) {
    let is_window = node.get("app_id").and_then(Value::as_str).is_some()
        || node
            .get("window")
            .and_then(Value::as_i64)
            .is_some_and(|w| w != 0);
    if is_window
        && node
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && let Some(rect) = node.get("rect")
    {
        let (x, y) = (
            rect["x"].as_f64().unwrap_or_default(),
            rect["y"].as_f64().unwrap_or_default(),
        );
        let (w, h) = (
            rect["width"].as_f64().unwrap_or(0.),
            rect["height"].as_f64().unwrap_or(0.),
        );
        if w > 0. && h > 0. {
            // xwayland windows have no app_id; fall back to the X class
            let app_id = node
                .get("app_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    node.pointer("/window_properties/class")
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_owned();
            out.push(SnapRect::from_xywh(
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                app_id,
                node.get("focused")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                0, // the tree carries no timestamps
            ));
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                walk(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*` (gpui test-macro shadowing; see
    // selection.rs)
    use super::walk;
    use serde_json::Value;

    /// Trimmed sway `GET_TREE` shape: root → output → workspace →
    /// tiled window + floating window, plus an invisible workspace.
    const TREE: &str = r#"{
        "id": 1, "name": "root", "type": "root",
        "nodes": [{
            "id": 2, "name": "HDMI-A-1", "type": "output",
            "nodes": [{
                "id": 3, "name": "1", "type": "workspace", "visible": true,
                "nodes": [{
                    "id": 4, "name": "Alacritty", "type": "con",
                    "app_id": "Alacritty", "window": null, "visible": true,
                    "focused": false,
                    "rect": {"x": 0, "y": 0, "width": 954, "height": 1048}
                }],
                "floating_nodes": [{
                    "id": 5, "name": "pavucontrol", "type": "floating_con",
                    "app_id": "pavucontrol", "window": null, "visible": true,
                    "focused": true,
                    "rect": {"x": 300, "y": 200, "width": 500, "height": 400}
                }]
            }, {
                "id": 6, "name": "2", "type": "workspace", "visible": false,
                "nodes": [{
                    "id": 7, "name": "hidden editor", "type": "con",
                    "app_id": "nvim", "window": null, "visible": false,
                    "rect": {"x": 0, "y": 0, "width": 954, "height": 1048}
                }],
                "floating_nodes": []
            }]
        }]
    }"#;

    #[test]
    fn collects_visible_tiled_and_floating_skips_inactive() {
        let tree: Value = serde_json::from_str(TREE).unwrap();
        let mut rects = Vec::new();
        walk(&tree, &mut rects);
        assert_eq!(rects.len(), 2, "hidden workspace window must be skipped");

        assert_eq!(rects[0].app_id, "Alacritty");
        assert_eq!(
            (
                f32::from(rects[0].bounds.origin.x),
                f32::from(rects[0].bounds.origin.y)
            ),
            (0., 0.)
        );
        assert!(!rects[0].focused);

        assert_eq!(rects[1].app_id, "pavucontrol");
        assert!(rects[1].focused);
        assert_eq!(
            (
                f32::from(rects[1].bounds.size.width),
                f32::from(rects[1].bounds.size.height)
            ),
            (500., 400.)
        );
    }

    #[test]
    fn xwayland_window_uses_window_properties_class() {
        let tree: Value = serde_json::from_str(
            r#"{"id": 9, "type": "con", "app_id": null, "window": 12345,
                "visible": true,
                "window_properties": {"class": "Steam"},
                "rect": {"x": 10, "y": 20, "width": 100, "height": 50}}"#,
        )
        .unwrap();
        let mut rects = Vec::new();
        walk(&tree, &mut rects);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].app_id, "Steam");
    }
}
