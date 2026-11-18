//! # vptr — virtual pointer + keyboard injector for interactive e2e tests
//!
//! Drives the pointer through the `zwlr_virtual_pointer_v1` protocol and
//! keystrokes through `zwp_virtual_keyboard_v1`, so scripts (and agents)
//! can drive real UIs on a live compositor — the missing half of
//! screenshot-tool e2e: `grim` sees the pixels, this pokes them. Built
//! during the pin (贴图) work to reproduce click → Esc flows without a
//! human; the keyboard half landed with the configurable-shortcuts
//! feature (the settings recorder only reacts to real key events).
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
//!
//! # keyboard & interactive session (raw protocol mode; chord in gpui syntax):
//! vptr key CHORD          # one-shot injection (attaches a keyboard,
//!                         # sends, detaches)
//! vptr keep [W H]         # hold a PERSISTENT virtual pointer+keyboard and
//!                         # execute pointer/keyboard commands from stdin
//!                         # (default desktop extent W×H = 1920×1080):
//!                         #   move X Y [W H]
//!                         #   click | dclick | rclick X Y [W H]
//!                         #   drag X1 Y1 X2 Y2 [STEPS]
//!                         #   down [left|right] | up [left|right]
//!                         #   scroll | hscroll LINES [STEPS]
//!                         #   mods shift|ctrl|alt|super|shift+alt|none
//!                         #   key CHORD | CHORD
//!                         #   extent W H | sleep MS
//! ```
//!
//! Why `keep` is mandatory for headless drags and keyboard focus:
//! 1. Compositors derive `wl_seat` capabilities from attached devices;
//!    starting `vptr keep` BEFORE launching shotori raises `seat0` from
//!    `capabilities: 0` to `3` (`keyboard | pointer`), so layer-shell
//!    overlays receive real pointer & keyboard focus.
//! 2. Short-lived `vptr down` processes destroy their `zwlr_virtual_pointer_v1`
//!    on exit, causing wlroots to auto-release held buttons before the next
//!    `vptr move` starts. Driving `drag`/`down`/`move`/`up` over `vptr keep`'s
//!    single persistent pointer preserves button grabs across frames.
//!
//! ## Headless sway recipe (a zero-footprint interactive e2e desktop)
//!
//! ```sh
//! WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=vulkan \
//!     sway -c sway.conf &      # renders to memory: no window anywhere
//! mkfifo input.fifo
//! vptr keep 1600 1000 < input.fifo &
//! exec 3>input.fifo            # keep FIFO writer open across commands
//! shotori &
//! sleep 3
//! printf "drag 200 150 960 620\nkey r\ndrag 320 240 720 480\nscroll 5 3\n" >&3
//! grim -o HEADLESS-1 out.png   # inspect real GPU pixels
//! ```
//! Start the keeper BEFORE launching shotori: the capability timeline
//! is what breaks, not the devices themselves.
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
use wayland_client::protocol::wl_keyboard::KeyState;
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// Generated bindings for `zwp_virtual_keyboard_unstable_v1` (see
/// protocols/virtual-keyboard-unstable-v1.xml — no crates.io protocol
/// crate packages this one). Module shape mirrors wayland-protocols'
/// own `wayland_protocol!` macro: glob the core protocol in, generate
/// interface tables against ITS `__interfaces` re-exports, then the
/// client API.
pub mod vkbd {
    #![allow(dead_code, non_camel_case_types, non_snake_case, unused_variables)]
    #![allow(non_upper_case_globals, unused_imports, missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/virtual-keyboard-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/virtual-keyboard-unstable-v1.xml");

