//! # vptr — virtual pointer injector for interactive e2e tests
//!
//! Drives the pointer through the `zwlr_virtual_pointer_v1` protocol so
//! scripts (and agents) can click real UIs on a live compositor — the
//! missing half of screenshot-tool e2e: `grim` sees the pixels, this
//! pokes them. Built during the pin (贴图) work to reproduce click →
//! Esc flows without a human.
//!
//! ## Usage
//!
//! ```text
//! # global logical coordinates (niri IPC resolves the output layout):
//! vptr gclick  GX GY      # left-click
//! vptr grclick GX GY      # right-click (context menus)
//! vptr gmove   GX GY      # hover / drag-to position
//!
//! # raw protocol mode (no niri needed — see the mapping note below):
//! vptr click   X Y [W H]  # move+left-click
//! vptr rclick  X Y [W H]  # move+right-click
//! vptr move    X Y [W H]  # move only
//! vptr down | up          # press / release the left button
//! vptr scroll N           # N lines (negative scrolls up)
//! ```
//!
//! `down`/`gmove`/`up` chain into a drag. There is **no keyboard
//! injection**: the wlr virtual-pointer protocol only carries pointer
//! events (zwp_virtual_keyboard exists but is a different manager; add
//! it here if a test ever needs keystrokes).
//!
//! ## The coordinate-mapping trap (measured, don't re-derive)
//!
//! `motion_absolute(x, y, x_extent, y_extent)` maps the FRACTIONS
//! `x/x_extent`, `y/y_extent` onto the compositor's whole pointer
//! space — **the union of all outputs**, not the output you are looking
//! at. On a 3-output niri desktop spanning x ∈ [-720, 3456] (4176 px),
//! passing `x=810, x_extent=1920` lands at global
//! `-720 + 810/1920 · 4176 = 1041.9`, not at 810. The `g*` commands
//! hide this: they ask niri for the output layout (`Request::Outputs`,
//! same socket as the window-snap backend), take the union, and
//! convert a global target to the fraction pair. Use them.
//!
//! ## Shotori e2e pattern
//!
//! ```sh
//! # pin at a known rect, then exercise it headlessly:
//! SHOTORI_DEBUG_TARGET=HDMI-A-1 SHOTORI_DEBUG_SELECTION=560,280,500,350 \
//!     SHOTORI_DEBUG_ACTION=pin target/release/shotori &
//! sleep 3
//! target/../tools/vptr/target/release/vptr grclick 810 455   # open the pin menu
//! grim -o HDMI-A-1 /tmp/proof.png                            # then inspect the pixels
//! tools/vptr/target/release/vptr gclick 890 534              # menu: Close
//! ```
//!
//! (Build the tool with `cargo build --release -p vptr`.)

use std::thread::sleep;
use std::time::Duration;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Queue state: nothing to track — globals arrive pre-bound.
struct App;

