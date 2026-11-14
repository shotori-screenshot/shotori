//! Pure scroll-session coordinator and preview camera.
//!
//! Separated from Wayland wire mechanics behind [`FrameSource`] so the
//! idle timeout, pause/resume clock, height-cap handling, HiDPI preview
//! scaling and backpressure guarantees are unit-tested deterministically
//! without a compositor.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};

use super::{Cmd, ScrollEvent};
use crate::model::scroll_sheet::{
    Fold, Phase, PreviewGeometry, Refusal, ScrollSheet, phase_of, preview_fit_rows,
    preview_scale_for, preview_slice, preview_window,
};

/// Polling cadence. One capture interval is the largest scroll step the
/// matcher ever has to bridge, and it is also the preview's refresh
/// period. We sleep only the REMAINDER of this budget after capture +
/// matching so the real cadence is `max(capture_cost, MANUAL_POLL)`.
const MANUAL_POLL: Duration = Duration::from_millis(28);

/// True inactivity timeout measured from the last accepted fold (and
/// refreshed on resume after a pause). Actively scrolling sessions stay
/// alive indefinitely; an abandoned session stops after ten idle
/// minutes while preserving everything captured so far.
const IDLE_DEADLINE: Duration = Duration::from_secs(600);

/// Polls with nothing changed for this long read as "the user stopped
/// scrolling" rather than "the capture is broken": the HUD says
/// *waiting*, which is what a reader pausing mid-capture is doing.
const WAIT_HINT: Duration = Duration::from_millis(1200);

/// Maximum unconsumed events in the channel before non-forced preview
/// bitmaps skip slicing. Terminal and state events always go through
/// (the channel is unbounded); this bounds preview memory and CPU when
/// the UI event loop lags behind the capture thread.
const MAX_QUEUED_EVENTS: usize = 8;

/// Timing parameters for [`run_session`], injectable by unit tests.
#[derive(Clone, Copy)]
pub(super) struct SessionTimings {
    pub poll: Duration,
    pub idle_deadline: Duration,
    pub wait_hint: Duration,
}

impl Default for SessionTimings {
    fn default() -> Self {
        Self {
            poll: MANUAL_POLL,
            idle_deadline: IDLE_DEADLINE,
            wait_hint: WAIT_HINT,
        }
    }
}

/// Source of physical-pixel RGBA frames for [`run_session`].
pub(super) trait FrameSource {
    fn capture_once(&mut self) -> anyhow::Result<Vec<u8>>;
}

/// Terminal failure that PRESERVES the capture so far: send one
/// `Failed` carrying the partial canvas, then end the loop cleanly.
fn fail_with_partial(events: &Sender<ScrollEvent>, sheet: &mut ScrollSheet, reason: String) {
    // The partial image must be in OUTPUT orientation: a horizontal
    // session's canvas is still transposed internally.
    let (w, h, rgba) = sheet.output();
    eprintln!("[shotori] scroll: failing with partial canvas: {reason}");
    drop(events.try_send(ScrollEvent::Failed {
        reason,
        partial: Some((w, h, Arc::new(rgba))),
    }));
}

/// Ship the finished canvas and log the session summary.
fn deliver(events: &Sender<ScrollEvent>, sheet: &mut ScrollSheet) {
    let (w, h, rgba) = sheet.output();
    let (pushed, accepted) = sheet.stats();
    println!("[shotori] scroll: finished {w}x{h} ({accepted}/{pushed} frames folded)");
    drop(events.try_send(ScrollEvent::Finished {
        width: w,
        height: h,
        rgba: Arc::new(rgba),
    }));
}

/// The live preview's camera.
///
/// The panel is a **viewfinder**, not a scrollable second copy of the
/// sheet: it paints [a window the size of its own well](preview_window)
/// at a fixed downscale derived from the panel's published physical
/// sample width, keeping the viewport's bottom edge on the window's
/// bottom edge.
pub(super) struct Preview {
    geom: PreviewGeometry,
    /// Identity of the bitmap the panel currently holds: `(window top,
    /// rows, sample_w, sheet top-shift revision)`. Including `sample_w`
    /// ensures a panel geometry update immediately refreshes the slice.
    shown: Option<(u32, u32, u32, u32)>,
    /// The panel stepped off the live edge: the sheet row the window
    /// should end at. `None` = follow the viewport.
    peek_end: Option<u32>,
    /// Last delivered phase, recorded only after `try_send` succeeds.
    phase: Option<Phase>,
}

