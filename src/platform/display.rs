//! # Display matching: pin each capture back to "the output it came from"
//!
//! **Why matching is needed**: without an explicit output, the compositor
//! picks a landing spot for a layer surface (multi-monitor = lottery; this
//! was the real culprit behind the "240Hz invisibility" bug). gpui's
//! `WindowOptions.display_id` can pin the window, but the id lives in gpui's
//! connection with no direct correspondence to the wl_output of the capture
//! connection — geometry is the only way to line them up.
//!
//! **Matching algorithm (one rule only — resist adding complexity)**:
//! gpui's display bounds origin = the capture's logical position ÷ its
//! scale (the wl_output integer scale); positions are unique in a
//! multi-monitor layout → match on position only, ignore sizes
//! (fractional scales report no true value, see
//! [`crate::platform::capture::Capture::scale`]).
//!
//! **Timing**: displays() is always empty during synchronous startup
//! (upstream zed#46378) and becomes available after the first event-loop
//! pass → [`await_display_ids`] polls inside an async task (1s cap).

use gpui_kit::*;

use crate::platform::capture::Capture;

/// Wait until displays() is usable and match each capture. Outputs that
/// time out (~1 s) get None — falling back to the old "compositor picks"
/// behavior, at least a window opens.
pub async fn await_display_ids(
    caps: Vec<Capture>,
    cx: &AsyncApp,
) -> Vec<(Capture, Option<DisplayId>)> {
    let mut targets: Vec<(Capture, Option<DisplayId>)> =
        caps.into_iter().map(|c| (c, None)).collect();
    for attempt in 0..20 {
        let all_matched = cx.update(|cx| match_all(cx, &mut targets, attempt == 19));
        if all_matched {
            break;
        }
        // 10 ms for the first four rounds (displays usually appear within
        // one event-loop pass — the old flat 50 ms cost a full round of
        // it), 50 ms afterwards; worst-case wait stays ~1 s
        let delay = std::time::Duration::from_millis(if attempt < 4 { 10 } else { 50 });
        cx.background_executor().timer(delay).await;
    }
    targets
}

/// One matching round; diagnostics are only printed on `final_round`
/// (avoid spamming one line every 50ms)
fn match_all(cx: &App, targets: &mut [(Capture, Option<DisplayId>)], final_round: bool) -> bool {
    let displays = cx.displays();
    if displays.is_empty() {
        return false;
    }
    let mut diag = Vec::new();
    for (cap, did) in targets.iter_mut() {
        if did.is_none() {
            *did = displays
                .iter()
                .find(|d| display_matches(&d.bounds(), cap))
                .map(|d| d.id());
            if did.is_none() {
                diag.push(format!(
                    "{}: expected gpui origin {:?}, actual displays={:?}",
                    cap.output_name,
                    expected_origin(cap),
                    displays
                        .iter()
                        .map(|d| {
                            let b = d.bounds();
                            (f32::from(b.origin.x) as i32, f32::from(b.origin.y) as i32)
                        })
                        .collect::<Vec<_>>()
                ));
            }
        }
    }
    if final_round {
        for d in &diag {
            eprintln!("[shotori] unmatched: {d}");
        }
    }
    targets.iter().all(|(_, d)| d.is_some())
}

/// The origin this capture should have in gpui coordinates
/// (logical position ÷ scale — the same division both gpui backends
/// perform on their display bounds)
fn expected_origin(cap: &Capture) -> (f32, f32) {
    (
        cap.logical_pos.0 as f32 / cap.scale,
        cap.logical_pos.1 as f32 / cap.scale,
    )
}

/// Match predicate: origins equal (±2px tolerance for float jitter)
/// → same output
fn display_matches(bounds: &Bounds<Pixels>, cap: &Capture) -> bool {
    let (ex, ey) = expected_origin(cap);
    (f32::from(bounds.origin.x) - ex).abs() < 2.0 && (f32::from(bounds.origin.y) - ey).abs() < 2.0
}

/// The gpui display a capture belongs to, by the position rule above.
/// `Window::display` is unreliable for layer-shell windows (the wayland
/// backend only learns their output from a later enter event — measured:
/// it returned None mid-session), so anything that must land on the
/// triggering output resolves its display through this instead.
pub(crate) fn display_for_capture(cap: &Capture, cx: &App) -> Option<DisplayId> {
    let displays = cx.displays();
    if displays.is_empty() {
        return None;
    }
    displays
        .iter()
        .find(|d| display_matches(&d.bounds(), cap))
        .map(|d| d.id())
}

// tests deliberately avoids `use super::*`: the parent module's
// `use gpui_kit::*` pulls gpui's test macro in and shadows the built-in
// #[test] (see the comment in selection.rs)
#[cfg(test)]
mod tests {
    use super::{display_matches, expected_origin};
    use crate::platform::capture::Capture;
    use gpui_kit::{Bounds, Pixels, point, px, size};

    fn cap(pos: (i32, i32), scale: f32) -> Capture {
        Capture::for_test(pos, scale)
    }

    fn bounds(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(1920.), px(1080.)),
        }
    }

    #[test]
    fn matches_real_triple_monitor_coordinates() {
        // The user's three monitors: HDMI (0,0)@1x; eDP (1920,0)@2x→gpui(960,0);
        // DP-2 (-720,-100)@2.0(integer-reported)→(-360,-50)
        let hdmi = cap((0, 0), 1.);
        let edp = cap((1920, 0), 2.);
        let dp2 = cap((-720, -100), 2.);

        assert!(display_matches(&bounds(0., 0.), &hdmi));
        assert!(display_matches(&bounds(960., 0.), &edp));
        assert!(display_matches(&bounds(-360., -50.), &dp2));

        // must not cross-match
        assert!(!display_matches(&bounds(960., 0.), &hdmi));
        assert!(!display_matches(&bounds(-360., -50.), &edp));
    }

    #[test]
    fn matches_within_2px_tolerance() {
        let hdmi = cap((0, 0), 1.);
        assert!(display_matches(&bounds(1.5, -1.5), &hdmi)); // within ±1.5px
        assert!(!display_matches(&bounds(2.5, 0.), &hdmi)); // beyond
    }

    #[test]
    fn negative_coordinate_division() {
        // -720/2 = -360; -100/2 = -50 (negative integer division direction
        // does not affect f32 division)
        let dp2 = cap((-720, -100), 2.);
        assert_eq!(expected_origin(&dp2), (-360., -50.));
    }
}
