//! # Scroll capture engine: the Wayland half of long screenshots
//!
//! One dedicated thread owns ONE wayland connection that re-captures
//! the selection (`zwlr_screencopy_manager_v1.capture_output_region`, in
//! LOGICAL px per the protocol) and folds the frames through
//! [`crate::model::scroll_sheet`] (the pure decision core). The user
//! scrolls; the engine only watches — there is no wheel injection here,
//! so a capture can never race a scroll the engine did not cause.
//!
//! The platform→model import here is a conscious exception to the
//! "ui → model → platform" arrows: the sheet imports nothing
//! platform-side, and keeping the engine next to the other compositor
//! clients beats inventing a trait seam for exactly one consumer.
//!
//! Compositor requirements: `zwlr_screencopy`, the same protocol every
//! shotori overlay already depends on.

use std::fs::File;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
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

use super::capture::convert_to_rgba;
use crate::model::scroll_sheet::{
    Axis as ScrollAxis, Fold, Phase, Refusal, ScrollSheet, SheetLimits, phase_of,
};
use crate::model::session::ScrollRect;

/// Which screen to watch and where (output-local LOGICAL integer px —
/// the `capture_output_region` contract).
pub struct ScrollSpec {
    pub output: String,
    /// The capture rectangle, SHARED with the UI: dragging the region
    /// frame rewrites it and the next poll captures the new spot (w/h
    /// stay fixed — drags move, never resize).
    pub rect: std::sync::Arc<std::sync::Mutex<ScrollRect>>,
    /// Initial growth direction; the HUD can switch it until the first
    /// frame is accepted.
    pub axis: ScrollAxis,
    /// How many preview pixels tall the panel's well is (0 = no panel,
    /// e.g. a chrome-free session: the engine then never renders one).
    /// The panel writes this once it knows its own height; the engine
    /// converts it into sheet rows with the same `k` it downscales by,
    /// so the strip is exactly as tall as the well and never rescales
    /// mid-scroll.
    pub preview_rows: Arc<AtomicU32>,
}

/// What the engine reports to the UI. `Finished` carries the folded
/// image; every terminal event (`Finished`/`Failed`/`Cancelled`) is sent
/// exactly once, then the thread exits.
pub enum ScrollEvent {
    /// First frame captured; the physical frame size (post-scale).
    Started { width: u32, height: u32 },
    /// Growth direction changed (or the initial direction, sent right
    /// after `Started`). `locked` is true once a frame was accepted and
    /// the axis can no longer be swapped.
    Axis { axis: ScrollAxis, locked: bool },
    /// Rows captured beyond the initial viewport, plus the sheet height
    /// the header can show as "W × H" (progress UI).
    Progress { extent: u32, total: u32 },
    /// The viewport moved (preview rows, engine-mapped) — no pixels,
    /// cheap enough to fire on every accepted frame so the highlight
    /// tracks the frame drag in real time.
    Viewport { top: u32, height: u32 },
    /// A downscaled WINDOW of the sheet for the live preview panel:
    /// box-filtered by [`crate::model::scroll_sheet::preview_slice`] at a
    /// fixed scale, cut at the window
    /// [`preview_window`](crate::model::scroll_sheet::preview_window)
    /// picked. The window is exactly the panel's own height in sheet
    /// rows, so the strip scrolls at 1:1 with the page instead of
    /// rescaling as the sheet grows. `viewport_*` place the capture
    /// viewport inside that window (preview rows; `None` when the panel
    /// is looking at a part of the sheet the viewport is not on).
    Preview {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
        viewport_top: Option<u32>,
        viewport_height: u32,
    },
    /// What the session is doing right now (see [`Phase`]). Fired on
    /// change only. A refusal is a STATE, not a failure: the session
    /// keeps sampling and the chip says "no overlap" instead of the
    /// capture dying under the user.
    Phase(Phase),
    /// The canvas is complete (bottom reached, cap hit, or user finish).
    Finished {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
    },
    /// Unrecoverable (missing protocol, unsupported output, no alignment).
    /// `partial` carries whatever was folded before the failure — a
    /// session that captured something must never silently discard it;
    /// the UI copies/saves it and says so in the notification.
    Failed {
        reason: String,
        /// (width, height, rgba) of the canvas so far, when one exists.
        partial: Option<(u32, u32, Arc<Vec<u8>>)>,
    },
    /// User cancelled; nothing is delivered.
    Cancelled,
}

