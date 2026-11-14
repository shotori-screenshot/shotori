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
//! Split by concern:
//! - [`scroll_capture`](self): public contract (`ScrollSpec`,
//!   `ScrollEvent`, `ScrollControls`) and thread entrypoint;
//! - `session`: pure capture-loop coordinator and preview camera,
//!   unit-tested against synthetic frame streams without a compositor;
//! - `wayland`: `zwlr_screencopy_manager_v1` connection, shm buffer
//!   reuse and protocol dispatch.

use std::sync::Arc;

use anyhow::Context as _;
use async_channel::{Receiver, Sender};

use crate::model::scroll_sheet::{Axis as ScrollAxis, Phase, PreviewGeometry, SheetLimits};
use crate::model::session::ScrollRect;

mod session;
mod wayland;

/// Which screen to watch and where (output-local LOGICAL integer px —
/// the `capture_output_region` contract).
pub struct ScrollSpec {
    pub output: String,
    /// The capture rectangle, SHARED with the UI: dragging the region
    /// frame rewrites it and the next poll captures the new spot (w/h
    /// stay fixed — drags move, never resize).
    pub rect: Arc<std::sync::Mutex<ScrollRect>>,
    /// Initial growth direction; the HUD can switch it until the first
    /// frame is accepted.
    pub axis: ScrollAxis,
    /// Shared preview well height (logical px) and physical sample
    /// width published by the preview panel; unset (`None`) marks a
    /// chrome-free session where no preview pixels are rendered.
    pub preview: PreviewGeometry,
}

/// What the engine reports to the UI. `Finished` carries the folded
/// image; every terminal event (`Finished`/`Failed`/`Cancelled`) is sent
/// exactly once, then the thread exits.
#[derive(Debug)]
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
    /// box-filtered by [`crate::model::scroll_sheet::preview_slice`] at
    /// the panel's published sample width, cut at the window
    /// [`preview_window`](crate::model::scroll_sheet::preview_window)
    /// picked. `viewport_*` place the capture viewport inside that
    /// window (preview rows; `None` when peeking at history).
    Preview {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
        viewport_top: Option<u32>,
        viewport_height: u32,
    },
    /// What the session is doing right now (see [`Phase`]). Fired on
    /// change only. A refusal is a STATE, not a failure: the session
    /// keeps sampling and the chip explains what happened instead of the
    /// capture dying under the user.
    Phase(Phase),
    /// The canvas is complete (user finish or command channel closed).
    Finished {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
    },
    /// Unrecoverable (missing protocol, unsupported output, idle
    /// timeout, compositor disconnect). `partial` carries whatever was
    /// folded before the failure — a session that captured something
    /// must never silently discard it; the UI copies/saves it and says
    /// so in the notification.
    Failed {
        reason: String,
        /// (width, height, rgba) of the canvas so far, when one exists.
        partial: Option<(u32, u32, Arc<Vec<u8>>)>,
    },
    /// User cancelled; nothing is delivered.
    Cancelled,
}

pub(super) enum Cmd {
    /// Stop scrolling and deliver the canvas as-is.
    Finish,
    /// Stop and discard.
    Cancel,
    /// Stop sampling; keep the session and its canvas alive.
    Pause,
    /// Resume after [`Cmd::Pause`].
    Resume,
    /// Switch growth direction (ignored once locked).
    SetAxis(ScrollAxis),
    /// Step the preview away from the live edge (`Some(preview rows)`,
    /// negative = look further up) or return to it (`None`).
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
        drop(self.tx.try_send(Cmd::Finish));
    }

    pub fn cancel(&self) {
        drop(self.tx.try_send(Cmd::Cancel));
    }

    /// Freeze the capture without ending the session. The engine keeps
    /// the sheet and its canvas; the next frame after resume
    /// re-anchors against whatever the page shows then.
    pub fn set_paused(&self, paused: bool) {
        drop(
            self.tx
                .try_send(if paused { Cmd::Pause } else { Cmd::Resume }),
        );
    }

    /// Switch growth direction; the engine confirms with a `ScrollEvent::Axis`
    /// (or reports it locked). No-op once the first frame was accepted.
    pub fn set_axis(&self, axis: ScrollAxis) {
        drop(self.tx.try_send(Cmd::SetAxis(axis)));
    }

    /// Aim the preview at history (`Some(preview-row delta)`, negative =
    /// up) or back at the live edge (`None`).
    pub fn peek(&self, delta: Option<i32>) {
        drop(self.tx.try_send(Cmd::Peek(delta)));
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
    let (cmd_tx, cmd_rx) = async_channel::bounded(16);
    // Unbounded so lightweight state transitions (`Phase`, `Axis`) and
    // terminal deliverables (`Finished`, `Failed`, `Cancelled`) can
    // NEVER be silently dropped if the UI loop stalls briefly; heavy
    // `Preview` bitmaps self-throttle on `ev_tx.len()` in `session.rs`.
    let (ev_tx, ev_rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("shotori-scroll".into())
        .spawn(move || {
            if let Err(err) = wayland::run(&spec, &options, &ev_tx, &cmd_rx) {
                // Pre-canvas failures only — once a sheet exists,
                // `session::run_session` reports via `fail_with_partial`
                // so the captured rows survive.
                drop(ev_tx.try_send(ScrollEvent::Failed {
                    reason: err.user_message(),
                    partial: None,
                }));
            }
        })
        .context("spawning the scroll thread")?;
    Ok((ScrollControls { tx: cmd_tx }, ev_rx))
}