macro_rules! noop_dispatch {
    ($($t:ty),* $(,)?) => { $(
        impl Dispatch<$t, ()> for App {
            fn event(
                _: &mut Self,
                _: &$t,
                _: <$t as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )* };
}
noop_dispatch!(WlSeat, ZwlrVirtualPointerManagerV1, ZwlrVirtualPointerV1);

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first().cloned() else {
        usage_and_exit();
    };
    // num(0) = first argument after the mode
    let num = |i: usize| -> f64 {
        args.get(i + 1)
            .and_then(|a| a.parse().ok())
            .unwrap_or(f64::NAN)
    };

    // Global modes resolve through niri first; everything funnels into
    // the raw (x, y, extent) path below.
    let (x, y, w, h, button) = match mode.as_str() {
        "gclick" | "grclick" | "gmove" => {
            let Some((gx, gy, gw, gh)) = niri_desktop_union() else {
                eprintln!(
                    "vptr: niri IPC unavailable (NIRI_SOCKET / `niri msg`) — use the raw modes"
                );
                std::process::exit(3);
            };
            let x = num(0) - gx;
            let y = num(1) - gy;
            if !(x >= 0. && y >= 0. && x < gw && y < gh) {
                eprintln!(
                    "vptr: target ({}, {}) is outside the desktop {gx}+{gw} × {gy}+{gh}",
                    num(0),
                    num(1)
                );
                std::process::exit(4);
            }
            (
                x.round() as u32,
                y.round() as u32,
                gw.round() as u32,
                gh.round() as u32,
                match mode.as_str() {
                    "gclick" => Some(BTN_LEFT),
                    "grclick" => Some(BTN_RIGHT),
                    _ => None,
                },
            )
        }
        "click" | "rclick" | "move" => (
            num(0).round() as u32,
            num(1).round() as u32,
            if num(2).is_nan() {
                1920
            } else {
                num(2).round() as u32
            }
            .max(1),
            if num(3).is_nan() {
                1080
            } else {
                num(3).round() as u32
            }
            .max(1),
            match mode.as_str() {
                "click" => Some(BTN_LEFT),
                "rclick" => Some(BTN_RIGHT),
                _ => None,
            },
        ),
        "down" | "up" | "scroll" | "scrollloop" => (0, 0, 1, 1, None),
        _ => usage_and_exit(),
    };

    let conn = Connection::connect_to_env().expect("connect to $WAYLAND_DISPLAY");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("wayland registry");
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).expect("wl_seat");
    let mgr: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("zwlr_virtual_pointer_manager_v1 (compositor support missing?)");
    let ptr = mgr.create_virtual_pointer(Some(&seat), &qh, ());
    let mut app = App;
    queue.roundtrip(&mut app).expect("roundtrip");

    match mode.as_str() {
        "scrollloop" => {
            // Diagnosis mode: N lines, M times, 250ms apart — one client,
            // repeated gestures. If only the first lands, the compositor
            // accumulates axis events per client connection.
            let times = (num(1) as i64).clamp(1, 50) as u32;
            for k in 0..times {
                ptr.axis(2000 + k * 250, Axis::VerticalScroll, num(0) * -15.0);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                sleep(Duration::from_millis(250));
            }
        }
        "down" => {
            ptr.button(1000, BTN_LEFT, ButtonState::Pressed);
            ptr.frame();
        }
        "up" => {
            ptr.button(1100, BTN_LEFT, ButtonState::Released);
            ptr.frame();
        }
        "scroll" => {
            ptr.axis(1200, Axis::VerticalScroll, num(0) * -15.0);
            ptr.frame();
        }
        _ => {
            ptr.motion_absolute(1000, x, y, w, h);
            ptr.frame();
            queue.roundtrip(&mut app).expect("roundtrip");
            sleep(Duration::from_millis(120));
            if let Some(btn) = button {
                ptr.button(1050, btn, ButtonState::Pressed);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                sleep(Duration::from_millis(90));
                ptr.button(1100, btn, ButtonState::Released);
                ptr.frame();
            }
        }
    }
    queue.roundtrip(&mut app).expect("roundtrip");
}

/// The union of all niri outputs' logical rects: (x, y, w, h). This is
/// the space `motion_absolute` fractions map onto (see the module docs).
fn niri_desktop_union() -> Option<(f64, f64, f64, f64)> {
    let mut socket = niri_ipc::socket::Socket::connect().ok()?;
    let outputs = match socket.send(niri_ipc::Request::Outputs).ok()? {
        niri_ipc::Reply::Ok(niri_ipc::Response::Outputs(v)) => v,
        _ => return None,
    };
    let mut logical = outputs
        .values()
        .filter_map(|o| o.logical.as_ref())
        .map(|l| (l.x as f64, l.y as f64, l.width as f64, l.height as f64));
    let (mut x0, mut y0, mut x1, mut y1) = {
        let (x, y, w, h) = logical.next()?;
        (x, y, x + w, y + h)
    };
    for (x, y, w, h) in logical {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + w);
        y1 = y1.max(y + h);
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

fn usage_and_exit() -> ! {
    eprintln!(
        "usage: vptr gclick|grclick|gmove GX GY   (global logical coords, via niri IPC)\n\
         \x20      vptr click|rclick|move X Y [W H]  (raw fraction mode)\n\
         \x20      vptr down | up                    (left button press/release)\n\
         \x20      vptr scroll N                     (lines; negative = up)"
    );
    std::process::exit(2);
}
