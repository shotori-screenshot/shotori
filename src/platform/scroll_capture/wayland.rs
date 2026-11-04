//! Wayland `zwlr-screencopy` plumbing for long-screenshot region capture.
//!
//! Connects a dedicated Wayland queue for the scroll thread, validates
//! output orientation and bounds, seeds [`ScrollSheet`] with display-scale-
//! adjusted [`SheetLimits`], and feeds physical RGBA frames into
//! [`super::session::run_session`].

use std::fs::File;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use async_channel::{Receiver, Sender};
use wayland_client::globals::{Global, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wl_shm::Format;

use super::super::capture::convert_to_rgba;
use super::session::{FrameSource, Preview, SessionTimings, run_session};
use super::{Cmd, ScrollEvent, ScrollSpec};
use crate::model::scroll_sheet::{Axis as ScrollAxis, ScrollSheet, SheetLimits};
use crate::model::session::ScrollRect;

/// One screencopy frame must arrive within this.
const FRAME_DEADLINE: Duration = Duration::from_millis(2000);

/// Pre-canvas startup errors mapped onto localized `scroll.fail.*` keys
/// so user notifications never expose untranslated internal prose.
pub(super) enum StartupError {
    RotatedOutput,
    OutsideOutput,
    SelectionTooSmall,
    Other(anyhow::Error),
}

impl StartupError {
    pub(super) fn user_message(&self) -> String {
        match self {
            Self::RotatedOutput => crate::i18n::t!("scroll.fail.rotated_output").into_owned(),
            Self::OutsideOutput => crate::i18n::t!("scroll.fail.outside_output").into_owned(),
            Self::SelectionTooSmall => {
                crate::i18n::t!("scroll.fail.selection_too_small").into_owned()
            }
            Self::Other(err) => {
                crate::i18n::t!("scroll.fail.start_error", error = format!("{err:#}")).into_owned()
            }
        }
    }
}

impl From<anyhow::Error> for StartupError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

pub(super) fn run(
    spec: &ScrollSpec,
    options: &SheetLimits,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
) -> Result<(), StartupError> {
    let conn = Connection::connect_to_env().context("connecting to the compositor")?;
    let (globals, mut queue) =
        registry_queue_init::<EngineState>(&conn).context("wayland registry")?;
    let qh = queue.handle();
    let mut state = EngineState::default();
    for Global {
        name,
        interface,
        version,
    } in globals.contents().clone_list()
    {
        match interface.as_str() {
            "wl_shm" => {
                state.shm = Some(
                    globals
                        .bind(&qh, version.min(1)..=1, ())
                        .context("binding wl_shm")?,
                );
            }
            "zwlr_screencopy_manager_v1" => {
                let v = version.min(3);
                state.screencopy_version = v;
                state.screencopy = Some(
                    globals
                        .bind(&qh, v..=v, ())
                        .context("binding zwlr_screencopy_manager_v1")?,
                );
            }
            "zxdg_output_manager_v1" => {
                state.xdg_manager = Some(
                    globals
                        .bind(&qh, version.min(3)..=3, ())
                        .context("binding zxdg_output_manager_v1")?,
                );
            }
            "wl_output" => {
                let idx = state.outputs.len();
                let output = globals
                    .registry()
                    .bind::<wl_output::WlOutput, usize, EngineState>(
                        name,
                        version.min(4),
                        &qh,
                        idx,
                    );
                state.outputs.push(OutputInfo::new(output));
            }
            _ => {}
        }
    }
    queue.roundtrip(&mut state).context("first roundtrip")?;
    if let Some(xdg) = state.xdg_manager.clone() {
        for (i, o) in state.outputs.iter().enumerate() {
            xdg.get_xdg_output(&o.output, &qh, i);
        }
    }
    queue
        .roundtrip(&mut state)
        .context("output geometry roundtrip")?;

    let screencopy = state
        .screencopy
        .clone()
        .ok_or_else(|| anyhow!("compositor does not support zwlr_screencopy_manager_v1"))?;

    let idx = state
        .outputs
        .iter()
        .position(|o| o.name == spec.output)
        .ok_or_else(|| anyhow!("output {} not found", spec.output))?;
    let target = state.outputs[idx].clone();
    // 90°-family rotations swap the output's axes — the logical region
    // math and the row-based sheet both break there. Flips are fine:
    // screencopy buffers come orientation-normalized (measured on niri's
    // winit backend, which advertises Flipped180 with upright buffers).
    if matches!(
        target.transform,
        wl_output::Transform::_90
            | wl_output::Transform::_270
            | wl_output::Transform::Flipped90
            | wl_output::Transform::Flipped270
    ) {
        eprintln!(
            "[shotori] scroll: output {} has unsupported rotation {:?}",
            spec.output, target.transform
        );
        return Err(StartupError::RotatedOutput);
    }
    let (logical_w, logical_h) = target.logical_size.ok_or_else(|| {
        anyhow!(
            "no logical size for {} (zxdg_output_v1 missing)",
            spec.output
        )
    })?;
    let initial_rect = *spec
        .rect
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if initial_rect.x < 0
        || initial_rect.y < 0
        || initial_rect.width <= 0
        || initial_rect.height <= 0
        || initial_rect.x + initial_rect.width > logical_w
        || initial_rect.y + initial_rect.height > logical_h
    {
        eprintln!("[shotori] scroll: selection {initial_rect:?} outside {logical_w}x{logical_h}");
        return Err(StartupError::OutsideOutput);
    }

    // Grace before the first capture: the overlays unmapped when this
    // engine started, and their destroy requests need a few loop passes
    // to reach the compositor (same flush lag the save flow's 150 ms
    // quit delay exists for). Capturing any earlier folds overlay chrome
    // into the seed frame.
    std::thread::sleep(Duration::from_millis(400));
    let mut capturer = WaylandCapturer {
        conn: &conn,
        queue: &mut queue,
        state: &mut state,
        screencopy: &screencopy,
        output: target.output.clone(),
        rect: &spec.rect,
        frame_size: (0, 0),
    };
    let first = capturer.capture_once()?;
    let (fw, fh) = capturer.frame_size;

    // Derive the output's true physical-to-logical ratio straight from
    // captured physical width ÷ logical region width (immune to
    // `wl_output`'s integer-ceil scale on fractional HiDPI setups), then
    // express `SheetLimits::max_height` in logical px with a 512 MiB
    // physical ceiling.
    let display_scale = (fw as f32 / initial_rect.width.max(1) as f32).max(1.0);
    let sheet_width = match spec.axis {
        ScrollAxis::Vertical => fw,
        ScrollAxis::Horizontal => fh,
    };
    let scaled_options = options.with_display_scale(display_scale, sheet_width);

    let mut sheet = match spec.axis {
        ScrollAxis::Vertical => ScrollSheet::new(fw, fh, &first, scaled_options),
        ScrollAxis::Horizontal => {
            ScrollSheet::new_with_axis(fw, fh, &first, scaled_options, ScrollAxis::Horizontal)
        }
    }
    .map_err(|r| {
        eprintln!("[shotori] scroll: selection too small ({r:?})");
        StartupError::SelectionTooSmall
    })?;

    let axis = spec.axis;
    println!(
        "[shotori] scroll: started {fw}x{fh} (scale {display_scale:.2}x, cap {}) on {}",
        scaled_options.max_height, spec.output
    );
    drop(events.try_send(ScrollEvent::Started {
        width: fw,
        height: fh,
    }));
    drop(events.try_send(ScrollEvent::Axis {
        axis,
        locked: false,
    }));

    let mut preview = Preview::new(spec.preview.clone());
    run_session(
        &mut sheet,
        &mut capturer,
        events,
        cmds,
        &mut preview,
        SessionTimings::default(),
    );
    Ok(())
}

/// One region capture session: the connection pieces + the persistent
/// shm buffer the compositor copies every frame into.
struct WaylandCapturer<'a> {
    conn: &'a Connection,
    queue: &'a mut wayland_client::EventQueue<EngineState>,
    state: &'a mut EngineState,
    screencopy: &'a zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    output: wl_output::WlOutput,
    rect: &'a std::sync::Arc<std::sync::Mutex<ScrollRect>>,
    /// The physical (buffer-space) frame size, filled by the first
    /// capture — the sheet's viewport geometry.
    frame_size: (u32, u32),
}