impl Preview {
    pub(super) fn new(geom: PreviewGeometry) -> Self {
        Self {
            geom,
            shown: None,
            peek_end: None,
            phase: None,
        }
    }

    /// True when no preview panel has published metrics (chrome-free
    /// full-screen single-monitor session).
    fn is_chrome_free(&self) -> bool {
        self.geom.get().is_none()
    }

    /// `(window_rows_in_sheet, downscale_k, sample_w)` for `width`, or
    /// `None` when there is no panel (or it has not sized itself yet).
    fn window(&self, width: u32) -> Option<(u32, u32, u32)> {
        let (well_rows, sample_w) = self.geom.get()?;
        if width == 0 {
            return None;
        }
        let (k, pw) = preview_scale_for(width, sample_w);
        let ph = preview_fit_rows(well_rows as f32, pw);
        Some((ph * k, k, sample_w))
    }

    /// Aim, then repaint the panel if its bitmap is out of date. `force`
    /// rebuilds even when nothing moved (axis switch, peek, first frame).
    fn push(&mut self, sheet: &ScrollSheet, events: &Sender<ScrollEvent>, force: bool) {
        let (w, h) = sheet.dimensions();
        let Some((rows, k, sample_w)) = self.window(w) else {
            return;
        };
        let (vt, vh) = sheet.viewport_span();
        let aimed = match self.peek_end {
            Some(end) => end.saturating_sub(vh),
            None => vt,
        };
        let (top, rows) = preview_window(h, aimed, vh, rows);
        let identity = (top, rows, sample_w, sheet.top_revision());
        if force || self.shown != Some(identity) {
            if !force && events.len() >= MAX_QUEUED_EVENTS {
                // Leave `self.shown` stale so the next poll after the UI
                // drains the queue slices the latest canvas once instead
                // of piling up multi-megabyte intermediate frames.
                return;
            }
            let (pw, ph, rgba) = preview_slice(sheet.canvas(), w, h, top, rows, sample_w);
            let (v_top, v_h) = self.mapping(sheet, top, rows, k);
            if events
                .try_send(ScrollEvent::Preview {
                    width: pw,
                    height: ph,
                    rgba: Arc::new(rgba),
                    viewport_top: v_top,
                    viewport_height: v_h,
                })
                .is_ok()
            {
                self.shown = Some(identity);
            }
        } else if events.len() < MAX_QUEUED_EVENTS
            && let (Some(v_top), v_h) = self.mapping(sheet, top, rows, k)
        {
            drop(events.try_send(ScrollEvent::Viewport {
                top: v_top,
                height: v_h,
            }));
        }
    }