enum Cmd {
    /// Stop scrolling and deliver the canvas as-is.
    Finish,
    /// Stop and discard.
    Cancel,
    /// Stop sampling/injecting; keep the session and its canvas alive.
    Pause,
    /// Resume after [`Cmd::Pause`].
    Resume,
    /// Switch growth direction (ignored once locked).
    SetAxis(ScrollAxis),
    /// Step the preview away from the live edge (`Some(preview rows)`,
    /// negative = look further up) or return to it (`None`). The strip
    /// is a viewfinder, not a scrollable copy: a wheel step therefore
    /// re-aims the engine's camera instead of moving a bitmap around in
    /// the panel.
    Peek(Option<i32>),
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
    /// Freeze the capture without ending the session. The engine keeps
    /// the sheet and its canvas; the next frame after resume
    /// re-anchors against whatever the page shows then.
    pub fn set_paused(&self, paused: bool) {
        let _ = self
            .tx
            .try_send(if paused { Cmd::Pause } else { Cmd::Resume });
    }
    /// Switch growth direction; the engine confirms with a `ScrollEvent::Axis`
    /// (or reports it locked). No-op once the first frame was accepted.
    pub fn set_axis(&self, axis: ScrollAxis) {
        let _ = self.tx.try_send(Cmd::SetAxis(axis));
    }
    /// Aim the preview at history (`Some(preview-row delta)`, negative =
    /// up) or back at the live edge (`None`).
    pub fn peek(&self, delta: Option<i32>) {
        let _ = self.tx.try_send(Cmd::Peek(delta));
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
    options: SheetLimits,
) -> anyhow::Result<(ScrollControls, Receiver<ScrollEvent>)> {
    let (cmd_tx, cmd_rx) = async_channel::bounded(8);
    let (ev_tx, ev_rx) = async_channel::bounded(64);
    std::thread::Builder::new()
        .name("shotori-scroll".into())
        .spawn(move || {
            let result = run(&spec, &options, &ev_tx, &cmd_rx);
            if let Err(e) = result {
                // The UI is gone too (window closed)? Then nobody reads;
                // try_send failing is fine. Pre-canvas failures only —
                // once a sheet exists, the loops report through
                // `fail_with_partial` so the capture so far survives.
                let _ = ev_tx.try_send(ScrollEvent::Failed {
                    reason: format!("{e:#}"),
                    partial: None,
                });
            }
        })
        .context("spawning the scroll thread")?;
    Ok((ScrollControls { tx: cmd_tx }, ev_rx))
}

// ── timing / tuning ──────────────────────────────────────────────────
/// One screencopy frame must arrive within this.
const FRAME_DEADLINE: Duration = Duration::from_millis(2000);
/// Polling cadence. This is the whole sync story: one capture interval
/// is the largest scroll step the matcher ever has to bridge, and it is
/// also the preview's refresh period. The old 60 ms sat ON TOP of the
/// capture's own latency (a screencopy roundtrip), so a cycle was
/// 90–150 ms — one brisk flick then moved more rows than the overlap
/// budget could cover, the frame was refused, the strip froze, and the
/// gap poisoned a whole viewport of good rows. Smaller is better up to
/// the point where screencopies start costing more than they reveal; the
/// sleep is only the REMAINDER of this budget (see `run_manual`), so the
/// cadence is honest whatever the capture costs.
const MANUAL_POLL: Duration = Duration::from_millis(28);
/// Wall budget for the whole session (the user pauses to read; this is a
/// safety valve, not a target).
const MANUAL_DEADLINE: Duration = Duration::from_secs(600);
/// Polls with nothing changed for this long read as "the user stopped
/// scrolling" rather than "the capture is broken": the HUD says
/// *waiting*, which is what a reader pausing mid-capture is doing.
const WAIT_HINT: Duration = Duration::from_millis(1200);

/// Terminal failure that PRESERVES the capture so far: send exactly one
/// `Failed` carrying the partial canvas, then end the loop cleanly
/// (returning `Ok` — the thread wrapper only reports `Err` for
/// failures that predate the sheet, where nothing exists to keep).
fn fail_with_partial(
    events: &Sender<ScrollEvent>,
    sheet: &mut ScrollSheet,
    reason: &str,
) -> anyhow::Result<()> {
    // The partial image must be in OUTPUT orientation: a horizontal
    // session's canvas is still transposed internally.
    let (w, h, rgba) = sheet.output();
    eprintln!("[shotori] scroll: failing with partial canvas: {reason}");
    let _ = events.try_send(ScrollEvent::Failed {
        reason: reason.to_owned(),
        partial: Some((w, h, Arc::new(rgba))),
    });
    Ok(())
}
fn run(
    spec: &ScrollSpec,
    options: &SheetLimits,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
) -> anyhow::Result<()> {
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

    // ── the target output ───────────────────────────────────────────
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

    // ── capture machinery ───────────────────────────────────────────
    // Grace before the first capture: the overlays unmapped when this
    // engine started, and their destroy requests need a few loop passes
    // to reach the compositor (same flush lag the save flow's 150 ms
    // quit delay exists for). Capturing any earlier folds overlay chrome
    // into the seed frame.
    std::thread::sleep(Duration::from_millis(400));
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
    let mut sheet = match spec.axis {
        ScrollAxis::Vertical => ScrollSheet::new(fw, fh, &first, *options),
        ScrollAxis::Horizontal => {
            ScrollSheet::new_with_axis(fw, fh, &first, *options, ScrollAxis::Horizontal)
        }
    }
    .map_err(|r| anyhow!("selection too small for a scroll sheet ({r:?})"))?;
    let axis = spec.axis;
    println!("[shotori] scroll: started {fw}x{fh} on {}", spec.output);
    let _ = events.try_send(ScrollEvent::Started {
        width: fw,
        height: fh,
    });
    let _ = events.try_send(ScrollEvent::Axis {
        axis,
        locked: false,
    });

    let mut preview = Preview::new(spec.preview_rows.clone());
    run_manual(&mut sheet, &mut capturer, events, cmds, &mut preview)
}

/// Ship the finished canvas and log the session summary.
fn deliver(events: &Sender<ScrollEvent>, sheet: &mut ScrollSheet) -> anyhow::Result<()> {
    let (w, h, rgba) = sheet.output();
    let (pushed, accepted) = sheet.stats();
    println!("[shotori] scroll: finished {w}x{h} ({accepted}/{pushed} frames folded)");
    let _ = events.try_send(ScrollEvent::Finished {
        width: w,
        height: h,
        rgba: Arc::new(rgba),
    });
    Ok(())
}

/// The live preview's camera.
///
/// The panel is a **viewfinder**, not a scrollable second copy of the
/// sheet: it paints [a window the size of its own well](preview_window)
/// at a fixed downscale, and the camera keeps the viewport's bottom edge
/// on the window's bottom edge. Two consequences, both deliberate:
///
/// - the strip advances at exactly the scroll rate (one captured row is
///   one strip row, always — the old "tail" grew the image while the
///   sheet grew, which read as a zoom-out and as a lag against the page);
/// - the pixel cost per refresh is bounded by the well, not by the sheet,
///   so a 20 000-row capture previews as cheaply as a 1 000-row one.
struct Preview {
    /// Panel well height in LOGICAL px, written by the panel; 0 = no
    /// panel to feed (chrome-free session) and the engine renders none.
    rows: Arc<AtomicU32>,
    /// Identity of the bitmap the panel currently holds: (window top,
    /// rows, sheet top-shift revision). A frame that only *re-writes*
    /// rows (an up-scroll inside the sheet) changes none of these, so
    /// the pixels are not rebuilt.
    shown: Option<(u32, u32, u32)>,
    /// The panel stepped off the live edge: the sheet row the window
    /// should end at. `None` = follow the viewport.
    peek_end: Option<u32>,
    /// Last reported phase, so the loop speaks only on change.
    phase: Option<Phase>,
}

impl Preview {
    fn new(rows: Arc<AtomicU32>) -> Self {
        Self {
            rows,
            shown: None,
            peek_end: None,
            phase: None,
        }
    }