    pub use zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
    pub use zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
}

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
noop_dispatch!(
    vkbd::ZwpVirtualKeyboardManagerV1,
    vkbd::ZwpVirtualKeyboardV1
);

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
    // Keyboard path is fully separate: a virtual keyboard, no pointer.
    if mode == "key" {
        let Some(chord) = args.get(1) else {
            usage_and_exit();
        };
        key_inject(chord);
        return;
    }
    // Device-keeper: hold the connection open so the seat keeps its
    // pointer+keyboard capabilities, and serve pointer/key commands from stdin.
    if mode == "keep" {
        let w = args
            .get(1)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1920)
            .max(1);
        let h = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1080)
            .max(1);
        keep_devices(w, h);
        return;
    }
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
            // repeated gestures.
            let times = (num(1) as i64).clamp(1, 50) as u32;
            let value = num(0) * -15.0;
            let discrete =
                (-num(0).round() as i32).signum() * (num(0).abs().round().max(1.0) as i32);
            for k in 0..times {
                ptr.axis_source(AxisSource::Wheel);
                ptr.axis_discrete(2000 + k * 250, Axis::VerticalScroll, value, discrete);
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
            let value = num(0) * -15.0;
            let discrete =
                (-num(0).round() as i32).signum() * (num(0).abs().round().max(1.0) as i32);
            ptr.axis_source(AxisSource::Wheel);
            ptr.axis_discrete(1200, Axis::VerticalScroll, value, discrete);
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

/// Inject one chord (gpui keystroke syntax: `ctrl-shift-f1`) through a
/// virtual keyboard: press the modifiers, press+release the key,
/// release the modifiers. The keymap is a system-include XKB string —
/// the COMPOSITOR parses it with its own libxkbcommon and system xkb
/// data, so no keymap tables live here; only the evdev codes below.
fn key_inject(chord: &str) {
    let conn = Connection::connect_to_env().expect("connect to $WAYLAND_DISPLAY");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("wayland registry");
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).expect("wl_seat");
    let mgr: vkbd::ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("zwp_virtual_keyboard_manager_v1 (compositor support missing?)");
    let kb = mgr.create_virtual_keyboard(&seat, &qh, ());

    // The protocol wants the keymap as an fd; a temp file is the
    // dependency-free way to get one. Compositor-side parsing makes
    // the include-based map as complete as the system's xkb data.
    let keymap = "xkb_keymap {\n\
                  \x20 xkb_keycodes { include \"evdev\" };\n\
                  \x20 xkb_types { include \"complete\" };\n\
                  \x20 xkb_compatibility { include \"complete\" };\n\
                  \x20 xkb_symbols { include \"pc+us\" };\n\
                  };";
    let path = std::env::temp_dir().join("vptr-keymap.xkb");
    std::fs::write(&path, keymap).expect("write keymap temp file");
    use std::os::fd::AsRawFd as _;
    let file = std::fs::File::open(&path).expect("reopen keymap temp file");
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(file.as_raw_fd()) };
    kb.keymap(
        wayland_client::protocol::wl_keyboard::KeymapFormat::XkbV1,
        fd,
        keymap.len() as u32,
    );
    let mut app = App;
    queue.roundtrip(&mut app).expect("roundtrip");
    inject_chord(&kb, &mut queue, &mut app, chord);
    drop(std::fs::remove_file(&path));
}

