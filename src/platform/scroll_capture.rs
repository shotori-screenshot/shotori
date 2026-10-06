//! # Scroll capture engine: the Wayland half of long screenshots
//!
//! One dedicated thread owns ONE wayland connection that does both jobs
//! of the capture loop — injecting wheel events (`zwlr_virtual_pointer_v1`)
//! and re-capturing the selection (`zwlr_screencopy_manager_v1`
//! `.capture_output_region`, in LOGICAL px per the protocol). Sharing a
//! connection is deliberate: injection and capture serialize through the
//! same event queue, so a capture can never race the scroll it is
//! supposed to follow.
//!
//! The engine is wire code around [`crate::model::scroll_stitch`]
//! (the pure decision core). The platform→model import here is a
//! conscious exception to the "ui → model → platform" arrows: the
//! stitcher imports nothing platform-side, and keeping the engine next
//! to the other compositor clients beats inventing a trait seam for
//! exactly one consumer (ROADMAP: scroll stitching).
//!
//! Compositor requirements: `zwlr_screencopy` (as the rest of shotori)
//! plus `zwlr_virtual_pointer_v1` — broadly implemented (sway, Hyprland,
//! niri, KWin, COSMIC, labwc, river, Wayfire, Weston…), NOT by GNOME
//! Mutter. Without it the engine fails fast with a clear message.