    /// Where the viewport sits inside `[top, top + rows)` in preview rows.
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
        let Some((rows, k, _)) = self.window(w) else {
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

    /// First paint once the panel publishes its dimensions.
    fn ensure_first(&mut self, sheet: &ScrollSheet, events: &Sender<ScrollEvent>) {
        if self.shown.is_none() {
            self.push(sheet, events, true);
        }
    }

    /// Report a phase transition, recording `self.phase` ONLY after the
    /// event actually enters the channel so a transient send failure
    /// retries on the next poll instead of desynchronising the HUD.
    fn report(&mut self, phase: Phase, events: &Sender<ScrollEvent>) {
        if self.phase != Some(phase) && events.try_send(ScrollEvent::Phase(phase)).is_ok() {
            self.phase = Some(phase);
        }
    }
}

/// Pure capture-and-fold loop: watches `capturer`, folds into `sheet`,
/// drives `preview`, and honours commands until a terminal condition.
pub(super) fn run_session(
    sheet: &mut ScrollSheet,
    capturer: &mut impl FrameSource,
    events: &Sender<ScrollEvent>,
    cmds: &Receiver<Cmd>,
    preview: &mut Preview,
    timings: SessionTimings,
) {
    println!("[shotori] scroll: started — you scroll, the engine folds");
    preview.push(sheet, events, true);
    let mut streak: Option<(u32, Refusal)> = None;
    let mut last_accept = Instant::now();
    let mut paused = false;
    loop {
        let tick = Instant::now();
        if !paused && last_accept.elapsed() > timings.idle_deadline {
            fail_with_partial(
                events,
                sheet,
                crate::i18n::t!("scroll.fail.idle_timeout").into_owned(),
            );
            return;
        }
        match cmds.try_recv() {
            Ok(Cmd::Finish) => {
                deliver(events, sheet);
                return;
            }
            Ok(Cmd::Cancel) => {
                drop(events.try_send(ScrollEvent::Cancelled));
                return;
            }
            Ok(Cmd::Pause) => {
                paused = true;
                println!("[shotori] scroll: paused");
                continue;
            }
            Ok(Cmd::Resume) => {
                paused = false;
                // A deliberate pause is not idle neglect: give the user
                // a fresh idle budget from the moment sampling resumes.
                last_accept = Instant::now();
                println!("[shotori] scroll: resumed");
                continue;
            }
            Ok(Cmd::SetAxis(next)) => {
                if sheet.set_axis(next) {
                    println!("[shotori] scroll: axis → {next:?}");
                    preview.push(sheet, events, true);
                }
                drop(events.try_send(ScrollEvent::Axis {
                    axis: sheet.axis(),
                    locked: sheet.axis_locked(),
                }));
                continue;
            }
            Ok(Cmd::Peek(delta)) => {
                preview.peek(delta, sheet, events);
                continue;
            }
            Err(async_channel::TryRecvError::Empty) => {}
            Err(async_channel::TryRecvError::Closed) => {
                // The UI dropped its command channel: deliver what we
                // have rather than leaking a zombie capture thread.
                deliver(events, sheet);
                return;
            }
        }
        if paused {
            std::thread::sleep(timings.poll);
            continue;
        }
        preview.ensure_first(sheet, events);
        let frame = match capturer.capture_once() {
            Ok(frame) => frame,
            Err(e) => {
                fail_with_partial(
                    events,
                    sheet,
                    crate::i18n::t!("scroll.fail.capture_lost", error = format!("{e:#}"))
                        .into_owned(),
                );
                return;
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
                drop(events.try_send(ScrollEvent::Progress {
                    extent: sheet.captured_extent(),
                    total: sheet.dimensions().1,
                }));
            }
            Fold::Refused(Refusal::TooTall) => {
                if preview.is_chrome_free() {
                    // No panel exists to show a truncation chip: stop
                    // now with a localized partial-delivery notice so
                    // the user knows why the image stopped growing.
                    fail_with_partial(
                        events,
                        sheet,
                        crate::i18n::t!("scroll.fail.max_length").into_owned(),
                    );
                    return;
                }
                // Panel session: keep the window open and surface
                // `Phase::MaxLength` in the chip so the truncation is
                // unmistakable and the user can choose Copy or Save.
                streak = Some((1, Refusal::TooTall));
            }
            Fold::Refused(Refusal::TooSmall) => {
                fail_with_partial(
                    events,
                    sheet,
                    crate::i18n::t!("scroll.fail.region_changed").into_owned(),
                );
                return;
            }
            Fold::Refused(reason) => {
                streak = Some(match streak {
                    Some((count, _)) => (count.saturating_add(1), reason),
                    None => (1, reason),
                });
            }
            // A still frame clears transient refusal streaks, EXCEPT
            // `TooTall`: pausing at the height cap does not make room
            // for more rows, so the truncation chip must stay visible.
            Fold::Unchanged | Fold::Still => {
                if !matches!(streak, Some((_, Refusal::TooTall))) {
                    streak = None;
                }
            }
        }
        let phase = match streak {
            None if sheet.has_incomplete_column() => Phase::Unstable,
            None if last_accept.elapsed() >= timings.wait_hint => Phase::Waiting,
            None => Phase::Following,
            Some((count, reason)) => phase_of(count, reason),
        };
        preview.report(phase, events);
        if !matches!(outcome, Fold::Unchanged | Fold::Still) {
            preview.push(sheet, events, false);
        }
        std::thread::sleep(timings.poll.saturating_sub(tick.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::model::scroll_sheet::SheetLimits;

    const W: u32 = 160;
    const H: u32 = 160;

    /// Non-periodic textured page so `ScrollSheet` matches exact offsets.
    fn frame_at(y0: u32) -> Vec<u8> {
        let mut rgba = vec![248u8; (W * H * 4) as usize];
        for y in 0..H {
            let py = y0 + y;
            for x in 0..W {
                let ink = ((py.wrapping_mul(131) ^ x.wrapping_mul(17)).wrapping_add(py / 7))
                    .is_multiple_of(29)
                    && (12..W - 12).contains(&x);
                let i = ((y * W + x) * 4) as usize;
                let v = if ink { 28 } else { 248 };
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = (py % 251) as u8;
                rgba[i + 3] = 255;
            }
        }
        rgba
    }

    struct ScriptedFrames {
        frames: VecDeque<Vec<u8>>,
        last: Vec<u8>,
    }

    impl ScriptedFrames {
        fn new(frames: impl IntoIterator<Item = Vec<u8>>) -> Self {
            let mut frames: VecDeque<Vec<u8>> = frames.into_iter().collect();
            let last = frames.back().cloned().unwrap_or_else(|| frame_at(0));
            if frames.is_empty() {
                frames.push_back(last.clone());
            }
            Self { frames, last }
        }
    }

    impl FrameSource for ScriptedFrames {
        fn capture_once(&mut self) -> anyhow::Result<Vec<u8>> {
            Ok(self.frames.pop_front().unwrap_or_else(|| self.last.clone()))
        }
    }

    #[test]
    fn active_scrolling_refreshes_idle_deadline_until_truly_idle() {
        let mut sheet =
            ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).expect("valid seed");
        // Each step takes 8 ms (`poll`); idle deadline is 22 ms. Four
        // active scroll steps span ~32 ms (> 22 ms wall clock) and MUST
        // all succeed because each accepted fold refreshes `last_accept`;
        // once scrolling stops, the session times out and preserves the
        // partial canvas.
        let mut src = ScriptedFrames::new([frame_at(24), frame_at(48), frame_at(72), frame_at(96)]);
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (_cmd_tx, cmd_rx) = async_channel::bounded(4);
        let mut preview = Preview::new(PreviewGeometry::new());
        run_session(
            &mut sheet,
            &mut src,
            &ev_tx,
            &cmd_rx,
            &mut preview,
            SessionTimings {
                poll: Duration::from_millis(8),
                idle_deadline: Duration::from_millis(22),
                wait_hint: Duration::from_millis(10),
            },
        );
        let mut partial_h = None;
        let mut reason_msg = String::new();
        while let Ok(ev) = ev_rx.try_recv() {
            if let ScrollEvent::Failed {
                reason,
                partial: Some((_, h, _)),
            } = ev
            {
                reason_msg = reason;
                partial_h = Some(h);
            }
        }
        assert_eq!(
            partial_h,
            Some(H + 96),
            "all four folds past wall deadline must be preserved before idle expiry"
        );
        assert!(
            !reason_msg.is_empty(),
            "idle timeout must carry localized reason"
        );
    }

    #[test]
    fn pause_suspends_idle_timeout_and_resume_refreshes_budget() {
        let mut sheet =
            ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).expect("valid seed");
        let mut src = ScriptedFrames::new([frame_at(24)]);
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (cmd_tx, cmd_rx) = async_channel::bounded(8);
        cmd_tx.try_send(Cmd::Pause).expect("queue pause");

        let handle = std::thread::spawn(move || {
            let mut preview = Preview::new(PreviewGeometry::new());
            run_session(
                &mut sheet,
                &mut src,
                &ev_tx,
                &cmd_rx,
                &mut preview,
                SessionTimings {
                    poll: Duration::from_millis(4),
                    idle_deadline: Duration::from_millis(20),
                    wait_hint: Duration::from_millis(10),
                },
            );
            ev_rx
        });

        // Stay paused well past `idle_deadline`, then resume + finish.
        std::thread::sleep(Duration::from_millis(40));
        cmd_tx.try_send(Cmd::Resume).expect("queue resume");
        std::thread::sleep(Duration::from_millis(10));
        cmd_tx.try_send(Cmd::Finish).expect("queue finish");

        let ev_rx = handle.join().expect("session thread");
        let mut finished_h = None;
        while let Ok(ev) = ev_rx.try_recv() {
            if let ScrollEvent::Finished { height, .. } = ev {
                finished_h = Some(height);
            }
        }
        assert_eq!(finished_h, Some(H + 24));
    }

    #[test]
    fn height_cap_surfaces_max_length_phase_on_panel_and_waits_for_finish() {
        let limits = SheetLimits {
            max_height: H + 40,
            ..SheetLimits::default()
        };
        let mut sheet = ScrollSheet::new(W, H, &frame_at(0), limits).expect("valid seed");
        // First scroll reaches H + 30 (ok); second would reach H + 60
        // (`TooTall`), then stays still until user finishes.
        let mut src = ScriptedFrames::new([frame_at(30), frame_at(60), frame_at(60)]);
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (cmd_tx, cmd_rx) = async_channel::bounded(4);
        let geom = PreviewGeometry::new();
        geom.set(320, PreviewGeometry::sample_width_for_scale(2.0));

        let handle = std::thread::spawn(move || {
            let mut preview = Preview::new(geom);
            run_session(
                &mut sheet,
                &mut src,
                &ev_tx,
                &cmd_rx,
                &mut preview,
                SessionTimings {
                    poll: Duration::from_millis(3),
                    idle_deadline: Duration::from_secs(5),
                    wait_hint: Duration::from_secs(1),
                },
            );
            ev_rx
        });

        std::thread::sleep(Duration::from_millis(25));
        cmd_tx.try_send(Cmd::Finish).expect("finish after cap");
        let ev_rx = handle.join().expect("session thread");

        let mut phases = Vec::new();
        let mut preview_widths = Vec::new();
        let mut finished_h = None;
        while let Ok(ev) = ev_rx.try_recv() {
            match ev {
                ScrollEvent::Phase(p) => phases.push(p),
                ScrollEvent::Preview { width, .. } => preview_widths.push(width),
                ScrollEvent::Finished { height, .. } => finished_h = Some(height),
                _ => {}
            }
        }
        assert!(
            phases.contains(&Phase::MaxLength),
            "panel must surface Phase::MaxLength and keep it across still frames, got {phases:?}"
        );
        assert_eq!(
            phases.last().copied(),
            Some(Phase::MaxLength),
            "still frames at the cap must not clear the MaxLength chip: {phases:?}"
        );
        assert!(
            preview_widths.contains(&W),
            "2× sample_w (508) >= W (160) must keep k=1 crisp width, got {preview_widths:?}"
        );
        assert_eq!(finished_h, Some(H + 30));
    }

    #[test]
    fn height_cap_delivers_partial_with_notice_when_chrome_free() {
        let limits = SheetLimits {
            max_height: H + 40,
            ..SheetLimits::default()
        };
        let mut sheet = ScrollSheet::new(W, H, &frame_at(0), limits).expect("valid seed");
        let mut src = ScriptedFrames::new([frame_at(30), frame_at(60)]);
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (_cmd_tx, cmd_rx) = async_channel::bounded(4);
        // Chrome-free: `PreviewGeometry::new()` stays unset (`None`).
        let mut preview = Preview::new(PreviewGeometry::new());
        run_session(
            &mut sheet,
            &mut src,
            &ev_tx,
            &cmd_rx,
            &mut preview,
            SessionTimings {
                poll: Duration::from_millis(1),
                idle_deadline: Duration::from_secs(5),
                wait_hint: Duration::from_secs(1),
            },
        );

        let mut got_partial = None;
        while let Ok(ev) = ev_rx.try_recv() {
            if let ScrollEvent::Failed {
                reason,
                partial: Some((_, h, _)),
            } = ev
            {
                got_partial = Some((reason, h));
            }
        }
        let (reason, h) = got_partial.expect("chrome-free cap must deliver partial");
        assert_eq!(h, H + 30);
        assert!(!reason.is_empty());
    }

    #[test]
    fn closed_command_channel_delivers_sheet_instead_of_hanging() {
        let mut sheet =
            ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).expect("valid seed");
        let mut src = ScriptedFrames::new([frame_at(0)]);
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (cmd_tx, cmd_rx) = async_channel::bounded(1);
        drop(cmd_tx);
        let mut preview = Preview::new(PreviewGeometry::new());
        run_session(
            &mut sheet,
            &mut src,
            &ev_tx,
            &cmd_rx,
            &mut preview,
            SessionTimings::default(),
        );
        let mut finished = false;
        while let Ok(ev) = ev_rx.try_recv() {
            if matches!(ev, ScrollEvent::Finished { height: H, .. }) {
                finished = true;
            }
        }
        assert!(finished);
    }
}