#[derive(Debug, PartialEq)]
enum KeepCommand {
    Extent {
        w: u32,
        h: u32,
    },
    Sleep(u64),
    Move {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    Click {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        button: u32,
        count: u32,
    },
    Button {
        button: u32,
        pressed: bool,
    },
    Drag {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        w: u32,
        h: u32,
        steps: u32,
    },
    Scroll {
        axis: Axis,
        lines: f64,
        steps: u32,
    },
    Mods {
        codes: Vec<u32>,
        mask: u32,
    },
    Key(String),
}

fn parse_button_name(token: Option<&str>) -> Result<u32, String> {
    match token.unwrap_or("left") {
        "left" => Ok(BTN_LEFT),
        "right" => Ok(BTN_RIGHT),
        other => Err(format!("unknown button {other:?} (expected left|right)")),
    }
}

fn parse_mods_spec(spec: &str) -> Result<(Vec<u32>, u32), String> {
    let trimmed = spec.trim().to_lowercase();
    if trimmed == "none" || trimmed == "off" || trimmed == "clear" {
        return Ok((Vec::new(), 0));
    }
    let mut codes = Vec::new();
    let mut mask = 0;
    for part in trimmed
        .split(['+', '-', ','])
        .filter(|part| !part.is_empty())
    {
        if !is_modifier(part) {
            return Err(format!(
                "unknown modifier {part:?} (expected shift|ctrl|alt|super|none)"
            ));
        }
        let bit = match part {
            "shift" => 1,
            "ctrl" => 4,
            "alt" => 8,
            _ => 64,
        };
        if mask & bit == 0 {
            mask |= bit;
            if let Some(code) = key_code(part) {
                codes.push(code);
            }
        }
    }
    Ok((codes, mask))
}

fn parse_xy_extent(
    tokens: &[&str],
    default_w: u32,
    default_h: u32,
) -> Result<(u32, u32, u32, u32), String> {
    if tokens.len() != 2 && tokens.len() != 4 {
        return Err("expected X Y [W H]".into());
    }
    let x = tokens[0]
        .parse::<f64>()
        .map_err(|_| format!("bad X {:?}", tokens[0]))?;
    let y = tokens[1]
        .parse::<f64>()
        .map_err(|_| format!("bad Y {:?}", tokens[1]))?;
    let (w, h) = if tokens.len() == 4 {
        let w = tokens[2]
            .parse::<u32>()
            .map_err(|_| format!("bad W {:?}", tokens[2]))?
            .max(1);
        let h = tokens[3]
            .parse::<u32>()
            .map_err(|_| format!("bad H {:?}", tokens[3]))?
            .max(1);
        (w, h)
    } else {
        (default_w.max(1), default_h.max(1))
    };
    Ok((x.max(0.).round() as u32, y.max(0.).round() as u32, w, h))
}

fn parse_keep_command(
    line: &str,
    default_w: u32,
    default_h: u32,
) -> Result<Option<KeepCommand>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let head = tokens[0];
    let rest = &tokens[1..];
    match head {
        "extent" => {
            if rest.len() != 2 {
                return Err("usage: extent W H".into());
            }
            let w = rest[0]
                .parse::<u32>()
                .map_err(|_| format!("bad W {:?}", rest[0]))?
                .max(1);
            let h = rest[1]
                .parse::<u32>()
                .map_err(|_| format!("bad H {:?}", rest[1]))?
                .max(1);
            Ok(Some(KeepCommand::Extent { w, h }))
        }
        "sleep" => {
            if rest.len() != 1 {
                return Err("usage: sleep MS".into());
            }
            let ms = rest[0]
                .parse::<u64>()
                .map_err(|_| format!("bad MS {:?}", rest[0]))?;
            Ok(Some(KeepCommand::Sleep(ms)))
        }
        "move" => {
            let (x, y, w, h) = parse_xy_extent(rest, default_w, default_h)?;
            Ok(Some(KeepCommand::Move { x, y, w, h }))
        }
        "click" | "dclick" | "rclick" => {
            let (x, y, w, h) = parse_xy_extent(rest, default_w, default_h)?;
            let (button, count) = match head {
                "rclick" => (BTN_RIGHT, 1),
                "dclick" => (BTN_LEFT, 2),
                _ => (BTN_LEFT, 1),
            };
            Ok(Some(KeepCommand::Click {
                x,
                y,
                w,
                h,
                button,
                count,
            }))
        }
        "down" | "up" if rest.len() <= 1 => {
            // Bare `down` / `up` on its own line is an arrow key in legacy
            // chord streams only if someone meant a key; require `key up` /
            // `key down` for arrow keys and treat `down [left|right]` / `up`
            // as pointer buttons, matching the CLI subcommands.
            let button = parse_button_name(rest.first().copied())?;
            Ok(Some(KeepCommand::Button {
                button,
                pressed: head == "down",
            }))
        }
        "drag" => {
            if rest.len() != 4 && rest.len() != 5 {
                return Err("usage: drag X1 Y1 X2 Y2 [STEPS]".into());
            }
            let x1 = rest[0]
                .parse::<f64>()
                .map_err(|_| format!("bad X1 {:?}", rest[0]))?;
            let y1 = rest[1]
                .parse::<f64>()
                .map_err(|_| format!("bad Y1 {:?}", rest[1]))?;
            let x2 = rest[2]
                .parse::<f64>()
                .map_err(|_| format!("bad X2 {:?}", rest[2]))?;
            let y2 = rest[3]
                .parse::<f64>()
                .map_err(|_| format!("bad Y2 {:?}", rest[3]))?;
            let steps = if let Some(s) = rest.get(4) {
                s.parse::<u32>()
                    .map_err(|_| format!("bad STEPS {s:?}"))?
                    .clamp(1, 256)
            } else {
                16
            };
            Ok(Some(KeepCommand::Drag {
                x1,
                y1,
                x2,
                y2,
                w: default_w.max(1),
                h: default_h.max(1),
                steps,
            }))
        }
        "scroll" | "hscroll" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err("usage: scroll|hscroll LINES [STEPS]".into());
            }
            let lines = rest[0]
                .parse::<f64>()
                .map_err(|_| format!("bad LINES {:?}", rest[0]))?;
            let steps = if let Some(s) = rest.get(1) {
                s.parse::<u32>()
                    .map_err(|_| format!("bad STEPS {s:?}"))?
                    .clamp(1, 200)
            } else {
                1
            };
            let axis = if head == "hscroll" {
                Axis::HorizontalScroll
            } else {
                Axis::VerticalScroll
            };
            Ok(Some(KeepCommand::Scroll { axis, lines, steps }))
        }
        "mods" => {
            if rest.len() != 1 {
                return Err("usage: mods shift|ctrl|alt|super|shift+alt|none".into());
            }
            let (codes, mask) = parse_mods_spec(rest[0])?;
            Ok(Some(KeepCommand::Mods { codes, mask }))
        }
        "key" => {
            if rest.len() != 1 {
                return Err("usage: key CHORD".into());
            }
            Ok(Some(KeepCommand::Key(rest[0].to_string())))
        }
        _ if rest.is_empty() => Ok(Some(KeepCommand::Key(head.to_string()))),
        other => Err(format!("unknown command {other:?}")),
    }
}