impl FrameSource for WaylandCapturer<'_> {
    /// Capture the region once, RGBA (physical px). The frame proxy is
    /// destroyed on drop at the end of the request — one frame object
    /// per capture, per the screencopy lifecycle.
    fn capture_once(&mut self) -> anyhow::Result<Vec<u8>> {
        self.state.cur = Some(FrameCapture::default());
        let (x, y, w, h) = {
            let rect = self
                .rect
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (rect.x, rect.y, rect.width, rect.height)
        };
        let _frame = self.screencopy.capture_output_region(
            0,
            &self.output,
            x,
            y,
            w,
            h,
            &self.queue.handle(),
            (),
        );
        wait_frame(self.conn, self.queue, self.state)?;
        let cur = self
            .state
            .cur
            .take()
            .ok_or_else(|| anyhow!("frame state vanished"))?;
        if cur.failed {
            bail!("screencopy frame failed");
        }
        let (fmt, w, h, stride) = cur
            .geometry
            .ok_or_else(|| anyhow!("frame ready without buffer info"))?;
        self.frame_size = (w as u32, h as u32);
        let bcx = self
            .state
            .buffer
            .as_ref()
            .ok_or_else(|| anyhow!("no shm buffer was created for the frame"))?;
        if (w, h, stride) != (bcx.w, bcx.h, bcx.stride) {
            bail!("region buffer geometry changed mid-session ({w}x{h} s{stride})");
        }
        Ok(convert_to_rgba(
            &bcx.mmap[..],
            fmt,
            w,
            h,
            stride,
            cur.y_invert,
        ))
    }
}