use std::fs::File;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use async_channel::{Receiver, Sender};
use wayland_client::globals::{Global, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_pointer::Axis;
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

use super::capture::convert_to_rgba;
use crate::model::scroll_stitch::{ScrollStitcher, StitchOptions, StitchOutcome, StitchReject};
use crate::model::session::ScrollRect;

/// Which screen to watch and where (output-local LOGICAL integer px —
/// the `capture_output_region` contract).
pub struct ScrollSpec {
    pub output: String,
    /// The capture rectangle, SHARED with the UI: dragging the region
    /// frame rewrites it and the next poll captures the new spot (w/h
    /// stay fixed — drags move, never resize).
    pub rect: std::sync::Arc<std::sync::Mutex<ScrollRect>>,
    /// Manual: the user scrolls, the engine only captures + stitches
    /// (works everywhere, no injection). Auto: the engine injects wheel
    /// steps itself — currently niri-hostile (see inject_step).
    pub mode: ScrollMode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollMode {
    Manual,
    Auto,
}

/// What the engine reports to the UI. `Finished` carries the stitched
/// image; every terminal event (`Finished`/`Failed`/`Cancelled`) is sent
/// exactly once, then the thread exits.
pub enum ScrollEvent {
    /// First frame captured; the physical frame size (post-scale).
    Started { width: u32, height: u32 },
    /// Rows captured beyond the initial viewport (progress UI).
    Progress { stitched: u32 },
    /// The viewport moved (preview rows, engine-mapped) — no pixels,
    /// cheap enough to fire on every accepted frame so the highlight
    /// tracks the frame drag in real time.
    Viewport { top: u32, height: u32 },
    /// A downscaled tail of the canvas for the live preview panel
    /// (throttled; RGBA, box-filtered by [`crate::model::scroll_stitch::preview_tail`]).
    /// `tail_start`/`viewport_*` place the capture viewport on the
    /// preview so the panel can highlight "what you are looking at".
    Preview {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
        tail_start: u32,
        viewport_top: u32,
        viewport_height: u32,
    },
    /// The canvas is complete (bottom reached, cap hit, or user finish).
    Finished {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
    },
    /// Unrecoverable (missing protocol, unsupported output, no alignment).
    Failed { reason: String },
    /// User cancelled; nothing is delivered.
    Cancelled,
}

enum Cmd {
    /// Stop scrolling and deliver the canvas as-is.
    Finish,
    /// Stop and discard.
    Cancel,
}

/// The UI's handle onto a running engine. Cheap to clone; dropping it
/// does NOT stop the engine (the thread ends itself on its terminal
/// conditions or a command).
#[derive(Clone)]
pub struct ScrollControls {
    tx: Sender<Cmd>,
}

impl ScrollControls {
    pub fn finish(&self) {
        let _ = self.tx.try_send(Cmd::Finish);
    }
    pub fn cancel(&self) {
        let _ = self.tx.try_send(Cmd::Cancel);
    }
    /// A handle to nowhere — commands vanish. For the UI when the engine
    /// thread could not even spawn; the failure arrives as a `Failed`
    /// event on the paired (synthetic) stream.
    pub fn dead() -> Self {
        let (tx, _rx) = async_channel::bounded(1);
        Self { tx }
    }
}

/// Spawn the scroll engine. Returns the control handle and the event
/// stream; the caller owns the receiving end (typically a gpui spawn
/// pumping events into the control-bar window).
pub fn start(
    spec: ScrollSpec,
    options: StitchOptions,
) -> anyhow::Result<(ScrollControls, Receiver<ScrollEvent>)> {
    let (cmd_tx, cmd_rx) = async_channel::bounded(8);
    let (ev_tx, ev_rx) = async_channel::bounded(64);
    std::thread::Builder::new()
        .name("shotori-scroll".into())
        .spawn(move || {
            let result = run(&spec, &options, &ev_tx, &cmd_rx);
            if let Err(e) = result {
                // The UI is gone too (window closed)? Then nobody reads;
                // try_send failing is fine.
                let _ = ev_tx.try_send(ScrollEvent::Failed {
                    reason: format!("{e:#}"),
                });
            }
        })
        .context("spawning the scroll thread")?;
    Ok((ScrollControls { tx: cmd_tx }, ev_rx))
}

// ── timing / tuning ──────────────────────────────────────────────────
/// Pause after injecting before the first stability poll: the scroll
/// must land and repaint first.
const POST_INJECT_DELAY: Duration = Duration::from_millis(60);
/// Stability polling cadence.
const POLL_INTERVAL: Duration = Duration::from_millis(40);
/// A scroll step must settle within this budget (smooth-scroll apps
/// animate; infinite feeds never do — the matcher judges the rest).
const SETTLE_DEADLINE: Duration = Duration::from_millis(1500);
/// One screencopy frame must arrive within this.
const FRAME_DEADLINE: Duration = Duration::from_millis(2000);
/// Whole-session budget — a runaway scroll on an infinite feed stops
/// here even without the height cap.
const SESSION_DEADLINE: Duration = Duration::from_secs(60);
/// Consecutive NoMotion outcomes before declaring the bottom reached.
const BOTTOM_NO_MOTION: u32 = 3;
/// Consecutive alignment rejections tolerated before failing.
const MAX_REJECTS: u32 = 4;
/// Wheel "lines" injected on the first probe (calibrated afterwards).
const INITIAL_LINES: i32 = 3;
/// Aim for this step size as a fraction of the viewport (must stay
/// below the stitcher's max_motion_ratio for overlap to survive).
const TARGET_STEP_RATIO: f32 = 0.45;
/// Manual-mode polling cadence — fast enough that a brisk user scroll
/// never skips more than one viewport, slow enough to stay cheap.
const MANUAL_POLL: Duration = Duration::from_millis(60);
/// Manual-mode wall budget (the user pauses to read; this is a
/// safety valve, not a target).
const MANUAL_DEADLINE: Duration = Duration::from_secs(600);
/// Live-preview refresh cadence.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(250);
/// Live-preview target width (px, before the panel scales it again).
/// ~2× the panel's logical column: the panel upscales to its width, and
/// a real 2× sample still holds text legible on HiDPI. (200 was a 5–10×
/// box filter on wide frames — destructively soft, user-reported.)
const PREVIEW_WIDTH: u32 = 480;
/// Canvas rows included in the preview tail (the growing edge). Also
/// the cost bound: the tail spans at most this × k canvas rows.
const PREVIEW_MAX_ROWS: u32 = 2000;

/// How the wheel is injected. Discrete (axis_discrete) is the modern
/// form; some compositors only honor the plain axis value — if two
/// cycles produce no motion we degrade gracefully instead of failing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum InjectStyle {
    Discrete,
    Plain,
}

/// Real CLOCK_MONOTONIC milliseconds — the clock Wayland input
/// timestamps are defined against. Synthetic counters (vptr-style
/// 1000, 1030, …) can trip compositor gesture heuristics that group
/// axis events by timestamp gaps (measured: repeated small injections
/// went dead after the first on niri).
fn now_ms() -> u32 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (t.tv_sec as u64 * 1_000 + t.tv_nsec as u64 / 1_000_000) as u32
}