    /// The window height in SHEET rows: the panel's well, converted with
    /// the same integer `k` the slicer uses. `None` when there is no
    /// panel (or it has not sized itself yet).
    fn window(&self, width: u32) -> Option<(u32, u32)> {
        let well = self.rows.load(Ordering::Relaxed);
        if well == 0 || width == 0 {
            return None;
        }
        let (k, pw) = crate::model::scroll_sheet::preview_scale(width);
        let ph = crate::model::scroll_sheet::preview_fit_rows(well as f32, pw);
        Some((ph * k, k))
    }

    /// Aim, then repaint the panel if its bitmap is out of date. `force`
    /// rebuilds even when nothing moved (axis switch, first frame).
    fn push(&mut self, sheet: &ScrollSheet, events: &Sender<ScrollEvent>, force: bool) {
        let (w, h) = sheet.dimensions();
        let Some((rows, k)) = self.window(w) else {
            return;
        };
        let (vt, vh) = sheet.viewport_span();
        let aimed = match self.peek_end {
            Some(end) => end.saturating_sub(vh),
            None => vt,
        };
        let (top, rows) = crate::model::scroll_sheet::preview_window(h, aimed, vh, rows);
        let identity = (top, rows, sheet.top_revision());
        if force || self.shown != Some(identity) {
            let (pw, ph, rgba) = crate::model::scroll_sheet::preview_slice(
                sheet.canvas(),
                w,
                h,
                top,
                rows,
                crate::model::scroll_sheet::PREVIEW_SAMPLE_W,
            );
            let (v_top, v_h) = self.mapping(sheet, top, rows, k);
            let _ = events.try_send(ScrollEvent::Preview {
                width: pw,
                height: ph,
                rgba: Arc::new(rgba),
                viewport_top: v_top,
                viewport_height: v_h,
            });
            self.shown = Some(identity);
        } else {
            // Pixels are already right (the sheet only rewrote rows in
            // place); the highlight may still have moved.
            if let (Some(v_top), v_h) = self.mapping(sheet, top, rows, k) {
                let _ = events.try_send(ScrollEvent::Viewport {
                    top: v_top,
                    height: v_h,
                });
            }
        }
    }

