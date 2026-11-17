//! sway backend: `$SWAYSOCK` → i3 IPC `GET_TREE`, rect space = global
//! logical (same space as the outputs' positions; sway heritage, stable
//! for a decade).
//!
//! The tree contains EVERYTHING (all workspaces, scratchpad); leaf
//! containers carry a `visible` flag that is false for windows on
//! inactive workspaces and hidden scratchpad windows — exactly the
//! visibility filter we need. Container `"focus"` arrays (MRU child IDs)
//! and `floating_nodes` membership drive `SnapRect::recency` so
//! overlapping unfocused windows hit-test in compositor Z-order. Written
//! from the i3/sway IPC docs; tested against JSON fixtures, not a live
//! sway (reports welcome).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

use super::{CompositorSnapshot, SnapRect};

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_TREE: u32 = 4;

pub fn query() -> Option<CompositorSnapshot> {
    let path = std::env::var("SWAYSOCK").ok()?;
    let mut stream = UnixStream::connect(path).ok()?;
    // The query wrapper has a 1 s budget on the main thread, but a hung
    // i3-IPC socket would also leak this helper thread forever — cap both
    // directions so the thread can always exit (niri.rs / hyprland.rs do
    // the same).
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    let payload = ipc_call(&mut stream, GET_TREE).ok()?;
    let tree: Value = serde_json::from_slice(&payload).ok()?;
    let mut rects = Vec::new();
    walk(&tree, &mut rects);
    Some(CompositorSnapshot {
        rects,
        focused_output: focused_output(&tree),
    })
}

fn focused_output(tree: &Value) -> Option<String> {
    fn subtree_has_focus(node: &Value) -> bool {
        if node
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return true;
        }
        for key in ["nodes", "floating_nodes"] {
            if let Some(children) = node.get(key).and_then(Value::as_array)
                && children.iter().any(subtree_has_focus)
            {
                return true;
            }
        }
        false
    }

    tree.get("nodes")?
        .as_array()?
        .iter()
        .filter(|n| n.get("type").and_then(Value::as_str) == Some("output"))
        .find_map(|out| {
            let name = out.get("name").and_then(Value::as_str)?;
            if name == "__i3" || !subtree_has_focus(out) {
                return None;
            }
            Some(name.to_owned())
        })
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
    let len = u32::from_ne_bytes([header[6], header[7], header[8], header[9]]) as usize;
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
/// leaf windows while deriving Z-order `recency` from floating layer +
/// ancestor `"focus"` MRU arrays. Pure, unit-tested.
fn walk(node: &Value, out: &mut Vec<SnapRect>) {
    walk_with_rank(node, false, 0, u64::from(u32::MAX), out);
}

fn walk_with_rank(node: &Value, in_floating: bool, lo: u64, hi: u64, out: &mut Vec<SnapRect>) {
    let is_floating =
        in_floating || node.get("type").and_then(Value::as_str) == Some("floating_con");
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
            // Sway always composites `floating_nodes` above tiled `nodes`;
            // within a layer, `lo` encodes the leaf's lexicographic MRU rank
            // across ancestor `"focus"` arrays (0 when `"focus"` is absent).
            let recency = (u64::from(is_floating) << 32) | lo;
            out.push(SnapRect::from_xywh(
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                app_id,
                node.get("focused")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                recency,
            ));
        }
    }
    for (key, child_floating) in [("nodes", is_floating), ("floating_nodes", true)] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                let (child_lo, child_hi) = child_focus_range(lo, hi, node, child);
                walk_with_rank(child, child_floating, child_lo, child_hi, out);
            }
        }
    }
}

/// Narrow `[lo, hi]` to the sub-interval for `child` according to `parent`'s
/// `"focus"` MRU array (`focus[0]` is most recent). Bucket 0 is reserved for
/// unlisted children so missing `"focus"` arrays leave `lo == 0`.
fn child_focus_range(lo: u64, hi: u64, parent: &Value, child: &Value) -> (u64, u64) {
    let Some(focus) = parent
        .get("focus")
        .and_then(Value::as_array)
        .filter(|f| !f.is_empty())
    else {
        return (lo, hi);
    };
    let pos = child
        .get("id")
        .and_then(Value::as_i64)
        .and_then(|cid| focus.iter().position(|v| v.as_i64() == Some(cid)));
    let buckets = focus.len() as u128 + 1;
    let bucket = match pos {
        Some(idx) => (focus.len() - idx) as u128,
        None => 0,
    };
    let span = u128::from(hi - lo) + 1;
    if span < buckets {
        let clamped = lo + bucket.min(span - 1) as u64;
        return (clamped, clamped);
    }
    let child_lo = lo + ((span * bucket) / buckets) as u64;
    let child_hi = lo + ((span * (bucket + 1)) / buckets - 1) as u64;
    (child_lo, child_hi)
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*` (gpui test-macro shadowing; see
    // selection.rs)
    use super::{focused_output, walk};
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
        assert_eq!(focused_output(&tree).as_deref(), Some("HDMI-A-1"));
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

    #[test]
    fn floating_focus_mru_outranks_smaller_buried_floating_and_tiled() {
        use super::super::hit_test;
        use gpui_kit::{point, px};

        // Workspace has `focus: [52, 51, 40]` while keyboard focus is on
        // another output, so all three windows on this workspace are unfocused:
        // - 40: full-screen tiled background
        // - 51: smaller buried floating window (pushed earlier in `floating_nodes`)
        // - 52: larger top-most floating window (`focus[0]`)
        let tree: Value = serde_json::from_str(
            r#"{
                "id": 1, "name": "root", "type": "root",
                "focus": [80, 2],
                "nodes": [{
                    "id": 2, "name": "HDMI-A-1", "type": "output",
                    "focus": [3],
                    "nodes": [{
                        "id": 3, "name": "1", "type": "workspace", "visible": true,
                        "focus": [52, 51, 40],
                        "nodes": [{
                            "id": 40, "name": "foot", "type": "con",
                            "app_id": "foot", "window": null, "visible": true,
                            "focused": false,
                            "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080}
                        }],
                        "floating_nodes": [{
                            "id": 51, "name": "small-buried", "type": "floating_con",
                            "app_id": "imv-small", "window": null, "visible": true,
                            "focused": false,
                            "rect": {"x": 250, "y": 250, "width": 200, "height": 150}
                        }, {
                            "id": 52, "name": "large-top", "type": "floating_con",
                            "app_id": "imv-large", "window": null, "visible": true,
                            "focused": false,
                            "rect": {"x": 150, "y": 150, "width": 700, "height": 500}
                        }]
                    }]
                }]
            }"#,
        )
        .unwrap();

        let mut rects = Vec::new();
        walk(&tree, &mut rects);
        let ids: Vec<&str> = rects.iter().map(|r| r.app_id.as_str()).collect();
        assert_eq!(ids, vec!["foot", "imv-small", "imv-large"]);
        assert!(rects[2].recency > rects[1].recency && rects[1].recency > rects[0].recency);

        // Inside all three (300, 300): larger top-most floating `52` beats
        // both smaller buried `51` and tiled `40`.
        let hit_overlap = hit_test(&rects, point(px(300.), px(300.))).expect("overlap hit");
        assert_eq!(rects[hit_overlap].app_id, "imv-large");

        // Outside floating windows (50, 50): tiled `40` still matches.
        let hit_tiled = hit_test(&rects, point(px(50.), px(50.))).expect("tiled hit");
        assert_eq!(rects[hit_tiled].app_id, "foot");
    }
}
