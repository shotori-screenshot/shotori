//! # scrollprobe — minimal screencopy / virtual-pointer repro tool
//!
//! Standalone diagnosis for the long-screenshot engine. Modes:
//! - `region [OUT] [x y w h]` — one capture_output_region, event trace
//! - `full [OUT]`             — one capture_output, event trace
//! - `lib`                    — call the production capture_all_outputs
//! - `ptr [gx gy]`            — engine's exact pointer-mapping math +
//!                              motion_absolute; verify with `grim -c`
//!
//! Born from the e2e chase where the engine timed out where grim
//! succeeded (the flush-after-dispatch deadlock + the v3 buffer_done
//! handshake — see ROADMAP "scroll stitching").

use std::fs::File;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::globals::{Global, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

struct Out {
    output: wl_output::WlOutput,
    name: String,
    pos: (i32, i32),
    logical: Option<(i32, i32)>,
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    outputs: Vec<Out>,
    xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    vptr: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    geometry: Option<(wl_shm::Format, i32, i32, i32)>,
    ready: bool,
    failed: bool,
    y_invert: bool,
    announced: bool,
    buffer: Option<wl_buffer::WlBuffer>,
}

macro_rules! noop {
    ($($t:ty),* $(,)?) => { $(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )* };
}
noop!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        e: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(*idx) else {
            return;
        };
        match e {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Geometry { x, y, .. } => o.pos = (x, y),
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        e: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(*idx) else {
            return;
        };
        match e {
            zxdg_output_v1::Event::LogicalPosition { x, y } => o.pos = (x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                o.logical = Some((width, height));
            }
            _ => {}
        }
    }
}