/// The bounded socket wait from `capture_all_outputs`, tightened: flush →
/// dispatch what's queued → poll for readability → read. A frame must
/// land within [`FRAME_DEADLINE`].
fn wait_frame(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<EngineState>,
    state: &mut EngineState,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + FRAME_DEADLINE;
    loop {
        if state.cur.as_ref().is_some_and(|c| c.ready || c.failed) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("screencopy frame timed out");
        }
        conn.flush()?;
        queue.dispatch_pending(state)?;
        // Load-bearing second flush: requests queued by event handlers
        // (the screencopy `copy`!) must reach the wire before poll()
        // blocks — without it the loop deadlocks against a server
        // waiting for a request still sitting in the out-buffer
        // (measured on niri).
        conn.flush()?;
        if state.cur.as_ref().is_some_and(|c| c.ready || c.failed) {
            return Ok(());
        }
        let ts = rustix::event::Timespec {
            tv_sec: remaining.as_secs() as _,
            tv_nsec: remaining.subsec_nanos() as _,
        };
        let fd = conn.as_fd();
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut fds, Some(&ts)) {
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        if let Some(guard) = queue.prepare_read() {
            guard.read()?;
        }
    }
}

#[derive(Default)]
struct EngineState {
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    /// The bound screencopy manager version (gates the `buffer_done`
    /// handshake; see [`EngineState::handle_buffer`]).
    screencopy_version: u32,
    xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<OutputInfo>,
    /// The persistent region buffer (created on the first Buffer event,
    /// reused for every frame — one tempfile + one mmap per session).
    buffer: Option<BufferCtx>,
    /// The in-flight frame's flags; reset before every capture.
    cur: Option<FrameCapture>,
}

#[derive(Clone)]
struct OutputInfo {
    output: wl_output::WlOutput,
    name: String,
    logical_pos: (i32, i32),
    logical_size: Option<(i32, i32)>,
    transform: wl_output::Transform,
}

impl OutputInfo {
    fn new(output: wl_output::WlOutput) -> Self {
        Self {
            output,
            name: String::new(),
            logical_pos: (0, 0),
            logical_size: None,
            transform: wl_output::Transform::Normal,
        }
    }
}

