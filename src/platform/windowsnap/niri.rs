//! niri backend: `NIRI_SOCKET` → typed window list with geometry.
//!
//! Visibility requires two checks:
//! 1. `Workspace::is_active` — `niri`'s `Layout::with_windows` iterates every
//!    workspace on every monitor, and `tiles_with_ipc_layouts` populates
//!    `tile_pos_in_workspace_view` relative to each workspace's own viewport
//!    even when that workspace is inactive (both for floating windows on 26.04
//!    and for scrolling tiled columns since upstream `niri#4711`).
//! 2. `WindowLayout::tile_pos_in_workspace_view` — `None` when a tiled column
//!    is scrolled completely outside its workspace view (or on niri ≤ 26.04
//!    where tiled positions were not yet exposed). Partially scrolled columns
//!    still report their full `window_size`, so rects are clipped to their
//!    output's `logical` bounds to avoid bleeding across monitor seams.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use niri_ipc::{Output, Reply, Request, Response, Window, Workspace};

use super::{CompositorSnapshot, SnapRect};

pub fn query() -> Option<CompositorSnapshot> {
    let path = std::env::var_os(niri_ipc::socket::SOCKET_PATH_ENV)?;
    let stream = UnixStream::connect(path).ok()?;
    // The query wrapper budgets 1 s on the main thread, but a wedged
    // compositor socket would leak this helper thread forever inside
    // read_line — cap the I/O so the thread can always exit (sway.rs /
    // hyprland.rs do the same). niri-ipc's Socket keeps its stream
    // private with no timeout hook, so the one-JSON-line-each-way
    // protocol is spoken directly here.
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    let mut socket = BufReader::new(stream);
    let windows = match send(&mut socket, Request::Windows)? {
        Reply::Ok(Response::Windows(v)) => v,
        _ => return None,
    };
    let workspaces = match send(&mut socket, Request::Workspaces)? {
        Reply::Ok(Response::Workspaces(v)) => v,
        _ => return None,
    };
    let outputs = match send(&mut socket, Request::Outputs)? {
        Reply::Ok(Response::Outputs(v)) => v,
        _ => return None,
    };
    Some(CompositorSnapshot {
        rects: assemble(&windows, &workspaces, &outputs),
        focused_output: focused_output(&workspaces),
    })
}

fn focused_output(workspaces: &[Workspace]) -> Option<String> {
    workspaces
        .iter()
        .find(|w| w.is_focused)
        .and_then(|w| w.output.clone())
}

/// One niri-IPC round trip: JSON request line → JSON reply line.
fn send(socket: &mut BufReader<UnixStream>, request: Request) -> Option<Reply> {
    let mut buf = serde_json::to_string(&request).ok()?;
    buf.push('\n');
    socket.get_mut().write_all(buf.as_bytes()).ok()?;
    buf.clear();
    socket.read_line(&mut buf).ok()?;
    serde_json::from_str(&buf).ok()
}