/// Hold a virtual pointer + keyboard open until stdin closes, executing
/// pointer gestures and keyboard chords on the SAME persistent devices.
/// Compositors derive seat capabilities from attached devices; a headless
/// session that starts device-less leaves clients without focus paths, and
/// short-lived `vptr down` invocations lose button grabs when their process
/// exits. Keeping both devices alive for the whole session fixes both.
fn keep_devices(mut extent_w: u32, mut extent_h: u32) {
    let conn = Connection::connect_to_env().expect("connect to $WAYLAND_DISPLAY");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("wayland registry");
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).expect("wl_seat");
    let pointer_mgr: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("zwlr_virtual_pointer_manager_v1 (compositor support missing?)");
    let keyboard_mgr: vkbd::ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("zwp_virtual_keyboard_manager_v1 (compositor support missing?)");
    let ptr = pointer_mgr.create_virtual_pointer(Some(&seat), &qh, ());
    let kb = keyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
    // A keymap is required before the compositor treats the keyboard as
    // usable; the include-based map resolves against the system xkb data
    let keymap = "xkb_keymap {\n\
                  \x20 xkb_keycodes { include \"evdev\" };\n\
                  \x20 xkb_types { include \"complete\" };\n\
                  \x20 xkb_compatibility { include \"complete\" };\n\
                  \x20 xkb_symbols { include \"pc+us\" };\n\
                  };";
    let path = std::env::temp_dir().join(format!("vptr-keep-keymap-{}.xkb", std::process::id()));
    std::fs::write(&path, keymap).expect("write keymap temp file");
    use std::os::fd::AsRawFd as _;
    let file = std::fs::File::open(&path).expect("reopen keymap temp file");
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(file.as_raw_fd()) };
    kb.keymap(
        wayland_client::protocol::wl_keyboard::KeymapFormat::XkbV1,
        fd,
        keymap.len() as u32,
    );
    drop(std::fs::remove_file(&path));
    let mut app = App;
    queue.roundtrip(&mut app).expect("roundtrip");
    eprintln!("vptr: keeper ready ({extent_w}x{extent_h})");

    let mut now_ms: u32 = 1000;
    let mut held_mods: (Vec<u32>, u32) = (Vec::new(), 0);
    let advance = |now: &mut u32, ms: u64| {
        if ms > 0 {
            sleep(Duration::from_millis(ms));
        }
        *now = now.wrapping_add(ms.max(1) as u32);
    };

    use std::io::BufRead as _;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let cmd = match parse_keep_command(&line, extent_w, extent_h) {
            Ok(Some(cmd)) => cmd,
            Ok(None) => continue,
            Err(err) => {
                eprintln!("vptr: {err} in {line:?}");
                continue;
            }
        };
        match cmd {
            KeepCommand::Extent { w, h } => {
                extent_w = w;
                extent_h = h;
            }
            KeepCommand::Sleep(ms) => advance(&mut now_ms, ms),
            KeepCommand::Move { x, y, w, h } => {
                ptr.motion_absolute(now_ms, x, y, w, h);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 16);
            }
            KeepCommand::Click {
                x,
                y,
                w,
                h,
                button,
                count,
            } => {
                ptr.motion_absolute(now_ms, x, y, w, h);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 50);
                for n in 0..count {
                    if n > 0 {
                        advance(&mut now_ms, 70);
                    }
                    ptr.button(now_ms, button, ButtonState::Pressed);
                    ptr.frame();
                    queue.roundtrip(&mut app).expect("roundtrip");
                    advance(&mut now_ms, 45);
                    ptr.button(now_ms, button, ButtonState::Released);
                    ptr.frame();
                    queue.roundtrip(&mut app).expect("roundtrip");
                }
                advance(&mut now_ms, 20);
            }
            KeepCommand::Button { button, pressed } => {
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                ptr.button(now_ms, button, state);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 20);
            }
            KeepCommand::Drag {
                x1,
                y1,
                x2,
                y2,
                w,
                h,
                steps,
            } => {
                let clamp_x = |v: f64| (v.max(0.).round() as u32).min(w.saturating_sub(1));
                let clamp_y = |v: f64| (v.max(0.).round() as u32).min(h.saturating_sub(1));
                ptr.motion_absolute(now_ms, clamp_x(x1), clamp_y(y1), w, h);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 35);
                ptr.button(now_ms, BTN_LEFT, ButtonState::Pressed);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 20);
                for step in 1..=steps {
                    let t = f64::from(step) / f64::from(steps);
                    let x = clamp_x(x1 + (x2 - x1) * t);
                    let y = clamp_y(y1 + (y2 - y1) * t);
                    ptr.motion_absolute(now_ms, x, y, w, h);
                    ptr.frame();
                    queue.roundtrip(&mut app).expect("roundtrip");
                    advance(&mut now_ms, 12);
                }
                advance(&mut now_ms, 25);
                ptr.button(now_ms, BTN_LEFT, ButtonState::Released);
                ptr.frame();
                queue.roundtrip(&mut app).expect("roundtrip");
                advance(&mut now_ms, 25);
            }
            KeepCommand::Scroll { axis, lines, steps } => {
                // wlroots drops `AxisSource::Wheel` frames that lack discrete
                // steps (`delta_discrete == 0`). `axis_discrete` populates both
                // `value` and `discrete` in one request; fractional sub-line
                // scrolls fall back to `AxisSource::Continuous` + `axis`.
                let value = lines * -15.0;
                let discrete = -lines.round() as i32;
                for step in 0..steps {
                    if step > 0 {
                        advance(&mut now_ms, 25);
                    }
                    if discrete != 0 {
                        ptr.axis_source(AxisSource::Wheel);
                        ptr.axis_discrete(now_ms, axis, value, discrete);
                    } else {
                        ptr.axis_source(AxisSource::Continuous);
                        ptr.axis(now_ms, axis, value);
                    }
                    ptr.frame();
                    queue.roundtrip(&mut app).expect("roundtrip");
                    advance(&mut now_ms, 15);
                }
            }
            KeepCommand::Mods { codes, mask } => {
                for old in held_mods.0.iter().rev() {
                    kb.key(now_ms, *old, KeyState::Released);
                }
                kb.modifiers(0, 0, 0, 0);
                advance(&mut now_ms, 10);
                for &code in &codes {
                    kb.key(now_ms, code, KeyState::Pressed);
                }
                kb.modifiers(mask, 0, 0, 0);
                queue.roundtrip(&mut app).expect("roundtrip");
                held_mods = (codes, mask);
                advance(&mut now_ms, 15);
            }
            KeepCommand::Key(chord) => {
                inject_chord(&kb, &mut queue, &mut app, &chord);
                if held_mods.1 != 0 {
                    for &code in &held_mods.0 {
                        kb.key(now_ms, code, KeyState::Pressed);
                    }
                    kb.modifiers(held_mods.1, 0, 0, 0);
                    queue.roundtrip(&mut app).expect("roundtrip");
                }
                advance(&mut now_ms, 20);
            }
        }
    }
}