#[derive(Default)]
struct FrameCapture {
    geometry: Option<(wl_shm::Format, i32, i32, i32)>,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

struct BufferCtx {
    /// Held (never read) to keep the mapping's backing alive; closing it
    /// while the pool lives would be UB-adjacent.
    #[allow(dead_code)]
    file: File,
    mmap: memmap2::MmapMut,
    buffer: wl_buffer::WlBuffer,
    w: i32,
    h: i32,
    stride: i32,
}

impl EngineState {
    /// Record the announced geometry and (once) create the shm buffer.
    /// The `copy` itself does NOT happen here: screencopy v3 announces
    /// several buffer options (shm, dmabuf) and the request is legal
    /// only after `buffer_done` — sending it early makes compositors
    /// drop the frame silently (measured on niri; v1/v2 managers never
    /// send `buffer_done`, and `copy_after_announce` handles that).
    fn handle_buffer(&mut self, qh: &QueueHandle<Self>, fmt: Format, w: i32, h: i32, stride: i32) {
        let reusable = self
            .buffer
            .as_ref()
            .is_some_and(|b| (w, h, stride) == (b.w, b.h, b.stride));
        let size = (stride as i64 * h as i64) as u64;
        if !reusable && (w <= 0 || h <= 0 || stride < w * 4 || size > i32::MAX as u64) {
            eprintln!("[shotori] scroll: impossible buffer geometry {w}x{h} stride {stride}");
            self.fail_cur();
            return;
        }
        if !reusable {
            let file = match tempfile::tempfile() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[shotori] scroll: temp file failed: {e}");
                    self.fail_cur();
                    return;
                }
            };
            if let Err(e) = file.set_len(size) {
                eprintln!("[shotori] scroll: setting file length failed: {e}");
                self.fail_cur();
                return;
            }
            let mmap = match unsafe { memmap2::MmapMut::map_mut(&file) } {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[shotori] scroll: mmap failed: {e}");
                    self.fail_cur();
                    return;
                }
            };
            let Some(shm) = self.shm.as_ref() else {
                eprintln!("[shotori] scroll: no wl_shm global");
                self.fail_cur();
                return;
            };
            let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
            let buffer = pool.create_buffer(0, w, h, stride, fmt, qh, ());
            self.buffer = Some(BufferCtx {
                file,
                mmap,
                buffer,
                w,
                h,
                stride,
            });
        }
        if let Some(c) = self.cur.as_mut() {
            c.geometry = Some((fmt, w, h, stride));
        }
    }

    fn copy_after_announce(&mut self, frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1) {
        if let Some(bcx) = self.buffer.as_ref() {
            frame.copy(&bcx.buffer);
        }
    }

    fn fail_cur(&mut self) {
        if let Some(c) = self.cur.as_mut() {
            c.failed = true;
        }
    }
}

macro_rules! noop_dispatch {
    ($($t:ty),* $(,)?) => { $(
        impl Dispatch<$t, ()> for EngineState {
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
noop_dispatch!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for EngineState {
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

impl Dispatch<wl_output::WlOutput, usize> for EngineState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(*idx) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                o.logical_pos = (x, y);
                o.transform = transform
                    .into_result()
                    .unwrap_or(wl_output::Transform::Normal);
            }
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for EngineState {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(*idx) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => o.logical_pos = (x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                o.logical_size = Some((width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for EngineState {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let fmt = format.into_result().unwrap_or(Format::Xrgb8888);
                state.handle_buffer(qh, fmt, width as i32, height as i32, stride as i32);
                if state.screencopy_version < 3 {
                    // v1/v2 managers never send buffer_done — the copy
                    // is legal the moment a buffer was announced.
                    state.copy_after_announce(frame);
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                state.copy_after_announce(frame);
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                if let Some(c) = state.cur.as_mut() {
                    c.y_invert = flags
                        .into_result()
                        .is_ok_and(|f| f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                if let Some(c) = state.cur.as_mut() {
                    c.ready = true;
                }
            }
            zwlr_screencopy_frame_v1::Event::Failed => state.fail_cur(),
            _ => {}
        }
    }
}