    /// Where the viewport sits inside the window `[top, top + rows)`,
    /// in preview rows. `None` when it is entirely off the window (the
    /// user is looking at history, so there is no "you are here" band).
    fn mapping(&self, sheet: &ScrollSheet, top: u32, rows: u32, k: u32) -> (Option<u32>, u32) {
        let k = k.max(1);
        let (vt, vh) = sheet.viewport_span();
        let ph = (rows / k).max(1);
        let v_h = (vh / k).max(1).min(ph);
        if vt.saturating_add(vh) <= top || vt >= top.saturating_add(rows) {
            return (None, v_h);
        }
        let v_top = vt.saturating_sub(top) / k;
        (Some(v_top.min(ph - v_h)), v_h)
    }

    /// Wheel in the strip: move the camera by `delta` preview rows
    /// (negative = up), or return to the live edge with `None`.
    fn peek(&mut self, delta: Option<i32>, sheet: &ScrollSheet, events: &Sender<ScrollEvent>) {
        let (w, h) = sheet.dimensions();
        let Some((rows, k)) = self.window(w) else {
            return;
        };
        match delta {
            None => self.peek_end = None,
            Some(delta) => {
                let (vt, vh) = sheet.viewport_span();
                let base = self.peek_end.unwrap_or((vt + vh).min(h)) as i64;
                let step = i64::from(delta) * i64::from(k);
                let lowest = rows.min(h) as i64;
                self.peek_end = Some((base - step).clamp(lowest, i64::from(h)) as u32);
            }
        }
        self.push(sheet, events, true);
    }

    /// First paint: the panel announces its height a moment after the
    /// engine starts (it opens after this thread does), so the initial
    /// slice cannot be rendered at launch. Cheap to call every poll —
    /// two atomics until the first slice exists.
    fn ensure_first(&mut self, sheet: &ScrollSheet, events: &Sender<ScrollEvent>) {
        if self.shown.is_none() {
            self.push(sheet, events, true);
        }
    }