/// Send one chord through the given (already keymapped) virtual
/// keyboard: modifiers request + modifier keys, the key, then release.
fn inject_chord(
    kb: &vkbd::ZwpVirtualKeyboardV1,
    queue: &mut wayland_client::EventQueue<App>,
    app: &mut App,
    chord: &str,
) {
    let Some((mods, mask, key)) = parse_chord(chord) else {
        eprintln!("vptr: unknown key or modifier in chord {chord:?}");
        return;
    };
    // The wire carries RAW EVDEV codes: wlroots applies the +8 into
    // libxkbcommon keycode space itself (measured: an evdev-21 "y"
    // sent as 29 landed as "k", exactly one +8 off). Modifiers go
    // through the protocol's own `modifiers` request — pressing the
    // modifier keys alone raced the client's modifiers event and the
    // chord arrived without ctrl (also measured); the explicit
    // depressed-mask request is deterministic.
    for m in &mods {
        kb.key(1000, *m, KeyState::Pressed);
    }
    if mask != 0 {
        kb.modifiers(mask, 0, 0, 0);
    }
    queue.roundtrip(app).expect("roundtrip");
    sleep(Duration::from_millis(120));
    kb.key(1100, key, KeyState::Pressed);
    queue.roundtrip(app).expect("roundtrip");
    sleep(Duration::from_millis(90));
    kb.key(1150, key, KeyState::Released);
    if mask != 0 {
        kb.modifiers(0, 0, 0, 0);
    }
    queue.roundtrip(app).expect("roundtrip");
    for m in mods.iter().rev() {
        kb.key(1200, *m, KeyState::Released);
    }
    queue.roundtrip(app).expect("roundtrip");
}