/// Inject ONE wheel step over a FRESH connection. niri accumulates axis
/// events per client connection into a single gesture — the first
/// injection scrolls and every later one vanishes (measured with
/// vptr's scrollloop: six events from one client all die; six separate
/// invocations all work). Connection-per-step reproduces the only
/// semantics compositors actually deliver; the ~1 ms connect cost is
/// noise next to the settle wait.
fn inject_step(lines: i32, sign: f64, style: InjectStyle) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env().context("injector connection")?;
    let (globals, mut queue) =
        registry_queue_init::<EngineState>(&conn).map_err(|e| anyhow!("injector registry: {e}"))?;
    let qh = queue.handle();
    let mut state = EngineState::default();
    let mut seat: Option<wl_seat::WlSeat> = None;
    let mut manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1> = None;
    for Global {
        interface, version, ..
    } in globals.contents().clone_list()
    {
        match interface.as_str() {
            "wl_seat" => seat = Some(globals.bind(&qh, version.min(7)..=7, ())?),
            "zwlr_virtual_pointer_manager_v1" => {
                manager = Some(globals.bind(&qh, version.min(2)..=2, ())?)
            }
            _ => {}
        }
    }
    let manager = manager.context("no virtual pointer manager for injection")?;
    let ptr = manager.create_virtual_pointer(seat.as_ref(), &qh, ());
    queue.roundtrip(&mut state).ok();
    let clock = now_ms();
    let value = sign * 15.0 * f64::from(lines);
    match style {
        InjectStyle::Discrete => {
            ptr.axis_discrete(clock, Axis::VerticalScroll, value, (sign as i32) * lines)
        }
        InjectStyle::Plain => ptr.axis(clock, Axis::VerticalScroll, value),
    }
    ptr.frame();
    queue.roundtrip(&mut state).ok();
    Ok(())
}