    /// Report a phase transition (deduplicated — the loop calls this on
    /// every frame).
    fn report(&mut self, phase: Phase, events: &Sender<ScrollEvent>) {
        if self.phase != Some(phase) {
            self.phase = Some(phase);
            let _ = events.try_send(ScrollEvent::Phase(phase));
        }
    }
}

/// The capture loop: the user scrolls, the engine watches. Pure capture
/// and fold polling — the engine never injects input. Termination is the
/// user's Finish/Cancel, the height cap or the session deadline.
///
/// Nothing the MATCHER says is fatal any more. A frame that cannot be
/// placed — a tear mid-animation, a flick faster than the overlap
/// budget, fixed chrome inside the region — increments the refusal
/// counter and the HUD says "no overlap"; the loop keeps sampling, and
/// the next frame that aligns folds in. The old loop ended the session
/// after a few refusals, which looked exactly like a crash: the capture
/// window vanished and a notification said "stopped early" while the
/// user was still scrolling.
fn run_manual(
    sheet: &mut ScrollSheet,
    capturer: &mut Capturer<'_>,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
    preview: &mut Preview,
) -> anyhow::Result<()> {
    println!("[shotori] scroll: started — you scroll, the engine folds");
    let deadline = Instant::now() + MANUAL_DEADLINE;
    preview.push(sheet, events, true);
    // (frames in a row that could not be placed, why the newest one
    // failed) — the chip says different words for "you scrolled too
    // fast" and "this area never sits still".
    let mut streak: Option<(u32, Refusal)> = None;
    let mut last_accept = Instant::now();
    let mut paused = false;
    loop {
        let tick = Instant::now();
        if !paused && Instant::now() > deadline {
            return fail_with_partial(
                events,
                sheet,
                "Nothing new was captured for ten minutes — the session timed out.",
            );
        }
        match cmds.try_recv() {
            Ok(Cmd::Finish) => return deliver(events, sheet),
            Ok(Cmd::Cancel) => {
                let _ = events.try_send(ScrollEvent::Cancelled);
                return Ok(());
            }
            Ok(Cmd::Pause) => {
                paused = true;
                println!("[shotori] scroll: paused");
                continue;
            }
            Ok(Cmd::Resume) => {
                paused = false;
                println!("[shotori] scroll: resumed");
                continue;
            }
            Ok(Cmd::SetAxis(next)) => {
                if sheet.set_axis(next) {
                    println!("[shotori] scroll: axis → {next:?}");
                    // The seed was transposed; refresh the strip and
                    // highlight now instead of waiting for a frame.
                    preview.push(sheet, events, true);
                }
                let _ = events.try_send(ScrollEvent::Axis {
                    axis: sheet.axis(),
                    locked: sheet.axis_locked(),
                });
                continue;
            }
            Ok(Cmd::Peek(delta)) => {
                preview.peek(delta, sheet, events);
                continue;
            }
            Err(_) => {}
        }
        if paused {
            std::thread::sleep(MANUAL_POLL);
            continue;
        }
        preview.ensure_first(sheet, events);
        let frame = match capturer.capture_once() {
            Ok(frame) => frame,
            Err(e) => {
                return fail_with_partial(
                    events,
                    sheet,
                    &format!("The compositor stopped delivering frames: {e:#}"),
                );
            }
        };
        let outcome = sheet.push(&frame);
        if !matches!(outcome, Fold::Unchanged) {
            println!(
                "[shotori] scroll: frame #{} → {outcome:?} ({}ms)",
                sheet.stats().0,
                tick.elapsed().as_millis()
            );
        }
        match outcome {
            Fold::GrewBottom { .. } | Fold::GrewTop { .. } | Fold::Shifted { .. } => {
                last_accept = Instant::now();
                streak = None;
                let _ = events.try_send(ScrollEvent::Progress {
                    extent: sheet.captured_extent(),
                    total: sheet.dimensions().1,
                });
            }
            Fold::Refused(Refusal::TooTall) => {
                println!("[shotori] scroll: height cap reached");
                return deliver(events, sheet);
            }
            Fold::Refused(Refusal::TooSmall) => {
                return fail_with_partial(
                    events,
                    sheet,
                    "The capture region changed size mid-session.",
                );
            }
            // Everything else is a normal, recoverable state: nothing was
            // written, the base stays where it was, and the next frame
            // that still overlaps it links up. The chip says which kind of
            // trouble it is; the session lives on.
            Fold::Refused(reason) => {
                streak = Some(match streak {
                    Some((count, _)) => (count.saturating_add(1), reason),
                    None => (1, reason),
                });
            }
            // A still frame carries no unplaceable motion: the ladder has
            // nothing to count, or a reader who paused mid-flick would
            // leave the chip stuck on a warning forever.
            Fold::Unchanged | Fold::Still => streak = None,
        }
        // Phase: the refusal ladder is a pure model function; the
        // "waiting" reading is a clock and lives here.
        let phase = match streak {
            None if sheet.has_incomplete_column() => Phase::Unstable,
            None if last_accept.elapsed() >= WAIT_HINT => Phase::Waiting,
            None => Phase::Following,
            Some((count, reason)) => phase_of(count, reason),
        };
        preview.report(phase, events);
        // Pixels only move when the sheet or the viewport did.
        if !matches!(outcome, Fold::Unchanged | Fold::Still) {
            preview.push(sheet, events, false);
        }
        // Sleep only what the cycle has left: the capture roundtrip is
        // the real cost, and adding the full budget on top of it halved
        // the sampling rate for nothing.
        std::thread::sleep(MANUAL_POLL.saturating_sub(tick.elapsed()));
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
    /// capture — the sheet's viewport geometry.
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

// ── the event state machine ──────────────────────────────────────────

#[derive(Default)]
struct EngineState {
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    /// The bound screencopy manager version (gates the buffer_done
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
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
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
