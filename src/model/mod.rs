//! # Model: pure logic and state — no elements, no Wayland wire code
//!
//! The testable heart of shotori. Everything here is a plain function or
//! a state machine over `gpui_kit` geometry types:
//! - [`selection`]: the Idle → Dragging → Selected lifecycle
//! - [`session`]: state shared by every per-output overlay (multi-monitor
//!   selections, hover tracking, click-snapping, cropping)
//! - [`export`]: logical bounds → physical crop → PNG
//! - [`placement`]: the two-zone chrome layout contract (label on top,
//!   toolbar at the bottom — disjoint by construction)
//! - [`scroll_stitch`]: the long-screenshot assembler (viewport state
//!   machine + column-sampled frame matching)

pub mod export;
pub mod placement;
pub mod scroll_stitch;
pub mod selection;
pub mod session;