fn run(
    spec: &ScrollSpec,
    options: &StitchOptions,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
) -> anyhow::Result<()> {
    let session_deadline = Instant::now() + SESSION_DEADLINE;

    // ── connection + globals ────────────────────────────────────────
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
            // Singletons bind through the snapshot; outputs need per-name
            // binds (userdata = index) for their per-output events.
            "wl_shm" => state.shm = Some(globals.bind(&qh, version.min(1)..=1, ())?),
            "zwlr_screencopy_manager_v1" => {
                let v = version.min(3);
                state.screencopy_version = v;
                state.screencopy = Some(globals.bind(&qh, v..=v, ())?)
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.pointer_manager = Some(globals.bind(&qh, version.min(2)..=2, ())?)
            }
            "wl_seat" => state.seat = Some(globals.bind(&qh, version.min(7)..=7, ())?),
            "zxdg_output_manager_v1" => {
                state.xdg_manager = Some(globals.bind(&qh, version.min(3)..=3, ())?)
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
    // Logical geometry per output (the authoritative size + position)
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
    // Virtual-pointer support is an AUTO-mode requirement only. Manual
    // mode never injects — and creating a virtual pointer to "park"
    // would WARP the user's cursor (motion_absolute is a real pointer
    // motion), which manual mode must never do (the "starting a long
    // screenshot moves my mouse" bug). GNOME/Mutter lacks the protocol
    // entirely; manual mode works there regardless.
    let pointer_manager = state.pointer_manager.clone();

    // ── the target output ───────────────────────────────────────────
    let idx = state
        .outputs
        .iter()
        .position(|o| o.name == spec.output)
        .ok_or_else(|| anyhow!("output {} not found", spec.output))?;
    let target = state.outputs[idx].clone();
    // 90°-family rotations swap the output's axes — the logical region
    // math and the row-based stitcher both break there. Flips are fine:
    // screencopy buffers come orientation-normalized (measured on niri's
    // winit backend, which advertises Flipped180 with upright buffers),
    // and the engine's direction calibration absorbs any inversion.
    if matches!(
        target.transform,
        wl_output::Transform::_90
            | wl_output::Transform::_270
            | wl_output::Transform::Flipped90
            | wl_output::Transform::Flipped270
    ) {
        bail!(
            "output reports transform {:?} — long screenshots support unrotated orientations only",
            target.transform
        );
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
        bail!("selection region {initial_rect:?} is outside the output");
    }

    // ── virtual pointer, parked on the region center ────────────────
    // motion_absolute maps FRACTIONS onto the whole desktop (measured:
    // tools/vptr's "coordinate-mapping trap"), so the denominator is the
    // union of all outputs' logical rects, not this output's size.
    // AUTO only (see the pointer_manager note above): manual mode must
    // never create a pointer, let alone move it.
    let ptr = if spec.mode == ScrollMode::Auto {
        let pointer_manager = pointer_manager.ok_or_else(|| {
            anyhow!("compositor does not support zwlr_virtual_pointer_v1 (GNOME/Mutter?) — automatic scrolling is unavailable")
        })?;
        Some(pointer_manager.create_virtual_pointer(state.seat.as_ref(), &qh, ()))
    } else {
        None
    };
    let (mut union_x0, mut union_y0, mut union_x1, mut union_y1) =
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for o in &state.outputs {
        let Some((w, h)) = o.logical_size else {
            continue;
        };
        union_x0 = union_x0.min(o.logical_pos.0);
        union_y0 = union_y0.min(o.logical_pos.1);
        union_x1 = union_x1.max(o.logical_pos.0 + w);
        union_y1 = union_y1.max(o.logical_pos.1 + h);
    }
    let union_size = if union_x1 > union_x0 && union_y1 > union_y0 {
        Some(((union_x1 - union_x0) as u32, (union_y1 - union_y0) as u32))
    } else {
        None
    };
    let center = (
        target.logical_pos.0 + initial_rect.x + initial_rect.width / 2 - union_x0,
        target.logical_pos.1 + initial_rect.y + initial_rect.height / 2 - union_y0,
    );

    // ── capture machinery ───────────────────────────────────────────
    // Grace before the first capture AND (auto) the pointer parking:
    // the overlays unmapped when this engine started, and their destroy
    // requests need a few loop passes to reach the compositor (same
    // flush lag the save flow's 150 ms quit delay exists for). Parking
    // the pointer any earlier aims it at the still-mapped overlay, and
    // a pointer focused on a since-destroyed surface never recovers on
    // niri — every later wheel event dies with it (measured).
    std::thread::sleep(Duration::from_millis(400));
    if let Some(ptr) = ptr.as_ref()
        && let Some((uw, uh)) = union_size
        && center.0 >= 0
        && center.1 >= 0
    {
        // vptr-exact mapping (measured on niri — see tools/vptr's
        // "coordinate-mapping trap"): the fractions x/x_extent map onto
        // the DESKTOP UNION, so extents = the union's logical size and
        // coordinates are relative to its origin.
        ptr.motion_absolute(now_ms(), center.0 as u32, center.1 as u32, uw, uh);
        ptr.frame();
        queue.roundtrip(&mut state).ok();
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut capturer = Capturer {
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
    let mut stitcher = ScrollStitcher::new(fw, fh, &first, *options)
        .map_err(|r| anyhow!("selection too small for scroll stitching ({r:?})"))?;
    println!("[shotori] scroll: started {fw}x{fh} on {}", spec.output);
    let _ = events.try_send(ScrollEvent::Started {
        width: fw,
        height: fh,
    });

    // ── the scroll loop ─────────────────────────────────────────────
    // sign: vptr-measured on niri — a negative vertical axis value
    // scrolls DOWN. The first Appended/Prepended verdict calibrates the
    // direction for real; apps disagree no further than that.
    let mut sign = -1.0f64;
    let mut style = InjectStyle::Discrete;
    let mut lines = INITIAL_LINES;
    let mut no_motion = 0u32;
    let mut rejects = 0u32;
    let mut flips = 0u32;
    // Has any injection produced motion yet? Gates the proactive
    // direction flip (see the no-motion arm).
    let mut direction_validated = false;
    let target_step = (fh as f32 * TARGET_STEP_RATIO).max(40.);
    let mut anchor = first;

    if spec.mode == ScrollMode::Manual {
        return run_manual(&mut stitcher, &mut capturer, &mut anchor, events, cmds);
    }

    loop {
        if Instant::now() > session_deadline {
            bail!("scroll session timed out");
        }
        match cmds.try_recv() {
            Ok(Cmd::Finish) => return deliver(events, &stitcher),
            Ok(Cmd::Cancel) => {
                let _ = events.try_send(ScrollEvent::Cancelled);
                return Ok(());
            }
            Err(_) => {}
        }

        inject_step(lines, sign, style)?;

        let frame = capturer.settle(&mut anchor)?;
        let outcome = stitcher.push(&frame);
        println!(
            "[shotori] scroll: frame #{} → {:?} (lines={lines}, sign={sign}, style={style:?})",
            stitcher.stats().0,
            outcome
        );
        // E2E backdoor: dump every pushed frame for offline matcher
        // analysis (see ui/e2e.rs conventions).
        if let Ok(dir) = std::env::var("SHOTORI_SCROLL_DUMP") {
            let n = stitcher.stats().0;
            if let Ok(png) = crate::model::export::encode_png(
                capturer.frame_size.0,
                capturer.frame_size.1,
                &frame,
            ) {
                let _ = std::fs::write(
                    std::path::Path::new(&dir).join(format!("frame_{n:03}.png")),
                    png,
                );
            }
        }
        match outcome {
            StitchOutcome::Appended { dy, .. } => {
                no_motion = 0;
                rejects = 0;
                direction_validated = true;
                // Calibrate the step size toward the target (bounded
                // adaptation: never below one line, never absurd).
                if dy > 0 {
                    lines = ((lines as f32 * target_step / dy as f32).round() as i32).clamp(1, 16);
                }
                let _ = events.try_send(ScrollEvent::Progress {
                    stitched: stitcher.captured_extent(),
                });
            }
            StitchOutcome::Prepended { .. } => {
                // We scrolled the wrong way (or the app inverted us):
                // flip once, fail if direction never stabilizes.
                flips += 1;
                if flips > 2 {
                    bail!("cannot establish a scroll direction over the selection");
                }
                sign = -sign;
                no_motion = 0;
                rejects = 0;
                direction_validated = true;
                println!("[shotori] scroll: direction flipped (sign now {sign})");
            }
            StitchOutcome::Contained { .. } => {
                no_motion = 0;
            }
            StitchOutcome::Duplicate | StitchOutcome::NoMotion => {
                no_motion += 1;
                // Discrete injection ignored twice ⇒ try the plain axis
                // value (niri ignores zwlr_virtual_pointer.axis_discrete
                // — vptr only ever uses plain axis) and give the new
                // style a full bottom-detection window of its own.
                if no_motion == 2 && style == InjectStyle::Discrete {
                    style = InjectStyle::Plain;
                    no_motion = 0;
                    println!(
                        "[shotori] scroll: axis_discrete produced no motion, falling back to plain axis"
                    );
                }
                // Boundary dead-lock escape: before ANY motion has been
                // seen, the wrong sign at a scroll boundary (top when
                // aiming down) produces no feedback at all — nothing
                // would ever flip the direction. Flip proactively.
                if no_motion == 2 && !direction_validated {
                    sign = -sign;
                    flips += 1;
                    no_motion = 0;
                    println!(
                        "[shotori] scroll: no response yet, flipping direction (sign now {sign})"
                    );
                    if flips > 4 {
                        bail!("the selection does not respond to scrolling");
                    }
                }
                if no_motion >= BOTTOM_NO_MOTION {
                    // Bottom reached (or the region ignores the wheel —
                    // either way, nothing more will happen).
                    return deliver(events, &stitcher);
                }
            }
            StitchOutcome::Rejected(StitchReject::Ambiguous)
            | StitchOutcome::Rejected(StitchReject::HighResidual) => {
                rejects += 1;
                if rejects > MAX_REJECTS {
                    bail!("could not align the scrolled frames (content changes or repeats)");
                }
            }
            StitchOutcome::Rejected(StitchReject::HeightLimit) => {
                println!("[shotori] scroll: height cap reached");
                return deliver(events, &stitcher);
            }
            StitchOutcome::Rejected(other) => {
                bail!("stitcher rejected the frame: {other:?}");
            }
        }
    }
}

/// Ship the finished canvas and log the session summary.
fn deliver(events: &Sender<ScrollEvent>, stitcher: &ScrollStitcher) -> anyhow::Result<()> {
    let (w, h) = stitcher.dimensions();
    let (pushed, accepted) = stitcher.stats();
    println!("[shotori] scroll: finished {w}x{h} ({accepted}/{pushed} frames folded)");
    let _ = events.try_send(ScrollEvent::Finished {
        width: w,
        height: h,
        rgba: Arc::new(stitcher.canvas().to_vec()),
    });
    Ok(())
}

/// Push a throttled downscaled canvas tail for the preview panel,
/// including where the capture viewport sits on it. Returns the canvas
/// height the image was built from (the growth-flush reference).
fn send_preview(stitcher: &ScrollStitcher, events: &Sender<ScrollEvent>) -> u32 {
    let (w, h) = stitcher.dimensions();
    let (tail_start, pw, ph, rgba) = crate::model::scroll_stitch::preview_tail(
        stitcher.canvas(),
        w,
        h,
        PREVIEW_WIDTH,
        PREVIEW_MAX_ROWS,
    );
    let (v_top, v_h) = viewport_in_preview(stitcher, w, tail_start, pw, ph);
    let _ = events.try_send(ScrollEvent::Preview {
        width: pw,
        height: ph,
        rgba: Arc::new(rgba),
        tail_start,
        viewport_top: v_top,
        viewport_height: v_h,
    });
    h
}

/// The viewport's position in PREVIEW rows (the mapping needs the
/// downscale factor only the engine knows). Fires as its own cheap
/// event so frame drags update the highlight without resending pixels.
fn send_viewport(stitcher: &ScrollStitcher, events: &Sender<ScrollEvent>) {
    let (w, h) = stitcher.dimensions();
    // Mirror preview_tail's geometry math for the tail bounds.
    let k = ((w as f32) / PREVIEW_WIDTH.max(1) as f32).ceil().max(1.0) as u32;
    let pw = w.div_ceil(k);
    let tail_start = h.saturating_sub(PREVIEW_MAX_ROWS.saturating_mul(k));
    let ph = h.saturating_sub(tail_start).div_ceil(k);
    let (v_top, v_h) = viewport_in_preview(stitcher, w, tail_start, pw, ph);
    let _ = events.try_send(ScrollEvent::Viewport {
        top: v_top,
        height: v_h,
    });
}

fn viewport_in_preview(
    stitcher: &ScrollStitcher,
    w: u32,
    tail_start: u32,
    pw: u32,
    ph: u32,
) -> (u32, u32) {
    let (vt, vh) = stitcher.viewport_span();
    let k = w.div_ceil(pw.max(1));
    let v_h = (vh / k).clamp(1, ph.max(1));
    let v_top = (vt.saturating_sub(tail_start) / k).min(ph.saturating_sub(v_h));
    (v_top, v_h)
}

/// Manual mode: the user scrolls, the engine watches. Pure capture +
/// stitch polling — no injection, no direction logic; every outcome
/// except the hard failures simply rides (duplicates = user idle,
/// rejects = mid-scroll tears the anchor refresh absorbs). Termination
/// is the user's Finish/Cancel, the height cap, or the (generous)
/// manual deadline.
fn run_manual(
    stitcher: &mut ScrollStitcher,
    capturer: &mut Capturer<'_>,
    anchor: &mut Vec<u8>,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
) -> anyhow::Result<()> {
    println!("[shotori] scroll: manual mode — you scroll, the engine stitches");
    let deadline = Instant::now() + MANUAL_DEADLINE;
    let mut last_preview = Instant::now()
        .checked_sub(PREVIEW_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut last_image_h = send_preview(stitcher, events);
    // Image-refresh fidelity floor: once the canvas has grown ~4 preview
    // rows past the image the panel currently shows, the viewport
    // highlight maps past that image's rows (clamped at the panel, i.e.
    // visibly stale content under the highlight) — refresh immediately
    // instead of waiting out the throttle. Idle stretches (duplicates,
    // small contained wiggles) still ride the 250 ms cadence.
    let k = ((stitcher.dimensions().0 as f32) / PREVIEW_WIDTH.max(1) as f32)
        .ceil()
        .max(1.0) as u32;
    let flush_rows = 4 * k;
    loop {
        if Instant::now() > deadline {
            bail!("manual scroll session timed out");
        }
        match cmds.try_recv() {
            Ok(Cmd::Finish) => return deliver(events, stitcher),
            Ok(Cmd::Cancel) => {
                let _ = events.try_send(ScrollEvent::Cancelled);
                return Ok(());
            }
            Err(_) => {}
        }
        let frame = capturer.capture_once()?;
        let outcome = stitcher.push(&frame);
        if !matches!(outcome, StitchOutcome::Duplicate) {
            println!(
                "[shotori] scroll: frame #{} → {outcome:?}",
                stitcher.stats().0
            );
        }
        match outcome {
            StitchOutcome::Appended { .. } => {
                let _ = events.try_send(ScrollEvent::Progress {
                    stitched: stitcher.captured_extent(),
                });
            }
            StitchOutcome::Rejected(StitchReject::HeightLimit) => {
                println!("[shotori] scroll: height cap reached");
                return deliver(events, stitcher);
            }
            StitchOutcome::Rejected(StitchReject::TooSmall)
            | StitchOutcome::Rejected(StitchReject::InsufficientOverlap) => {
                bail!("stitcher rejected the frame: {outcome:?}");
            }
            _ => {}
        }
        // Every accepted (non-duplicate) outcome can move the viewport
        // OR reshape the canvas (prepends grow the top) — the highlight
        // follows via the cheap event; the pixel stream refreshes on
        // throttle OR growth (see `flush_rows`).
        if !matches!(outcome, StitchOutcome::Duplicate) {
            send_viewport(stitcher, events);
            let grew = stitcher.dimensions().1.saturating_sub(last_image_h);
            if last_preview.elapsed() >= PREVIEW_INTERVAL || grew >= flush_rows {
                last_preview = Instant::now();
                last_image_h = send_preview(stitcher, events);
            }
        }
        *anchor = frame;
        std::thread::sleep(MANUAL_POLL);
    }
}

// ── capture plumbing ─────────────────────────────────────────────────

/// One region capture session: the connection pieces + the persistent
/// shm buffer the compositor copies every frame into.
struct Capturer<'a> {
    conn: &'a Connection,
    queue: &'a mut wayland_client::EventQueue<EngineState>,
    state: &'a mut EngineState,
    screencopy: &'a zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    output: wl_output::WlOutput,
    rect: &'a std::sync::Arc<std::sync::Mutex<ScrollRect>>,
    /// The physical (buffer-space) frame size, filled by the first
    /// capture — the stitcher's viewport geometry.
    frame_size: (u32, u32),
}

impl Capturer<'_> {
    /// Capture the region once, RGBA (physical px).
    /// Capture the region once, RGBA (physical px). The frame proxy is
    /// destroyed on drop at the end of the request — one frame object
    /// per capture, per the screencopy lifecycle.
    fn capture_once(&mut self) -> anyhow::Result<Vec<u8>> {
        self.state.cur = Some(FrameCapture::default());
        // The frame proxy drops (and destroys) at the end of this call —
        // one frame object per capture, per the screencopy lifecycle.
        // Coordinates are read per capture: the region may have been
        // dragged since the last poll.
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

    /// Wait until the capture stops changing: two consecutive identical
    /// polls (the scroll animation settled), or the deadline. `anchor`
    /// tracks the newest frame for the NEXT settle's first comparison.
    fn settle(&mut self, anchor: &mut Vec<u8>) -> anyhow::Result<Vec<u8>> {
        std::thread::sleep(POST_INJECT_DELAY);
        let deadline = Instant::now() + SETTLE_DEADLINE;
        let mut previous: Option<Vec<u8>> = None;
        loop {
            let cap = self.capture_once()?;
            if previous.as_ref() == Some(&cap) {
                *anchor = cap.clone();
                return Ok(cap);
            }
            previous = Some(cap.clone());
            *anchor = cap.clone();
            if Instant::now() >= deadline {
                // Unstable content (video, infinite animation): hand the
                // latest frame over — the matcher decides if it aligns.
                return Ok(cap);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
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
        // (measured on niri; see ROADMAP).
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

// ── the event state machine ──────────────────────────────────────────

#[derive(Default)]
struct EngineState {
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    /// The bound screencopy manager version (gates the buffer_done
    /// handshake; see [`EngineState::handle_buffer`]).
    screencopy_version: u32,
    pointer_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    seat: Option<wl_seat::WlSeat>,
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

use wl_shm::Format;

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

    /// Request the copy — called on `buffer_done` (v3) or right after
    /// the Buffer event (v1/v2, where no announcement terminator comes).
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
    wl_seat::WlSeat,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
);

/// The registry's own dispatch: globals arrive pre-bound through the
/// `registry_queue_init` snapshot; later additions are not our concern
/// (a mid-session output hotplug ends the scroll, not the app).
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
                // v3: the announcement batch is complete; copy may go.
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
