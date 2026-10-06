//! # Window snapping: hover-highlight + click-to-select a whole window
//!
//! Wayland isolation means no client can ask "what window is under my
//! cursor" — no standard protocol exposes other clients' geometry. The
//! practical path is per-compositor IPC. Each backend
//! reports rects in its compositor's **global logical space**, which is
//! the same space [`crate::model::session`]'s selection state machine already
//! uses (verified against niri: output logical positions match the
//! capture geometry).
//!
//! ## Coverage on niri (source-verified, 26.04)
//!
//! - **Floating windows: exact rects.** `tile_pos_in_workspace_view` is
//!   populated only for floating windows (niri's scrolling layout leaves
//!   it null for tiled ones — `tiles_with_ipc_layouts` never fills it;
//!   see upstream issue #2381 / PR #4147 proposing to expose the view
//!   offset). That field doubles as the visibility filter.
//! - **Tiled windows: not available.** Their positions depend on the
//!   workspace scroll offset, which the IPC does not expose, and column
//!   packing cannot be reconstructed from `(column, tile)` indices
//!   alone. Pixel-based detection was prototyped and rejected: content
//!   edges inside windows score as strongly as real window boundaries
//!   (measured), so snapping would sometimes grab a wrong region —
//!   silently degrading beats mis-snapping.
//! - The day upstream exposes tile positions, this backend lights up
//!   for tiled windows with no architectural change.
//!
//! Backends (detected via environment variables, first hit wins):
//! - niri — `NIRI_SOCKET`, typed via the `niri-ipc` crate
//! - sway — `SWAYSOCK`, i3 IPC `GET_TREE` (rect field, decade-stable)
//! - Hyprland — `HYPRLAND_INSTANCE_SIGNATURE`, request socket `j/clients`
//!
//! Anything else (GNOME, KDE, river, labwc) → `query()` returns None and
//! the feature is silently off; behavior is identical to pre-snap builds.
//!
//! Interaction (see [`crate::ui::overlay`]): hovering outlines the window
//! under the cursor; an in-place click selects its rect; dragging keeps
//! the classic freehand region.

#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "linux")]
mod niri;
#[cfg(target_os = "linux")]
mod sway;

use gpui_kit::*;

/// A visible toplevel in global logical coordinates, ready for
/// hit-testing. Built by whichever backend found its compositor.
/// `pub` with crate-private fields: the binary passes it through
/// opaquely into the session.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapRect {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) app_id: String,
    pub(crate) focused: bool,
    /// Millisecond recency of the last focus (a stacking tiebreaker);
    /// backends without timestamps report 0
    pub(crate) recency: u64,
}

/// Query the running compositor once. None = no supported compositor.
/// The whole IPC exchange runs on a helper thread with a hard 1s cap:
/// a wedged socket must never freeze the overlay startup.
pub fn query() -> Option<Vec<SnapRect>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(detect_and_query());
    });
    rx.recv_timeout(std::time::Duration::from_secs(1))
        .ok()
        .flatten()
}

fn detect_and_query() -> Option<Vec<SnapRect>> {
    if std::env::var_os("NIRI_SOCKET").is_some()
        && let Some(rects) = niri::query()
    {
        return Some(rects);
    }
    if std::env::var_os("SWAYSOCK").is_some()
        && let Some(rects) = sway::query()
    {
        return Some(rects);
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && let Some(rects) = hyprland::query()
    {
        return Some(rects);
    }
    None
}

/// Which window (if any) contains a point. Focused wins, then the most
/// recently focused, then the smallest area — for overlapping floating
/// windows the innermost rect is almost always the intended target.
pub(crate) fn hit_test(rects: &[SnapRect], p: Point<Pixels>) -> Option<usize> {
    let (px, py) = (f32::from(p.x), f32::from(p.y));
    rects
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let b = r.bounds;
            px >= f32::from(b.left())
                && px <= f32::from(b.right())
                && py >= f32::from(b.top())
                && py <= f32::from(b.bottom())
        })
        .max_by(|(_, a), (_, b)| {
            (a.focused, a.recency)
                .cmp(&(b.focused, b.recency))
                .then_with(|| {
                    area(b)
                        .partial_cmp(&area(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .map(|(i, _)| i)
}

fn area(r: &SnapRect) -> f32 {
    f32::from(r.bounds.size.width) * f32::from(r.bounds.size.height)
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*`: the parent module's `use gpui_kit::*`
    // pulls gpui's own `test` attribute macro which shadows the built-in
    // #[test] (see the comment in selection.rs)
    use super::{SnapRect, hit_test};
    use gpui_kit::{Bounds, Pixels, Point, point, px, size};

    fn rect(x: f32, y: f32, w: f32, h: f32, focused: bool, recency: u64) -> SnapRect {
        SnapRect {
            bounds: Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            },
            app_id: String::new(),
            focused,
            recency,
        }
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    #[test]
    fn point_inside_and_outside() {
        let rects = [rect(100., 100., 200., 100., false, 0)];
        assert_eq!(hit_test(&rects, at(150., 150.)), Some(0));
        assert_eq!(hit_test(&rects, at(50., 150.)), None);
        // edges are inclusive
        assert_eq!(hit_test(&rects, at(100., 100.)), Some(0));
        assert_eq!(hit_test(&rects, at(300., 200.)), Some(0));
        assert_eq!(hit_test(&rects, at(300.1, 200.)), None);
    }

    #[test]
    fn focused_beats_recency_beats_area() {
        let rects = [
            rect(0., 0., 400., 400., false, 999), // old, big
            rect(0., 0., 400., 400., true, 1),    // focused wins
        ];
        assert_eq!(hit_test(&rects, at(200., 200.)), Some(1));

        let rects = [
            rect(0., 0., 400., 400., false, 10), // older
            rect(0., 0., 400., 400., false, 20), // more recent
        ];
        assert_eq!(hit_test(&rects, at(200., 200.)), Some(1));
    }

    #[test]
    fn nested_windows_pick_the_innermost() {
        // floating dialog (small) over a big terminal, no focus info
        let rects = [
            rect(0., 0., 1000., 800., false, 0),
            rect(400., 300., 200., 150., false, 0),
        ];
        assert_eq!(hit_test(&rects, at(500., 370.)), Some(1));
        // outside the dialog the terminal still matches
        assert_eq!(hit_test(&rects, at(50., 50.)), Some(0));
    }

    #[test]
    fn zero_rects_or_miss_returns_none() {
        assert_eq!(hit_test(&[], at(1., 1.)), None);
    }
}