/// `ctrl-shift-f1` → ([ctrl code, shift code], f1 code). Modifier
/// names match gpui's keystroke prefixes; the last segment is the key.
/// Codes come out as evdev numbers (+8 into keymap space at the call
/// site) so the injection path can't re-fail on a name.
fn parse_chord(chord: &str) -> Option<(Vec<u32>, u32, u32)> {
    let parts: Vec<&str> = chord.split('-').filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts.split_last()?;
    let key = key_code(key)?;
    let mut codes = Vec::new();
    let mut mask = 0;
    for m in mods {
        if !is_modifier(m) {
            return None;
        }
        // wlroots' depressed-mask bit for each modifier (wlr_keyboard:
        // shift=1, caps=2, ctrl=4, alt=8, mod2=16, mod3=32, super=64)
        mask |= match *m {
            "shift" => 1,
            "ctrl" => 4,
            "alt" => 8,
            _ => 64, // "super" — is_modifier() filtered the rest
        };
        codes.push(key_code(m)?);
    }
    Some((codes, mask, key))
}

fn is_modifier(name: &str) -> bool {
    matches!(name, "ctrl" | "alt" | "shift" | "super")
}

/// evdev keycode for a key name (the +8 into libxkbcommon space
/// happens at the call site). Letters and digits are contiguous runs;
/// the rest is the sparse table e2e chords actually need.
fn key_code(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u32>()
    {
        return match n {
            1..=10 => Some(58 + n), // F1=59 .. F10=68
            11 => Some(87),
            12 => Some(88),
            _ => None,
        };
    }
    Some(match lower.as_str() {
        "escape" | "esc" => 1,
        "1" => 2,
        "2" => 3,
        "3" => 4,
        "4" => 5,
        "5" => 6,
        "6" => 7,
        "7" => 8,
        "8" => 9,
        "9" => 10,
        "0" => 11,
        "minus" => 12,
        "equal" => 13,
        "backspace" => 14,
        "tab" => 15,
        "q" => 16,
        "w" => 17,
        "e" => 18,
        "r" => 19,
        "t" => 20,
        "y" => 21,
        "u" => 22,
        "i" => 23,
        "o" => 24,
        "p" => 25,
        "leftbracket" | "[" => 26,
        "rightbracket" | "]" => 27,
        "enter" | "return" => 28,
        "ctrl" => 29,
        "a" => 30,
        "s" => 31,
        "d" => 32,
        "f" => 33,
        "g" => 34,
        "h" => 35,
        "j" => 36,
        "k" => 37,
        "l" => 38,
        "semicolon" | ";" => 39,
        "apostrophe" | "'" => 40,
        "grave" | "`" => 41,
        "shift" => 42,
        "backslash" | "\\" => 43,
        "z" => 44,
        "x" => 45,
        "c" => 46,
        "v" => 47,
        "b" => 48,
        "n" => 49,
        "m" => 50,
        "comma" | "," => 51,
        "dot" | "." => 52,
        "slash" | "/" => 53,
        "alt" => 56,
        "space" => 57,
        "capslock" => 58,
        "home" => 102,
        "up" => 103,
        "pageup" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "pagedown" => 109,
        "insert" => 110,
        "delete" => 111,
        "super" | "cmd" | "win" => 125,
        _ => return None,
    })
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
         \x20      vptr scroll N                     (lines; negative = up)\n\
         \x20      vptr key CHORD                    (e.g. ctrl-y, escape, ctrl-shift-f1)\n\
         \x20      vptr keep [W H]                   (persistent pointer+keyboard stdin session)"
    );
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_parser_skips_comments_and_blank_lines() {
        assert_eq!(parse_keep_command("", 1600, 1000), Ok(None));
        assert_eq!(parse_keep_command("   # comment", 1600, 1000), Ok(None));
    }

    #[test]
    fn keep_parser_covers_pointer_drag_scroll_mods_and_legacy_chords() {
        assert_eq!(
            parse_keep_command("click 120 340", 1600, 1000),
            Ok(Some(KeepCommand::Click {
                x: 120,
                y: 340,
                w: 1600,
                h: 1000,
                button: BTN_LEFT,
                count: 1,
            }))
        );
        assert_eq!(
            parse_keep_command("dclick 50 60 800 600", 1600, 1000),
            Ok(Some(KeepCommand::Click {
                x: 50,
                y: 60,
                w: 800,
                h: 600,
                button: BTN_LEFT,
                count: 2,
            }))
        );
        assert_eq!(
            parse_keep_command("rclick 10 20", 1600, 1000),
            Ok(Some(KeepCommand::Click {
                x: 10,
                y: 20,
                w: 1600,
                h: 1000,
                button: BTN_RIGHT,
                count: 1,
            }))
        );
        assert_eq!(
            parse_keep_command("drag 100 120 500 420 24", 1600, 1000),
            Ok(Some(KeepCommand::Drag {
                x1: 100.0,
                y1: 120.0,
                x2: 500.0,
                y2: 420.0,
                w: 1600,
                h: 1000,
                steps: 24,
            }))
        );
        assert_eq!(
            parse_keep_command("scroll -3 4", 1600, 1000),
            Ok(Some(KeepCommand::Scroll {
                axis: Axis::VerticalScroll,
                lines: -3.0,
                steps: 4,
            }))
        );
        assert_eq!(
            parse_keep_command("mods shift+alt", 1600, 1000),
            Ok(Some(KeepCommand::Mods {
                codes: vec![42, 56],
                mask: 9,
            }))
        );
        assert_eq!(
            parse_keep_command("mods none", 1600, 1000),
            Ok(Some(KeepCommand::Mods {
                codes: Vec::new(),
                mask: 0,
            }))
        );
        assert_eq!(
            parse_keep_command("ctrl-y", 1600, 1000),
            Ok(Some(KeepCommand::Key("ctrl-y".into())))
        );
        assert_eq!(
            parse_keep_command("key down", 1600, 1000),
            Ok(Some(KeepCommand::Key("down".into())))
        );
    }

    #[test]
    fn keep_parser_rejects_malformed_lines_cleanly() {
        assert!(parse_keep_command("drag 1 2 3", 1600, 1000).is_err());
        assert!(parse_keep_command("click nope 10", 1600, 1000).is_err());
        assert!(parse_keep_command("mods bogus", 1600, 1000).is_err());
        assert!(parse_keep_command("unknown two words", 1600, 1000).is_err());
    }
}
