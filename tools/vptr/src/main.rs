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
//! # keyboard (raw protocol mode; chord in gpui keystroke syntax):
//! vptr key CHORD          # one-shot injection (attaches a keyboard,
//!                         # sends, detaches)
//! vptr keep [CHORD...]    # hold a virtual pointer+keyboard forever and
//!                         # inject one chord per stdin line — a
//!                         # PERSISTENT device: compositors compute seat
//!                         # capabilities from attached devices, and
//!                         # sway (for one) won't give a layer surface
//!                         # keyboard focus when its seat had no
//!                         # keyboard at surface-map time; the stdin
//!                         # server also avoids the attach/detach churn
//!                         # per chord that crashes gpui's keyboard
//!                         # state (upstream unwrap, measured)
//! ```
//!
//! `down`/`gmove`/`up` chain into a drag.
//!
//! ## Headless sway recipe (a zero-footprint e2e desktop)
//!
//! ```sh
//! WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
//!     sway -c empty-config &   # renders to memory: no window anywhere
//! mkfifo keys.fifo
//! vptr keep < keys.fifo &      # PERSISTENT devices — sway only grants
//!                              # layer-surface keyboard focus once the
//!                              # seat has them (and per-chord attach/
//!                              # detach trips an upstream gpui unwrap)
//! echo "ctrl-y" > keys.fifo    # inject chords into the session
//! grim out.png                 # inspect the pixels
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
use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
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
    // pointer+keyboard capabilities, and serve chords from stdin (see
    // the `keep` docs above)
    if mode == "keep" {
        keep_devices();
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

/// Hold a virtual pointer + keyboard open until stdin closes, injecting
/// one chord per stdin line. Compositors derive seat capabilities from
/// attached devices; a headless session that starts device-less leaves
/// clients without keyboard focus paths even after devices come and go
/// (sway measured: layer surfaces mapped before the first keyboard
/// never get keyboard focus). Serving chords over the SAME persistent
/// keyboard also avoids the attach/detach churn per chord that trips
/// gpui's keyboard state (upstream unwrap in the Modifiers handler,
/// measured).
fn keep_devices() {
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
    let _ptr = pointer_mgr.create_virtual_pointer(Some(&seat), &qh, ());
    let kb = keyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
    // A keymap is required before the compositor treats the keyboard as
    // usable; the include-based map resolves against the system xkb data
    let keymap = "xkb_keymap {\n\
                  \x20 xkb_keycodes { include \"evdev\" };\n\
                  \x20 xkb_types { include \"complete\" };\n\
                  \x20 xkb_compatibility { include \"complete\" };\n\
                  \x20 xkb_symbols { include \"pc+us\" };\n\
                  };";
    let path = std::env::temp_dir().join("vptr-keep-keymap.xkb");
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
    eprintln!("vptr: keeper ready");

    use std::io::BufRead as _;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let chord = line.trim();
        if chord.is_empty() {
            continue;
        }
        inject_chord(&kb, &mut queue, &mut app, chord);
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
         \x20      vptr key CHORD                    (e.g. ctrl+y, escape, ctrl-shift-f1)"
    );
    std::process::exit(2);
}