/// window list → global logical rects. Pure, unit-tested with JSON
/// fixtures (the niri-ipc types are Deserialize; real `niri msg --json`
/// dumps make exact regression fixtures).
fn assemble(
    windows: &[Window],
    workspaces: &[Workspace],
    outputs: &HashMap<String, Output>,
) -> Vec<SnapRect> {
    let ws_output: HashMap<u64, &str> = workspaces
        .iter()
        .filter(|w| w.is_active)
        .filter_map(|w| Some((w.id, w.output.as_deref()?)))
        .collect();
    windows
        .iter()
        .filter_map(|w| {
            let layout = &w.layout;
            let (vx, vy) = layout.tile_pos_in_workspace_view?;
            let ws_id = w.workspace_id?;
            let logical = outputs.get(*ws_output.get(&ws_id)?)?.logical?;
            let (ww, wh) = layout.window_size;
            if ww <= 0 || wh <= 0 || logical.width == 0 || logical.height == 0 {
                return None; // mid-animation or closing
            }
            let ox = logical.x as f32;
            let oy = logical.y as f32;
            let ow = logical.width as f32;
            let oh = logical.height as f32;
            let raw_x = ox + vx as f32 + layout.window_offset_in_tile.0 as f32;
            let raw_y = oy + vy as f32 + layout.window_offset_in_tile.1 as f32;
            let x0 = raw_x.max(ox);
            let y0 = raw_y.max(oy);
            let x1 = (raw_x + ww as f32).min(ox + ow);
            let y1 = (raw_y + wh as f32).min(oy + oh);
            if x1 <= x0 || y1 <= y0 {
                return None;
            }
            Some(SnapRect::from_xywh(
                x0,
                y0,
                x1 - x0,
                y1 - y0,
                w.app_id.clone().unwrap_or_default(),
                w.is_focused,
                w.focus_timestamp
                    .map(|t| t.secs * 1_000 + u64::from(t.nanos) / 1_000_000)
                    .unwrap_or(0),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{assemble, focused_output};
    use niri_ipc::{Output, Window, Workspace};

    fn win(json: &str) -> Window {
        serde_json::from_str(json).expect("fixture Window")
    }
    fn ws(json: &str) -> Workspace {
        serde_json::from_str(json).expect("fixture Workspace")
    }
    fn out(json: &str) -> (&'static str, Output) {
        let o: Output = serde_json::from_str(json).expect("fixture Output");
        (leak_name(json), o)
    }
    // the output name is inside the JSON; extract without full typing
    fn leak_name(json: &str) -> &'static str {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        Box::leak(
            v["name"]
                .as_str()
                .expect("name")
                .to_string()
                .into_boxed_str(),
        )
    }

    /// A trimmed dump of the real triple-monitor session (2026-09-26):
    /// HDMI-A-1 at (0,0), DP-2 at (-720,-100) scale 1.5 transform 90.
    #[test]
    fn real_session_dump_produces_global_rects() {
        let windows = [
            win(
                r#"{"id":200,"title":"Releases","app_id":"zen","pid":1,"workspace_id":38,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[936.0,1009.0],
                "window_size":[936,1009],"tile_pos_in_workspace_view":[10.0,20.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":{"secs":53865,"nanos":942428172}}"#,
            ),
            // scrolled out of view → must be dropped
            win(
                r#"{"id":201,"title":"hidden","app_id":"zen","pid":1,"workspace_id":39,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[2,1],"tile_size":[936.0,1009.0],
                "window_size":[936,1009],"tile_pos_in_workspace_view":null,
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
            // floating window with an offset inside its tile
            win(
                r#"{"id":202,"title":"float","app_id":"clash-verge","pid":1,"workspace_id":38,
                "is_focused":true,"is_floating":true,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":null,"tile_size":[940.0,700.0],
                "window_size":[940,700],"tile_pos_in_workspace_view":[490.0,210.0],
                "window_offset_in_tile":[5.0,6.0]},"focus_timestamp":{"secs":60000,"nanos":0}}"#,
            ),
        ];
        let workspaces = [
            ws(
                r#"{"id":38,"idx":1,"name":"1","output":"HDMI-A-1","is_urgent":false,
                "is_active":true,"is_focused":true,"active_window_id":202}"#,
            ),
            ws(
                r#"{"id":39,"idx":2,"name":"2","output":"HDMI-A-1","is_urgent":false,
                "is_active":false,"is_focused":false,"active_window_id":null}"#,
            ),
        ];
        let outputs: std::collections::HashMap<_, _> = [
            out(r#"{"name":"HDMI-A-1","make":"","model":"","serial":null,
                "physical_size":[540,300],"modes":[],"current_mode":1,"is_custom_mode":false,
                "vrr_supported":false,"vrr_enabled":false,
                "logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}}"#),
            out(r#"{"name":"DP-2","make":"","model":"","serial":null,
                "physical_size":[350,190],"modes":[],"current_mode":1,"is_custom_mode":false,
                "vrr_supported":false,"vrr_enabled":false,
                "logical":{"x":-720,"y":-100,"width":720,"height":1280,"scale":1.5,"transform":"90"}}"#),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();

        assert_eq!(focused_output(&workspaces).as_deref(), Some("HDMI-A-1"));
        let rects = assemble(&windows, &workspaces, &outputs);
        assert_eq!(rects.len(), 2, "null-view window must be dropped");

        let zen = &rects[0];
        assert_eq!(
            (
                f32::from(zen.bounds.origin.x),
                f32::from(zen.bounds.origin.y)
            ),
            (10., 20.)
        );
        assert_eq!(
            (
                f32::from(zen.bounds.size.width),
                f32::from(zen.bounds.size.height)
            ),
            (936., 1009.)
        );
        assert!(!zen.focused);
        assert_eq!(zen.recency, 53_865_942);

        let float = &rects[1];
        // global = output origin + view pos + window offset in tile
        assert_eq!(
            (
                f32::from(float.bounds.origin.x),
                f32::from(float.bounds.origin.y)
            ),
            (495., 216.)
        );
        assert!(float.focused);
        assert_eq!(float.app_id, "clash-verge");
    }

    #[test]
    fn window_on_unknown_output_or_zero_size_is_dropped() {
        let windows = [
            // workspace → output that does not exist in the outputs map
            win(
                r#"{"id":1,"title":null,"app_id":null,"pid":1,"workspace_id":7,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[10.0,10.0],
                "window_size":[0,0],"tile_pos_in_workspace_view":[0.0,0.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
        ];
        let workspaces = [ws(
            r#"{"id":7,"idx":1,"name":"1","output":"ghost","is_urgent":false,
                "is_active":true,"is_focused":true,"active_window_id":null}"#,
        )];
        let outputs = std::collections::HashMap::new();
        assert!(assemble(&windows, &workspaces, &outputs).is_empty());
    }

    #[test]
    fn inactive_workspace_tiled_and_floating_windows_are_dropped() {
        // Since upstream niri#4711, `tiles_with_ipc_layouts` populates
        // `tile_pos_in_workspace_view` for visible-in-workspace tiled columns
        // across ALL workspaces (active and inactive), just as floating
        // windows already do on 26.04. Only `is_active: true` workspaces
        // (including unfocused secondary monitors) may produce SnapRects.
        let windows = [
            win(
                r#"{"id":10,"title":"inactive-tile","app_id":"foot","pid":1,"workspace_id":39,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[900.0,1000.0],
                "window_size":[900,1000],"tile_pos_in_workspace_view":[16.0,16.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
            win(
                r#"{"id":11,"title":"inactive-float","app_id":"mpv","pid":1,"workspace_id":39,
                "is_focused":false,"is_floating":true,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":null,"tile_size":[640.0,360.0],
                "window_size":[640,360],"tile_pos_in_workspace_view":[100.0,100.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
            win(
                r#"{"id":12,"title":"secondary-active-tile","app_id":"code","pid":1,"workspace_id":40,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[700.0,1200.0],
                "window_size":[700,1200],"tile_pos_in_workspace_view":[10.0,20.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
        ];
        let workspaces = [
            ws(
                r#"{"id":38,"idx":1,"name":"1","output":"HDMI-A-1","is_urgent":false,
                "is_active":true,"is_focused":true,"active_window_id":null}"#,
            ),
            ws(
                r#"{"id":39,"idx":2,"name":"2","output":"HDMI-A-1","is_urgent":false,
                "is_active":false,"is_focused":false,"active_window_id":10}"#,
            ),
            ws(
                r#"{"id":40,"idx":1,"name":"side","output":"DP-2","is_urgent":false,
                "is_active":true,"is_focused":false,"active_window_id":12}"#,
            ),
        ];
        let outputs: std::collections::HashMap<_, _> = [
            out(r#"{"name":"HDMI-A-1","make":"","model":"","serial":null,
                "physical_size":[540,300],"modes":[],"current_mode":1,"is_custom_mode":false,
                "vrr_supported":false,"vrr_enabled":false,
                "logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}}"#),
            out(r#"{"name":"DP-2","make":"","model":"","serial":null,
                "physical_size":[350,190],"modes":[],"current_mode":1,"is_custom_mode":false,
                "vrr_supported":false,"vrr_enabled":false,
                "logical":{"x":-720,"y":-100,"width":720,"height":1280,"scale":1.5,"transform":"90"}}"#),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();

        let rects = assemble(&windows, &workspaces, &outputs);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].app_id, "code");
        assert_eq!(
            (
                f32::from(rects[0].bounds.origin.x),
                f32::from(rects[0].bounds.origin.y),
                f32::from(rects[0].bounds.size.width),
                f32::from(rects[0].bounds.size.height),
            ),
            (-710., -80., 700., 1200.)
        );
    }

    #[test]
    fn partially_scrolled_tiles_clip_to_output_bounds() {
        let windows = [
            // Partially scrolled off the left edge of HDMI-A-1 (adjacent to DP-2 at x=-720..0)
            win(
                r#"{"id":20,"title":"left-bleed","app_id":"zen","pid":1,"workspace_id":38,
                "is_focused":true,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[936.0,1009.0],
                "window_size":[936,1009],"tile_pos_in_workspace_view":[-300.0,20.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
            // Partially scrolled off the right edge of HDMI-A-1
            win(
                r#"{"id":21,"title":"right-bleed","app_id":"foot","pid":1,"workspace_id":38,
                "is_focused":false,"is_floating":false,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":[2,1],"tile_size":[936.0,1009.0],
                "window_size":[936,1009],"tile_pos_in_workspace_view":[1500.0,20.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
            // Floating window pushed completely off-output
            win(
                r#"{"id":22,"title":"offscreen","app_id":"float","pid":1,"workspace_id":38,
                "is_focused":false,"is_floating":true,"is_urgent":false,
                "layout":{"pos_in_scrolling_layout":null,"tile_size":[200.0,200.0],
                "window_size":[200,200],"tile_pos_in_workspace_view":[1950.0,20.0],
                "window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#,
            ),
        ];
        let workspaces = [ws(
            r#"{"id":38,"idx":1,"name":"1","output":"HDMI-A-1","is_urgent":false,
                "is_active":true,"is_focused":true,"active_window_id":20}"#,
        )];
        let outputs: std::collections::HashMap<_, _> = [out(
            r#"{"name":"HDMI-A-1","make":"","model":"","serial":null,
                "physical_size":[540,300],"modes":[],"current_mode":1,"is_custom_mode":false,
                "vrr_supported":false,"vrr_enabled":false,
                "logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}}"#,
        )]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();

        let rects = assemble(&windows, &workspaces, &outputs);
        assert_eq!(rects.len(), 2);
        assert_eq!(
            (
                f32::from(rects[0].bounds.origin.x),
                f32::from(rects[0].bounds.size.width),
            ),
            (0., 636.)
        );
        assert_eq!(
            (
                f32::from(rects[1].bounds.origin.x),
                f32::from(rects[1].bounds.size.width),
            ),
            (1500., 420.)
        );
    }
}