impl State {
    fn handle_buffer(
        &mut self,
        qh: &QueueHandle<Self>,
        fmt: wl_shm::Format,
        w: i32,
        h: i32,
        stride: i32,
    ) {
        println!("probe: Buffer {w}x{h} stride {stride} fmt {fmt:?}");
        self.geometry = Some((fmt, w, h, stride));
        if self.buffer.is_none() {
            let size = stride as i64 * h as i64;
            let file = tempfile::tempfile().expect("tempfile");
            file.set_len(size as u64).expect("set_len");
            let mmap = unsafe { memmap2::MmapMut::map_mut(&file) }.expect("mmap");
            let shm = self.shm.as_ref().expect("wl_shm");
            let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
            let buffer = pool.create_buffer(0, w, h, stride, fmt, qh, ());
            std::mem::forget((file, mmap)); // keep alive for the process
            self.buffer = Some(buffer);
            println!("probe: buffer created");
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        e: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match e {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let fmt = format.into_result().unwrap_or(wl_shm::Format::Xrgb8888);
                state.handle_buffer(qh, fmt, width as i32, height as i32, stride as i32);
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                println!("probe: Flags {flags:?}");
                state.y_invert = flags
                    .into_result()
                    .is_ok_and(|f| f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                println!("probe: BufferDone — copying now");
                state.announced = true;
                if let Some(buffer) = &state.buffer {
                    frame.copy(buffer);
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                println!("probe: Ready");
                state.ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                println!("probe: Failed");
                state.failed = true;
            }
            other => println!("probe: other {other:?}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("region");
    if mode == "lib" {
        match shotori::platform::capture::capture_all_outputs() {
            Ok(caps) => println!(
                "probe: lib capture OK: {}",
                caps.iter()
                    .map(|c| format!("{} {}x{}", c.output_name, c.width, c.height))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(e) => println!("probe: lib capture FAILED: {e:#}"),
        }
        return;
    }

    let conn = Connection::connect_to_env().expect("connect");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let mut state = State::default();
    for Global {
        name,
        interface,
        version,
    } in globals.contents().clone_list()
    {
        match interface.as_str() {
            "wl_shm" => state.shm = Some(globals.bind(&qh, version.min(1)..=1, ()).unwrap()),
            "zwlr_screencopy_manager_v1" => {
                println!("probe: screencopy manager advertised v{version}");
                state.manager = Some(globals.bind(&qh, version.min(3)..=3, ()).unwrap())
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.vptr = Some(globals.bind(&qh, version.min(2)..=2, ()).unwrap())
            }
            "zxdg_output_manager_v1" => {
                state.xdg = Some(globals.bind(&qh, version.min(3)..=3, ()).unwrap())
            }
            "wl_output" => {
                let idx = state.outputs.len();
                let o = globals
                    .registry()
                    .bind::<wl_output::WlOutput, usize, State>(name, version.min(4), &qh, idx);
                state.outputs.push(Out {
                    output: o,
                    name: String::new(),
                    pos: (0, 0),
                    logical: None,
                });
            }
            _ => {}
        }
    }
    queue.roundtrip(&mut state).expect("roundtrip");
    if let Some(xdg) = state.xdg.clone() {
        for (i, o) in state.outputs.iter().enumerate() {
            xdg.get_xdg_output(&o.output, &qh, i);
        }
    }
    queue.roundtrip(&mut state).expect("geometry roundtrip");

    if mode == "ptr" {
        let cx: i32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1450);
        let cy: i32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(460);
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for o in &state.outputs {
            println!(
                "probe: output {} pos {:?} logical {:?}",
                o.name, o.pos, o.logical
            );
            if let Some((w, h)) = o.logical {
                x0 = x0.min(o.pos.0);
                y0 = y0.min(o.pos.1);
                x1 = x1.max(o.pos.0 + w);
                y1 = y1.max(o.pos.1 + h);
            }
        }
        let (uw, uh) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let (fx, fy) = ((cx - x0) as u32, (cy - y0) as u32);
        println!("probe: union {x0}+{uw} × {y0}+{uh} → motion_absolute({fx},{fy},{uw},{uh})");
        let mgr = state.vptr.clone().expect("no virtual pointer manager");
        let ptr = mgr.create_virtual_pointer(None, &qh, ());
        ptr.motion_absolute(1000, fx, fy, uw, uh);
        ptr.frame();
        queue.roundtrip(&mut state).expect("ptr roundtrip");
        println!("probe: pointer motion sent (verify with grim -c)");
        return;
    }

    let output_name = args.get(1).cloned().unwrap_or_else(|| "HDMI-A-1".into());
    let nums: Vec<i32> = args[2..].iter().filter_map(|v| v.parse().ok()).collect();
    let (x, y, w, h) = match nums.as_slice() {
        [x, y, w, h] => (*x, *y, *w, *h),
        _ => (1000, 160, 900, 600),
    };
    let manager = state.manager.clone().expect("no screencopy manager");
    let names: Vec<&str> = state.outputs.iter().map(|o| o.name.as_str()).collect();
    let output = state
        .outputs
        .iter()
        .find(|o| o.name == output_name)
        .map(|o| o.output.clone())
        .unwrap_or_else(|| panic!("output {output_name} not found among {names:?}"));
    println!("probe: mode={mode} region {x},{y} {w}x{h} on {output_name}");
    let _frame = match mode {
        "full" => manager.capture_output(0, &output, &qh, ()),
        _ => manager.capture_output_region(0, &output, x, y, w, h, &qh, ()),
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    while !state.ready && !state.failed {
        if Instant::now() > deadline {
            println!("probe: TIMEOUT — geometry {:?}", state.geometry);
            return;
        }
        conn.flush().unwrap();
        queue.dispatch_pending(&mut state).unwrap();
        // Load-bearing: handlers queue the copy request; it must reach
        // the wire before poll() blocks.
        conn.flush().unwrap();
        if state.ready || state.failed {
            break;
        }
        let remain = deadline.saturating_duration_since(Instant::now());
        let ts = rustix::event::Timespec {
            tv_sec: remain.as_secs() as _,
            tv_nsec: remain.subsec_nanos() as _,
        };
        let fd = conn.as_fd();
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut fds, Some(&ts)) {
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => panic!("poll {e}"),
            Ok(_) => {}
        }
        if let Some(guard) = queue.prepare_read()
            && guard.read().is_err()
        {
            continue; // EAGAIN: readable was consumed by dispatch_pending
        }
    }
    println!(
        "probe: done (ready={}, failed={}, y_invert={})",
        state.ready, state.failed, state.y_invert
    );
}
