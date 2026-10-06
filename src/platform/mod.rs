//! # Platform: compositor and desktop integration
//!
//! Everything that talks past gpui to the outside world, behind
//! platform-neutral signatures:
//! - [`capture`]: screen freeze of all outputs — wlr-screencopy (the
//!   `wayland` submodule is the event state machine); `pixels` is pure
//!   processing
//! - [`display`]: capture ↔ gpui display matching (position-based)
//! - [`windowsnap`]: window-rect backends for click-to-capture —
//!   niri / sway / Hyprland IPC
//! - [`scroll_capture`]: the long-screenshot engine (virtual-pointer
//!   wheel injection + region re-capture, one dedicated connection)

pub mod capture;
pub mod display;
pub mod scroll_capture;
pub mod windowsnap;
