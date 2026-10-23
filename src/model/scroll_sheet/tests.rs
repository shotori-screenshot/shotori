//! Scroll-sheet tests: synthetic aperiodic sheets exercise the
//! profiler, matcher, sticky bands and preview cadence end to end.
//! Kept as a sibling file — 1300+ test lines had buried the 900-line
//! implementation.

use super::preview::STALLED_FRAMES;
use super::profile::ALL_BANDS;
use super::*;

const W: u32 = 64;
const H: u32 = 96;

/// Deterministic aperiodic row color: a bit-mix of the row index.
/// Hash-y rows give every band signal texture with no periodicity,
/// which is exactly what the matcher needs and what the periodic
/// test must avoid.
fn source_row(row: u32) -> [u8; 4] {
    let mut x = row.wrapping_mul(2654435761).wrapping_add(0x9e3779b9);
    x ^= x >> 15;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    [
        (x & 0xff) as u8,
        ((x >> 8) & 0xff) as u8,
        ((x >> 16) & 0xff) as u8,
        0xff,
    ]
}

/// A text-like ink row: dark glyphs on a light ground, in-row
/// structure, aperiodic — the per-row shape of real content.
fn ink_row(y: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(W as usize * 4);
    for x in 0..W {
        let h = ink_hash(x, y);
        let l = if h % 100 < 42 {
            30 + (h % 60) as u8
        } else {
            238 + (h % 8) as u8
        };
        v.extend_from_slice(&[l, l, l, 255]);
    }
    v
}

/// The viewport at `top` over an infinite deterministic source page.
fn frame_at(top: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((W * H * 4) as usize);
    for row in top..top + H {
        v.extend_from_slice(&source_row(row).repeat(W as usize));
    }
    v
}

fn sheet() -> ScrollSheet {
    ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap()
}

/// The expected canvas after a walk that visited viewport tops
/// `first..=last` (rows `first..last + H` of the source page).
fn expected_canvas(first: u32, last: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(((last + H - first) * W * 4) as usize);
    for row in first..last + H {
        v.extend_from_slice(&source_row(row).repeat(W as usize));
    }
    v
}

/// A striped canvas: row `y` is a solid colour so a downscaled block
/// reveals exactly which sheet rows it came from.
fn striped(w: u32, h: u32, color: impl Fn(u32) -> [u8; 4]) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&color(y));
        }
    }
    rgba
}

#[test]
fn preview_slice_downscales_the_requested_window() {
    // 8×8 canvas: rows 0..4 red, rows 4..8 blue. target 2 → k = 4 →
    // 2 preview columns; the slice [4, 8) is all blue, [0, 8) mixes.
    let (w, h) = (8u32, 8u32);
    let rgba = striped(w, h, |y| {
        if y < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let (pw, ph, out) = preview_slice(&rgba, w, h, 4, 4, 2);
    assert_eq!((pw, ph), (2, 1));
    assert!(
        out.as_chunks::<4>()
            .0
            .iter()
            .all(|p| p == &[0, 0, 255, 255]),
        "slice [4, 8) must be the blue half, got {out:?}"
    );

    let (pw2, ph2, out2) = preview_slice(&rgba, w, h, 0, 8, 2);
    assert_eq!((pw2, ph2), (2, 2));
    let blocks = out2.as_chunks::<4>().0;
    assert!(blocks.first().is_some_and(|p| p[0] == 255 && p[2] == 0));
    assert!(blocks.last().is_some_and(|p| p[0] == 0 && p[2] == 255));

    // A window past the end is clamped, never a panic or a blank.
    let (_, ph3, out3) = preview_slice(&rgba, w, h, 7, 4, 2);
    assert_eq!(ph3, 1);
    assert_eq!(out3.len(), 2 * 4);
}

#[test]
fn preview_window_tracks_the_viewport_bottom_and_clamps() {
    // A short sheet: the whole thing, top-anchored, even though the
    // requested window is taller.
    assert_eq!(preview_window(300, 0, 300, 800), (0, 300));
    assert_eq!(preview_window(300, 100, 200, 800), (0, 300));

    // Growing sheet, viewport at the bottom edge: the window is the
    // tail, so new rows enter at its bottom.
    assert_eq!(preview_window(10_000, 9_200, 800, 1_600), (8_400, 1_600));
    // Reading mid-sheet: the camera follows the viewport bottom.
    assert_eq!(preview_window(10_000, 3_000, 800, 1_600), (2_200, 1_600));
    // Near the top the camera clamps at row 0 instead of going
    // negative.
    assert_eq!(preview_window(10_000, 100, 800, 1_600), (0, 1_600));
    // Requested window shorter than the viewport: grown to fit, so
    // the highlight is never clipped.
    assert_eq!(preview_window(10_000, 5_000, 900, 400), (5_000, 900));
}

#[test]
fn preview_window_rate_is_the_scroll_rate() {
    // The sync contract: for equal scroll steps the window top moves
    // by the same amount, so the strip advances exactly as fast as
    // the page (no easing, no easing-out at the edges) until the
    // sheet top clamps.
    let window = 1_600;
    let mut previous = None;
    for viewport_top in [2_000, 2_400, 2_800, 3_200] {
        let (top, _) = preview_window(20_000, viewport_top, 800, window);
        if let Some(previous) = previous {
            assert_eq!(top - previous, 400, "camera must move 1:1");
        }
        previous = Some(top);
    }
}

#[test]
fn phase_ladder_separates_a_tear_from_a_dead_region() {
    assert_eq!(phase_of(1, Refusal::NoOverlap), Phase::Following);
    assert_eq!(phase_of(2, Refusal::NoOverlap), Phase::NoOverlap);
    assert_eq!(
        phase_of(STALLED_FRAMES - 1, Refusal::NoOverlap),
        Phase::NoOverlap
    );
    assert_eq!(phase_of(STALLED_FRAMES, Refusal::NoOverlap), Phase::Stalled);
    assert_eq!(phase_of(u32::MAX, Refusal::Mismatch), Phase::Stalled);
}

/// The chip's word comes from the matcher's verdict, not from the
/// count alone: "no overlap" is advice about scroll speed, while
/// unstable pixels are a property of the area.
#[test]
fn the_chip_word_follows_the_refusal_reason() {
    assert_eq!(phase_of(3, Refusal::NoOverlap), Phase::NoOverlap);
    assert_eq!(phase_of(3, Refusal::Mismatch), Phase::Unstable);
    assert_eq!(phase_of(3, Refusal::Unclear), Phase::Unstable);
}

#[test]
fn state_machine_takes_each_normative_branch() {
    let s = ViewportState::default();
    let (s, b, g) = s.transition(10);
    assert_eq!((b, g), (Branch::Below, 10));
    assert_eq!((s.position, s.max_position), (10, 10));

    // Back up inside the captured extent: contained, zero growth.
    let (s2, b2, g2) = s.transition(-4);
    assert_eq!((b2, g2), (Branch::Within, 0));
    assert_eq!((s2.position, s2.max_position), (6, 10));

    // Above the canvas top: prepend grows without losing height.
    let (s3, b3, g3) = s2.transition(-12);
    assert_eq!((b3, g3), (Branch::Above, 6));
    assert_eq!((s3.position, s3.max_position), (0, 16));
}

#[test]
fn horizontal_sessions_transpose_in_and_out() {
    // One vertical reference walk…
    let mut vertical = ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap();
    vertical.push(&frame_at(30));
    let (vw, vh, vout) = vertical.output();

    // …and the same walk over transposed frames.
    let horiz = |top: u32| transpose_rgba(&frame_at(top), W, H);
    let mut horizontal =
        ScrollSheet::new_with_axis(H, W, &horiz(0), SheetLimits::default(), Axis::Horizontal)
            .unwrap();
    let out = horizontal.push(&horiz(30));
    assert!(matches!(out, Fold::GrewBottom { dy: 30, .. }), "{out:?}");
    let (hw, hh, hout) = horizontal.output();
    // The output is the vertical canvas laid on its side — exact
    // pixels, no resampling.
    assert_eq!((hw, hh), (vh, vw));
    assert_eq!(hout, transpose_rgba(&vout, vw, vh));
    // The preview keeps consuming the sheet-space (tall) canvas.
    assert_eq!(horizontal.dimensions(), (W, H + 30));
}

#[test]
fn axis_switches_only_before_the_first_accepted_frame() {
    // A horizontally scrolling source: every pixel samples a 1-D
    // page indexed by column, so shifting the frame left moves each
    // row left too. The sheet sees the transposed (row) motion.
    let hframe = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            for x in 0..W {
                let px = source_row(top.wrapping_add(x).wrapping_add(y.wrapping_mul(W)));
                v.extend_from_slice(&px);
            }
        }
        v
    };
    let options = SheetLimits {
        min_overlap: 16,
        ..SheetLimits::default()
    };
    let mut s = ScrollSheet::new(W, H, &hframe(0), options).unwrap();
    assert_eq!(s.axis(), Axis::Vertical);
    assert!(!s.axis_locked());
    assert!(s.set_axis(Axis::Horizontal));
    assert_eq!(s.axis(), Axis::Horizontal);
    // Sheet space is the transposed seed: (H, W) → (H, W)? The
    // seed was 64×96; transposed it is 96×64.
    assert_eq!(s.dimensions(), (H, W));
    assert_eq!(s.output_dimensions(), (W, H));
    let out = s.push(&hframe(20));
    assert!(matches!(out, Fold::GrewBottom { dy: 20, .. }), "{out:?}");
    assert_eq!(s.output_dimensions(), (W + 20, H));
    assert!(s.axis_locked());
    // A grown canvas cannot be swapped losslessly.
    assert!(!s.set_axis(Axis::Vertical));
    assert_eq!(s.axis(), Axis::Horizontal);
}

#[test]
fn first_frame_seeds_the_canvas() {
    let s = sheet();
    assert_eq!(s.dimensions(), (W, H));
    assert_eq!(s.canvas(), frame_at(0).as_slice());
    assert_eq!(s.captured_extent(), 0);
}

#[test]
fn tiny_viewports_are_rejected_upfront() {
    let frame = vec![0u8; W as usize * 60 * 4];
    assert_eq!(
        ScrollSheet::new(W, 60, &frame, SheetLimits::default()).err(),
        Some(Refusal::TooSmall)
    );
}

#[test]
fn scroll_down_appends_and_reconstructs_exactly() {
    let mut s = sheet();
    for top in [24u32, 24, 61, 95] {
        let out = s.push(&frame_at(top));
        // The dwell at 24 duplicates — skip asserting it here; the
        // duplicate path has its own test.
        if top != 24 {
            assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
        }
    }
    assert_eq!(s.dimensions(), (W, 95 + H));
    assert_eq!(s.canvas(), expected_canvas(0, 95).as_slice());
}

#[test]
fn duplicate_poll_frames_are_skipped_without_disturbing_state() {
    let mut s = sheet();
    assert_eq!(s.push(&frame_at(0)), Fold::Unchanged);
    assert_eq!(s.push(&frame_at(0)), Fold::Unchanged);
    // The anchor is still the ORIGINAL frame: scrolling after the
    // duplicates must match against it, not a replaced copy.
    s.push(&frame_at(30));
    assert_eq!(s.dimensions(), (W, 30 + H));
}

#[test]
fn scroll_up_prepends_recovering_content_above_the_start() {
    // Start mid-page, scroll down, then back up past the start.
    let mut s = ScrollSheet::new(W, H, &frame_at(100), SheetLimits::default()).unwrap();
    s.push(&frame_at(130));
    let out = s.push(&frame_at(90));
    assert!(matches!(out, Fold::GrewTop { growth: 10, .. }), "{out:?}");
    // Canvas covers source rows 90..=130+H.
    assert_eq!(s.canvas(), expected_canvas(90, 130).as_slice());
    assert_eq!(s.dimensions(), (W, 40 + H));
}

#[test]
fn up_down_wiggle_is_contained_without_growth() {
    let mut s = sheet();
    s.push(&frame_at(40));
    let height_after_append = s.height();
    let out = s.push(&frame_at(15));
    assert!(matches!(out, Fold::Shifted { dy: -25 }), "{out:?}");
    assert_eq!(s.height(), height_after_append);
    // Rewriting in place keeps the canvas consistent with the source.
    assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
}

#[test]
fn one_pixel_change_reports_no_motion_and_keeps_anchor_fresh() {
    let mut frame = frame_at(0);
    // Column 10 sits inside band 0 (w/8..): a real signal change in
    // the G channel, the caret-blink stand-in.
    frame[10 * 4 + 1] ^= 0xff;
    let mut s = sheet();
    assert_eq!(s.push(&frame), Fold::Still);
    assert_eq!(s.dimensions(), (W, H));
    // The changed frame became the anchor: a scroll away from HERE
    // (not from the pre-blink frame) must still match.
    let out = s.push(&frame_at(20));
    assert!(
        matches!(out, Fold::GrewBottom { growth: 20, .. }),
        "{out:?}"
    );
}

#[test]
fn periodic_texture_is_ambiguous_not_guessy() {
    // Repeated *similar* content: every 48 rows the page repeats the
    // same block, with adjacent copies differing by exactly one luma
    // step (the stand-in for list items that look alike). A pure
    // period with an exact-period offset would make the two frames'
    // signals identical at dy 0 — indistinguishable from no motion —
    // so the luma drift is what keeps d = 0 a *bad* match (cost 1.0)
    // while d = 48 is a perfect one (cost 0). Another offset (d = 0)
    // then fits nearly as well as the best, and the margin test must
    // refuse to choose.
    let similar_page = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H * 4) as usize);
        for row in top..top + H {
            let mut px = source_row(row % 48);
            let parity = ((row / 48) % 2) as u8;
            for c in &mut px[..3] {
                *c = c.saturating_add(parity);
            }
            v.extend_from_slice(&px.repeat(W as usize));
        }
        v
    };
    let mut s = ScrollSheet::new(W, H, &similar_page(0), SheetLimits::default()).unwrap();
    assert_eq!(s.push(&similar_page(48)), Fold::Refused(Refusal::Unclear));
    // The canvas is untouched by the rejection.
    assert_eq!(s.dimensions(), (W, H));
}

#[test]
fn flat_content_reports_no_motion() {
    let flat = || vec![0xff; (W * H * 4) as usize];
    let mut changed = flat();
    changed[10 * 4 + 1] = 0x00;
    let mut s = ScrollSheet::new(W, H, &flat(), SheetLimits::default()).unwrap();
    // A flat frame "matches" every offset at ~zero cost — dy 0 wins
    // by scan order, and zero must not demand an ambiguity margin.
    assert_eq!(s.push(&changed), Fold::Still);
}

#[test]
fn torn_frames_reject_but_the_settled_frame_recovers_the_full_step() {
    // A capture mid-smooth-scroll: the top half still shows the old
    // position, the bottom half the new one. Nothing aligns — and
    // the frame becomes the riding anchor.
    let old = frame_at(0);
    let settled = frame_at(20);
    let torn: Vec<u8> = [
        old[..old.len() / 2].to_vec(),
        settled[settled.len() / 2..].to_vec(),
    ]
    .concat();
    let mut s = sheet();
    assert_eq!(s.push(&torn), Fold::Refused(Refusal::Mismatch));
    // The settled frame matches the last ACCEPTED frame directly
    // (dual anchor), so the FULL 20-row step lands — no band lost.
    // (The single-anchor design swallowed the 20 rows between the
    // accept and the riding anchor — the long image came out
    // shorter than the real one.)
    let out = s.push(&settled);
    assert!(
        matches!(out, Fold::GrewBottom { dy: 20, growth: 20 }),
        "{out:?}"
    );
    let out = s.push(&frame_at(40));
    assert!(
        matches!(out, Fold::GrewBottom { dy: 20, growth: 20 }),
        "{out:?}"
    );
    assert_eq!(s.dimensions(), (W, 40 + H));
    assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
}

#[test]
fn sticky_top_chrome_still_matches_through_the_content() {
    // A fixed site header occupies the top 48 rows of every frame;
    // the content scrolls beneath it. Under translation the band is
    // misaligned somewhere in the comparison window for EVERY
    // candidate — the untrimmed mean rejected every frame (the
    // "scrolled up to the navbar and sync died" report). The
    // trimmed row mean must absorb it in all three directions.
    // H2 clears the edge-trim gate; the header is ~6% of the rows,
    // well under the 1/7 trim.
    const H2: u32 = 800;
    let hdr = 48u32;
    let header = source_row(999_999).repeat(W as usize);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for _ in 0..hdr {
            v.extend_from_slice(&header);
        }
        for row in top..top + H2 - hdr {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    };

    // Down, then back up (Shifted) — the exact gesture from the
    // bug report.
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    let out = s.push(&frame(60));
    assert!(
        matches!(out, Fold::GrewBottom { dy: 60, growth: 60 }),
        "{out:?}"
    );
    let out = s.push(&frame(0));
    assert!(matches!(out, Fold::Shifted { dy: -60 }), "{out:?}");

    // Up past the start of the canvas (Prepend) through the band.
    let mut s = ScrollSheet::new(W, H2, &frame(100), SheetLimits::default()).unwrap();
    s.push(&frame(160));
    let out = s.push(&frame(40));
    assert!(
        matches!(
            out,
            Fold::GrewTop {
                dy: 120,
                growth: 60
            }
        ),
        "{out:?}"
    );
    // The canvas covers source rows 40..160+H2-hdr; the viewport
    // highlight must be back at the very top.
    assert_eq!(s.viewport_span().0, 0);
}

#[test]
fn preview_window_and_viewport_span_agree_on_the_last_accepted_frame() {
    // The user-facing contract behind the preview highlight: the
    // preview rows under the viewport span are EXACTLY the
    // downscaled rows of the last accepted frame — what the user's
    // frame shows. Teardown of a tear in between must not shift it.
    let old = frame_at(0);
    let settled = frame_at(20);
    let torn: Vec<u8> = [
        old[..old.len() / 2].to_vec(),
        settled[settled.len() / 2..].to_vec(),
    ]
    .concat();
    let mut s = sheet();
    s.push(&torn);
    let out = s.push(&settled);
    assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");

    let (w, h) = s.dimensions();
    let (vt, vh) = s.viewport_span();
    let (top, rows) = preview_window(h, vt, vh, 10_000);
    let (pw, ph, rgba) = preview_slice(s.canvas(), w, h, top, rows, 32);
    // The engine's mapping, verbatim (viewport_in_preview).
    let k = w.div_ceil(pw.max(1));
    let v_h = (vh / k).clamp(1, ph.max(1));
    let v_top = (vt.saturating_sub(top) / k).min(ph.saturating_sub(v_h));
    // The frame's own preview, same filter and factor: in frame
    // space the accepted frame IS rows [0, vh).
    let (fw, fh, frame_prev) = preview_slice(&settled, W, H, 0, vh, 32);
    assert_eq!((fw, fh), (pw, v_h));
    let start = (v_top * pw * 4) as usize;
    let end = ((v_top + v_h) * pw * 4) as usize;
    assert_eq!(
        &rgba[start..end],
        &frame_prev[..v_h as usize * pw as usize * 4],
        "the highlighted preview rows must show the frame's rows"
    );
}

#[test]
fn coarse_sweep_finds_steps_beyond_the_prediction_window() {
    // A 300-row jump at 0 prediction is outside the ±128
    // neighbourhood, so only the step-3 coarse sweep can find it.
    const H2: u32 = 800;
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in top..top + H2 {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    let out = s.push(&frame(300));
    assert!(
        matches!(
            out,
            Fold::GrewBottom {
                dy: 300,
                growth: 300
            }
        ),
        "{out:?}"
    );
    let mut expected = Vec::new();
    for row in 0..300 + H2 {
        expected.extend_from_slice(&source_row(row).repeat(W as usize));
    }
    assert_eq!(s.canvas(), expected.as_slice());
}

#[test]
fn oversized_jump_is_high_residual() {
    let mut s = sheet();
    // 0.95 × H is beyond the admissible search range, so the best
    // in-range candidate is garbage — surfaced as Mismatch
    // (indistinguishable from a scene change by cost alone, which is
    // fine: the scroll loop reacts the same way).
    let out = s.push(&frame_at((H as f32 * 0.95) as u32));
    assert_eq!(out, Fold::Refused(Refusal::Mismatch));
}

#[test]
fn height_cap_stops_the_sheet() {
    let options = SheetLimits {
        max_height: H + 60,
        ..SheetLimits::default()
    };
    let mut s = ScrollSheet::new(W, H, &frame_at(0), options).unwrap();
    s.push(&frame_at(30));
    s.push(&frame_at(60)); // reaches the cap exactly
    assert_eq!(s.dimensions(), (W, H + 60));
    assert_eq!(s.push(&frame_at(90)), Fold::Refused(Refusal::TooTall));
    assert_eq!(s.dimensions(), (W, H + 60));
}

#[test]
fn random_walk_reconstructs_the_visited_page() {
    // The invariant test: a randomized (but deterministic) scroll
    // session — monotone-ish drift with dwells, small up-wiggles and
    // re-visits — must reconstruct exactly the source rows between
    // the lowest and highest viewport tops ever seen.
    let mut seed = 0x1234_5678_u64;
    let mut rand = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as u32
    };

    let mut top = 500u32;
    let mut s = ScrollSheet::new(W, H, &frame_at(top), SheetLimits::default()).unwrap();
    let (mut lo, mut hi) = (top, top);
    for _ in 0..200 {
        // Steps stay inside the matcher's admissible window
        // (≤ 40 down / 30 up, overlap ≥ 56 rows).
        let delta = match rand() % 10 {
            0..=1 => 0, // dwell (poll duplicate)
            2..=3 => -i64::from(rand() % 20 + 1),
            _ => i64::from(rand() % 30 + 5),
        }
        .clamp(-30, 40);
        let next = (i64::from(top) + delta).max(0) as u32;
        let out = s.push(&frame_at(next));
        assert!(
            !matches!(out, Fold::Refused(_)),
            "walk rejected at {top} → {next}: {out:?}"
        );
        top = next;
        lo = lo.min(top);
        hi = hi.max(top);
    }
    assert_eq!(s.dimensions(), (W, hi - lo + H));
    assert_eq!(s.canvas(), expected_canvas(lo, hi).as_slice());
    let (pushed, accepted) = s.stats();
    assert_eq!(pushed, 201);
    assert!(accepted >= 2);
}

// === Sticky-band detection ==========================================

/// A structured (non-flat) fixed chrome row: dark left half, bright
/// right half — constant across frames (what makes it sticky) but
/// with in-row luma spread (what lets the band-level flat-content
/// guard distinguish it from flat content).
fn chrome_band(rows: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((W * rows * 4) as usize);
    for _ in 0..rows {
        for x in 0..W {
            let px: [u8; 4] = if x < W / 2 {
                [30, 30, 40, 255]
            } else {
                [220, 220, 230, 255]
            };
            v.extend_from_slice(&px);
        }
    }
    v
}

/// A page with a structured header pinned from the FIRST frame; the
/// content scrolls beneath it (the classic fixed site navbar).
#[test]
fn structured_header_from_the_first_frame_pins_once() {
    const H2: u32 = 800;
    const HDR: u32 = 48;
    let band = chrome_band(HDR);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = band.clone();
        for row in top..top + H2 - HDR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    };
    let expected = |last: u32| -> Vec<u8> {
        let mut v = band.clone();
        for row in 0..last + H2 - HDR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    };

    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    // The verified-persistence bar (COMMIT_FRAMES) needs six moving
    // frames before the band commits.
    for top in [30u32, 60, 90, 120, 150, 180] {
        s.push(&frame(top));
    }
    // Band written ONCE at the top (the seed frame already had it
    // there — no splice), content stream exact beneath it.
    assert_eq!(s.dimensions(), (W, HDR + (180 + H2 - HDR)));
    assert_eq!(s.canvas(), expected(180).as_slice());
    // The highlight reports the CONTENT segment, below the band.
    assert_eq!(s.viewport_span(), (HDR + 180, H2 - HDR));
}

/// Flat page margins match both stationary and translated coordinates;
/// they must not enlarge the fixed-header exclusion.
#[test]
fn flat_background_below_the_chrome_never_pins() {
    const H2: u32 = 800;
    const HDR: u32 = 48;
    const FLAT: u32 = 200;
    let band = chrome_band(HDR);
    let flat_row: Vec<u8> = [24, 24, 28, 255].repeat(W as usize);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = band.clone();
        for _ in 0..FLAT {
            v.extend_from_slice(&flat_row);
        }
        for row in top..top + H2 - HDR - FLAT {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    };

    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [30u32, 60, 90, 120, 150, 180] {
        let out = s.push(&frame(top));
        assert!(matches!(out, Fold::GrewBottom { dy: 30, .. }), "{out:?}");
    }
    // The band is exactly the chrome (step = 1 at this size, no
    // grid coarseness); the flat strip below never joined the run.
    assert_eq!(s.sticky.top, HDR);
}

/// Short repaint stalls can look stationary beneath a fixed header.
/// The persistence requirement prevents them from enlarging its exclusion.
#[test]
fn stalled_damage_blocks_under_the_chrome_never_grow_the_band() {
    const H2: u32 = 800;
    const HDR: u32 = 48;
    const BLOCK: u32 = 16; // stale rows hugging the band's bottom edge
    let band = chrome_band(HDR);
    // Per-pixel ink rows: like real text, each row carries in-row
    // structure, so a stale block of them passes every per-frame
    // signal chrome can — stillness, displacement misfit, structure.
    let frame = |top: u32| -> Vec<u8> {
        let mut v = band.clone();
        for row in top..top + H2 - HDR {
            v.extend_from_slice(&ink_row(row));
        }
        v
    };
    // `lag` rows of the block still show the older position.
    let damaged = |top: u32, lag: u32| -> Vec<u8> {
        let mut v = band.clone();
        for row in top - lag..top - lag + BLOCK {
            v.extend_from_slice(&ink_row(row));
        }
        for row in top + BLOCK..top + H2 - HDR {
            v.extend_from_slice(&ink_row(row));
        }
        v
    };

    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80, 120, 160, 200, 240] {
        let out = s.push(&frame(top));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    }
    assert_eq!(s.sticky.top, HDR);
    // Three accepted frames with the same stale block hugging the
    // band: offset-0 still, misfits the translation.
    for (top, lag) in [(280u32, 40u32), (320u32, 80u32), (360u32, 120u32)] {
        let out = s.push(&damaged(top, lag));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    }
    assert_eq!(s.sticky.top, HDR, "a repaint stall must not grow the band");
    // Capture recovers once the block repaints.
    let out = s.push(&frame(400));
    assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    assert_eq!(s.sticky.top, HDR);
}

/// A compositor tear the trim absorbs: the top strip of the frame
/// still shows the previous position (painted first, scrolled
/// later) while the rest aligns at the true step. The trimmed row
/// mean accepts the frame — by design, one sticky navbar must not
/// be able to refuse the session — and the wholesale write then
/// stamped the stale strip over canvas rows that already held the
/// right page content: a sheared sliver nothing ever revisits (the
/// third GitHub capture). Matching may absorb misaligned rows; the
/// write may not. A skipped row keeps the canvas it already had,
/// and the row↔page mapping makes that the correct pixels.
#[test]
fn absorbed_tear_frames_do_not_stamp_their_stale_strip() {
    const H2: u32 = 800;
    const TEAR: u32 = 16; // well under the 1/7 trim
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in top..top + H2 {
            v.extend_from_slice(&ink_row(row));
        }
        v
    };
    // A TEAR-row block mid-frame still shows `top − lag`: the
    // compositor repainted around it but not it. Rows above the
    // block moved, so the offset-0 still prefix is zero and the
    // wholesale write reaches straight over it — this is the case
    // the prefix skip cannot see.
    let torn_mid = |top: u32, lag: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        let at = 300usize;
        for row in 0..at {
            v.extend_from_slice(&ink_row(top + row as u32));
        }
        for row in top - lag..top - lag + TEAR {
            v.extend_from_slice(&ink_row(row));
        }
        for row in top + at as u32 + TEAR..top + H2 {
            v.extend_from_slice(&ink_row(row));
        }
        v
    };

    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80] {
        let out = s.push(&frame(top));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    }
    let out = s.push(&torn_mid(120, 40));
    assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    let out = s.push(&frame(160));
    assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
    // The canvas must be an exact page prefix: canvas row c is page
    // row c.
    let (_, h) = s.dimensions();
    assert_eq!(h, 160 + H2);
    let canvas = s.canvas();
    for c in 0..h as usize {
        let want = ink_row(c as u32);
        let got = &canvas[c * W as usize * 4..(c + 1) * W as usize * 4];
        assert_eq!(got, want.as_slice(), "canvas row {c} diverged");
    }
}

/// A mid-scroll overlay leaves its original document occurrence intact
/// and never inserts another copy above the captured content.
#[test]
fn mid_scroll_pin_keeps_document_coordinates_and_a_single_original_heading() {
    const H2: u32 = 800;
    const NAV: u32 = 12;
    const NAV_AT: u32 = 40; // the navbar's content-plane rows
    let band = chrome_band(NAV);
    // The source page carries the navbar content at its placeholder.
    let row_of = |row: u32| -> Vec<u8> {
        if (NAV_AT..NAV_AT + NAV).contains(&row) {
            let mut v = Vec::with_capacity(W as usize * 4);
            for x in 0..W {
                let px: [u8; 4] = if x < W / 2 {
                    [30, 30, 40, 255]
                } else {
                    [220, 220, 230, 255]
                };
                v.extend_from_slice(&px);
            }
            v
        } else {
            source_row(row).repeat(W as usize)
        }
    };
    // Pre-pin: the whole viewport is content. Post-pin: the band
    // sits at [0, NAV) and viewport row r >= NAV shows source row
    // c + r (the placeholder scrolls on under the frozen band).
    let frame = |c: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        if c >= NAV_AT {
            v.extend_from_slice(&band);
            for row in c + NAV..c + H2 {
                v.extend_from_slice(&row_of(row));
            }
        } else {
            for row in c..c + H2 {
                v.extend_from_slice(&row_of(row));
            }
        }
        v
    };
    let expected = |last: u32| -> Vec<u8> { (0..last + H2).flat_map(row_of).collect() };

    // c = 40 is the first pinned frame; the still run reaches 2 two
    // frames later, so the commit happens while scrolling through
    // pinned territory.
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for c in [20u32, 40, 60, 90, 120, 150, 180, 210] {
        let out = s.push(&frame(c));
        assert!(matches!(out, Fold::GrewBottom { .. }), "frame {c}: {out:?}");
    }
    assert_eq!(s.dimensions(), (W, 210 + H2));
    assert_eq!(s.canvas(), expected(210).as_slice());
}

/// A tall fixed footer (taller than the static bottom trim) is
/// detected and committed; matching continues across it.
#[test]
fn fixed_footer_commits_and_folding_continues() {
    const H2: u32 = 800;
    const FTR: u32 = 40; // > the static trim (min(800/48, 32))
    let band = chrome_band(FTR);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in top..top + H2 - FTR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v.extend_from_slice(&band);
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80, 120, 160, 200, 240] {
        let out = s.push(&frame(top));
        assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
    } // sixth verified frame → bot mask commits
    // No top band ever: the header scrolls.
    assert_eq!(s.viewport_span().0, 240);
    assert_eq!(s.dimensions(), (W, H2 + 240));
}

// === Fixed-sidebar motion mask =====================================

/// A frame with a fixed left pane (PDF thumbnails, docs outline) that
/// keeps its own content while the page scrolls beneath the right
/// side. The pane's rows are static across frames but do NOT align
/// under the page's row translation — exactly the case the old
/// interior-only matcher rejected.
fn sidebar_frame(top: u32) -> Vec<u8> {
    const W2: u32 = 320;
    const H2: u32 = 400;
    const PANE: u32 = 160;
    let mut v = Vec::with_capacity((W2 * H2 * 4) as usize);
    for row in 0..H2 {
        let pane = source_row(50_000 + row);
        let content = source_row(top + row);
        for x in 0..W2 {
            let px = if x < PANE { pane } else { content };
            v.extend_from_slice(&px);
        }
    }
    v
}

#[test]
fn fixed_sidebar_bands_are_masked_and_folding_continues() {
    const W2: u32 = 320;
    const H2: u32 = 400;
    let mut s = ScrollSheet::new(W2, H2, &sidebar_frame(0), SheetLimits::default()).unwrap();
    let out = s.push(&sidebar_frame(40));
    assert!(
        matches!(out, Fold::GrewBottom { dy: 40, growth: 40 }),
        "the static pane must not poison the offset score: {out:?}"
    );
    // The pane occupies the four left signature bands of the
    // interior; the scrolling content the right four.
    assert_eq!(s.motion.weights[..4], [0.0, 0.0, 0.0, 0.0]);
    assert!(s.motion.weights[4..].iter().all(|&w| w == 1.0));
    assert_eq!(s.dimensions(), (W2, H2 + 40));
}

#[test]
fn all_moving_bands_keep_the_full_frame_in_the_score() {
    // Without a stationary pane the mask must not drop anything:
    // page content moves across the whole width.
    let mut s = ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap();
    s.push(&frame_at(20));
    assert_eq!(s.motion.weights, ALL_BANDS);
}

/// The committed footer is content-plane-excluded: the output is
/// the page's content rows followed by exactly one footer band.
#[test]
fn fixed_footer_is_written_once_at_the_canvas_bottom() {
    const H2: u32 = 800;
    const FTR: u32 = 40;
    let band = chrome_band(FTR);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in top..top + H2 - FTR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v.extend_from_slice(&band);
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80, 120, 160, 200, 240] {
        s.push(&frame(top));
    }
    let (_, h) = s.dimensions();
    let canvas = s.canvas();
    let row = W as usize * 4;
    let mut ideal = Vec::new();
    for r in 0..240 + H2 - FTR {
        ideal.extend_from_slice(&source_row(r).repeat(W as usize));
    }
    ideal.extend_from_slice(&band);
    assert_eq!(h, 240 + H2);
    let mut first = None;
    for y in 0..h as usize {
        let got = &canvas[y * row..(y + 1) * row];
        let want = &ideal[y * row..(y + 1) * row];
        if got != want {
            first = Some(y);
            break;
        }
    }
    assert!(first.is_none(), "first footer mismatch at row {first:?}");
}

/// Scrolling back up after the footer was committed must not stamp
/// a second footer at that viewport's bottom (the old limitation).
#[test]
fn fixed_footer_survives_an_up_scroll_without_duplicating() {
    const H2: u32 = 800;
    const FTR: u32 = 40;
    let band = chrome_band(FTR);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in top..top + H2 - FTR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v.extend_from_slice(&band);
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80, 120, 160, 200, 240] {
        s.push(&frame(top));
    }
    s.push(&frame(160)); // scroll back up mid-list
    let (_, h) = s.dimensions();
    let canvas = s.canvas();
    let row = W as usize * 4;
    let mut ideal = Vec::new();
    for r in 0..240 + H2 - FTR {
        ideal.extend_from_slice(&source_row(r).repeat(W as usize));
    }
    ideal.extend_from_slice(&band);
    let mut first = None;
    for y in 0..h.min((ideal.len() / row) as u32) as usize {
        let got = &canvas[y * row..(y + 1) * row];
        let want = &ideal[y * row..(y + 1) * row];
        if got != want {
            first = Some(y);
            break;
        }
    }
    assert_eq!(h, 240 + H2);
    assert!(
        first.is_none(),
        "first up-scroll footer mismatch at row {first:?}"
    );
}

/// Header and footer committed together: the output is one header,
/// the content stream, one footer — including after an up-scroll.
#[test]
fn header_and_footer_both_pin_once() {
    const H2: u32 = 800;
    const HDR: u32 = 40;
    const FTR: u32 = 40;
    let band = chrome_band(40);
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        v.extend_from_slice(&band);
        for row in top..top + H2 - HDR - FTR {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v.extend_from_slice(&band);
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    for top in [40u32, 80, 120, 160, 200, 240] {
        s.push(&frame(top));
    }
    s.push(&frame(120)); // scroll back up mid-list
    let mut ideal = Vec::new();
    ideal.extend_from_slice(&band);
    for row in 0..240 + H2 - HDR - FTR {
        ideal.extend_from_slice(&source_row(row).repeat(W as usize));
    }
    ideal.extend_from_slice(&band);
    assert_eq!(s.dimensions(), (W, H2 + 240));
    assert_eq!(s.canvas(), ideal.as_slice());
}

// === Realistic manual-scroll probe =================================

/// An independently rasterized floating button: opaque face, dark glyph,
/// antialiased rim and translucent shadow over generated page content.
fn floating_button(frame: &mut [u8], center: (i32, i32)) {
    let height = frame.len() / (TW as usize * 4);
    for y in 0..height as i32 {
        for x in 0..TW as i32 {
            let dx = x - center.0;
            let dy = y - center.1;
            let radius = ((dx * dx + dy * dy) as f32).sqrt();
            let (value, alpha) =
                if dx.abs() <= 1 && dy.abs() < 7 || (dy - 4).abs() < 2 && dx.abs() < 6 {
                    (35., 1.)
                } else if radius < 12. {
                    (250., 1.)
                } else if radius < 13. {
                    (180., 13. - radius)
                } else if radius < 17. {
                    (80., (17. - radius) * 0.06)
                } else {
                    continue;
                };
            let i = (y as usize * TW as usize + x as usize) * 4;
            for channel in &mut frame[i..i + 3] {
                *channel = (f32::from(*channel) * (1. - alpha) + value * alpha).round() as u8;
            }
        }
    }
}

fn assert_page_pixels(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    let differing = actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| (i % TW as usize, i / TW as usize))
        .collect::<Vec<_>>();
    assert!(
        differing.is_empty(),
        "{} incorrect pixels; first {:?}",
        differing.len(),
        differing.first()
    );
}

#[test]
fn floating_button_recovers_seed_and_preserves_clean_overlap_in_both_directions() {
    let page = text_page();
    let viewport = 480;
    let frame = |top| {
        let mut frame = sample(&page, top as f32, viewport);
        floating_button(&mut frame, (TW as i32 / 2, 240));
        frame
    };
    let mut sheet = ScrollSheet::new(TW, viewport, &frame(200), SheetLimits::default()).unwrap();
    for top in (208..=360).step_by(8) {
        let result = sheet.push(&frame(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 8, .. }),
            "{result:?}"
        );
    }
    assert_page_pixels(sheet.canvas(), &sample(&page, 200., viewport + 160));
    let revision = sheet.top_revision();
    for top in (120..360).step_by(8).rev() {
        let result = sheet.push(&frame(top));
        assert!(
            matches!(result, Fold::GrewTop { .. } | Fold::Shifted { .. }),
            "{result:?}"
        );
    }
    assert!(sheet.top_revision() > revision);
    assert_page_pixels(sheet.canvas(), &sample(&page, 120., viewport + 240));
}

#[test]
fn floating_buttons_survive_small_steps_and_different_viewport_positions() {
    let page = text_page();
    let viewport = 640;
    for step in [1, 2, 4, 16, 40] {
        let frame = |top| {
            let mut rgba = sample(&page, top as f32, viewport);
            floating_button(&mut rgba, (85, 193));
            floating_button(&mut rgba, (182, 507));
            rgba
        };
        let mut sheet = ScrollSheet::new(TW, viewport, &frame(0), SheetLimits::default()).unwrap();
        for top in (step..=160).step_by(step as usize) {
            let result = sheet.push(&frame(top));
            assert!(
                matches!(result, Fold::GrewBottom { .. }),
                "step {step}: {result:?}"
            );
            if top >= 80 {
                assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + top));
            }
        }
        assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + 160));
    }
}

#[test]
fn floating_button_and_pinned_header_survive_prepending() {
    let page = text_page();
    let viewport = 640;
    let header = sample(&page, 2_000., 32);
    let frame = |top| {
        let mut rgba = sample(&page, top as f32, viewport);
        rgba[..header.len()].copy_from_slice(&header);
        floating_button(&mut rgba, (120, 360));
        rgba
    };
    let mut sheet = ScrollSheet::new(TW, viewport, &frame(200), SheetLimits::default()).unwrap();
    for top in (224..=440).step_by(24) {
        assert!(matches!(sheet.push(&frame(top)), Fold::GrewBottom { .. }));
    }
    assert_eq!(sheet.sticky.top, 32);
    for top in (128..440).step_by(24).rev() {
        assert!(matches!(
            sheet.push(&frame(top)),
            Fold::GrewTop { .. } | Fold::Shifted { .. }
        ));
    }
    let mut expected = header;
    expected.extend_from_slice(&sample(&page, 160., 440 + viewport - 160));
    assert_page_pixels(sheet.canvas(), &expected);
}

#[test]
fn floating_button_never_fabricates_pixels_without_an_unobstructed_observation() {
    let page = text_page();
    let viewport = 640;
    let frame = |top| {
        let mut rgba = sample(&page, top as f32, viewport);
        floating_button(&mut rgba, (120, 550));
        rgba
    };
    let mut sheet = ScrollSheet::new(TW, viewport, &frame(0), SheetLimits::default()).unwrap();
    for top in [96, 192] {
        let capture = frame(top);
        let result = sheet.push(&capture);
        assert!(matches!(result, Fold::GrewBottom { .. }), "{result:?}");
        let src = (550 * TW as usize + 120) * 4;
        let dst = ((top as usize + 550) * TW as usize + 120) * 4;
        assert_eq!(&sheet.canvas()[dst..dst + 4], &capture[src..src + 4]);
    }
    // Earlier occlusions are now exposed; only the latest, never-visible
    // patch may retain its captured foreground.
    let known_rows = 700;
    assert_page_pixels(
        &sheet.canvas()[..known_rows * TW as usize * 4],
        &sample(&page, 0., known_rows as u32),
    );
    assert!(
        sheet
            .canvas()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
    );
}

#[test]
fn floating_button_appearance_disappearance_and_refused_frames_keep_the_page() {
    let page = text_page();
    let viewport = 480;
    let frame = |top, button| {
        let mut frame = sample(&page, top as f32, viewport);
        if button {
            floating_button(&mut frame, (TW as i32 / 2, 240));
        }
        frame
    };
    let mut sheet =
        ScrollSheet::new(TW, viewport, &frame(0, false), SheetLimits::default()).unwrap();
    for top in (24..=240).step_by(24) {
        let result = sheet.push(&frame(top, true));
        assert!(matches!(result, Fold::GrewBottom { .. }), "{result:?}");
    }
    let before = sheet.canvas().to_vec();
    let bad = frame(3_000, true);
    assert!(matches!(sheet.push(&bad), Fold::Refused(_)));
    assert_eq!(sheet.canvas(), before);
    for top in (264..=384).step_by(24) {
        assert!(matches!(
            sheet.push(&frame(top, true)),
            Fold::GrewBottom { .. }
        ));
    }
    for top in (240..384).step_by(24).rev() {
        assert!(matches!(
            sheet.push(&frame(top, false)),
            Fold::Shifted { .. }
        ));
    }
    assert!(sheet.floating.is_empty());
    assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + 384));
}

#[test]
fn page_local_button_is_preserved_and_horizontal_floating_button_is_removed() {
    let mut page_frame = sample(&text_page(), 0., 1_200);
    floating_button(&mut page_frame, (TW as i32 / 2, 280));
    let frame =
        |top: usize| page_frame[top * TW as usize * 4..(top + 480) * TW as usize * 4].to_vec();
    let mut sheet = ScrollSheet::new(TW, 480, &frame(0), SheetLimits::default()).unwrap();
    for top in (24..=240).step_by(24) {
        assert!(matches!(sheet.push(&frame(top)), Fold::GrewBottom { .. }));
        assert!(sheet.floating.is_empty());
    }
    assert_page_pixels(sheet.canvas(), &page_frame[..720 * TW as usize * 4]);

    let page = text_page();
    let horizontal = |top| {
        let mut rgba = sample(&page, top as f32, 480);
        floating_button(&mut rgba, (120, 240));
        transpose_rgba(&rgba, TW, 480)
    };
    let mut sheet = ScrollSheet::new_with_axis(
        480,
        TW,
        &horizontal(0),
        SheetLimits::default(),
        Axis::Horizontal,
    )
    .unwrap();
    for top in (24..=240).step_by(24) {
        assert!(matches!(
            sheet.push(&horizontal(top)),
            Fold::GrewBottom { .. }
        ));
    }
    assert_page_pixels(sheet.canvas(), &sample(&page, 0., 720));
}

const TW: u32 = 240;
const TH: u32 = 6_000;

fn ink_hash(x: u32, y: u32) -> u32 {
    x.wrapping_mul(0x9E37_79B1)
        .wrapping_add(y.wrapping_mul(0x85EB_CA77))
        .wrapping_mul(2654435761)
        >> 11
}

/// A text-like page: lines of per-pixel ink on a light ground. Unlike
/// the `source_row` fixtures, a row is NOT one constant colour — that
/// is what makes this page a fair stand-in for a real document.
fn text_page() -> Vec<u8> {
    let mut page = vec![246u8; (TW * TH) as usize];
    let mut y = 0u32;
    let mut line = 0u32;
    while y < TH {
        let line_h = 13 + line * 7 % 12; // 13..=24 rows
        let ink_rows = (y + line_h * 3 / 4).min(TH);
        for gy in y..ink_rows {
            for x in 0..TW {
                let h = ink_hash(x, gy);
                let px = if h % 100 < 42 {
                    30 + (h % 60) as u8
                } else {
                    238 + (h % 8) as u8
                };
                page[(gy * TW + x) as usize] = px;
            }
        }
        y += line_h;
        line += 1;
    }
    page
}

/// The viewport at a FRACTIONAL scroll offset: a real compositor
/// resamples a sub-pixel scroll, so the frame is a bilinear blend of
/// two page rows.
fn sample(page: &[u8], top: f32, vh: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((TW * vh * 4) as usize);
    for j in 0..vh {
        let y = top + j as f32;
        let y0 = y.floor().max(0.0) as u32;
        let f = y - y0 as f32;
        let a = y0.min(TH - 1);
        let b = (y0 + 1).min(TH - 1);
        for x in 0..TW {
            let va = f32::from(page[(a * TW + x) as usize]);
            let vb = f32::from(page[(b * TW + x) as usize]);
            let v = (va + (vb - va) * f).round() as u8;
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    out
}

/// A sparse-text page: a paragraph every 44 rows, the rest blank. This
/// is what most documentation/markdown pages look like inside a tall
/// selection, and it is the hard case for a translation matcher: most
/// rows carry no signal, so many offsets cost ~nothing.
fn sparse_page() -> Vec<u8> {
    let mut page = vec![250u8; (TW * TH) as usize];
    let mut y = 20u32;
    while y + 15 < TH {
        for gy in y..y + 15 {
            for x in 24..TW - 24 {
                let h = ink_hash(x, gy);
                if h % 100 < 40 {
                    page[(gy * TW + x) as usize] = 40 + (h % 50) as u8;
                }
            }
        }
        y += 44;
    }
    page
}

/// Drive a manual session over `page` — one frame per poll — and check
/// the invariant that makes a long capture worth anything:
///
/// **the sheet is always an exact PREFIX of the page.**
///
/// Not "looks about right": byte-for-byte. Rows that were folded in
/// early can never be rewritten by a later frame that was placed at a
/// guessed position (that is what used to shift a whole viewport of
/// correct content), and the sheet can never grow by more than the
/// page was actually scrolled. Returns the number of refused frames so
/// a caller can assert which path it exercised.
fn scroll_and_check_prefix(page: &[u8], steps: &[f32], rounds: usize) -> (ScrollSheet, usize) {
    const VH: u32 = 700;
    let mut s = ScrollSheet::new(TW, VH, &sample(page, 0., VH), SheetLimits::default())
        .expect("sheet from a full-width frame");
    let mut top = 0f32;
    let mut refused = 0;
    for _ in 0..rounds {
        for &step in steps {
            top += step;
            if matches!(s.push(&sample(page, top, VH)), Fold::Refused(_)) {
                refused += 1;
            }
        }
    }
    let rows = s.dimensions().1;
    assert!(
        rows <= top as u32 + VH,
        "the sheet claims more rows than were scrolled: {rows} > {}+{VH}",
        top as u32
    );
    let want = sample(page, 0., rows);
    assert!(
        s.canvas() == want.as_slice(),
        "the sheet stopped being an exact prefix of the page \
         ({rows} rows after scrolled-to {top})"
    );
    (s, refused)
}

/// The manual-scroll reality check: a real page (per-pixel ink, not
/// row-constant test stripes), a fractional eased-out flick, one frame
/// per poll. Every changed frame must fold, and the result must be the
/// page itself.
#[test]
fn a_smooth_flick_of_a_real_page_folds_every_frame() {
    let page = text_page();
    let steps = [96f32, 92., 84., 66., 44., 24., 10., 4.];
    let (s, refused) = scroll_and_check_prefix(&page, &steps, 3);
    let (pushed, accepted) = s.stats();
    assert_eq!(refused, 0, "no frame of a smooth flick may be refused");
    assert_eq!(accepted, pushed, "every changed frame must fold in");
    // 3 rounds of 420 rows over a 700-row viewport.
    assert_eq!(s.dimensions(), (TW, 1_260 + 700));
}

/// The same gesture over sparse content (a paragraph every 44 rows,
/// blank in between): the matcher's cost surface is nearly flat there,
/// which is exactly where a guessed position used to win. Some frames
/// may be refused as ambiguous; the sheet must still be exact.
#[test]
fn a_sparse_page_stays_exact_whatever_the_matcher_refuses() {
    let page = sparse_page();
    let steps = [96f32, 92., 84., 66., 44., 24., 10., 4.];
    let (s, _) = scroll_and_check_prefix(&page, &steps, 3);
    let (pushed, accepted) = s.stats();
    assert!(accepted >= pushed / 2, "{accepted}/{pushed} folded in");
}

/// A fast trackpad flick outruns the overlap budget: steps beyond
/// 0.8 × 700 rows. Those frames are refused — and the refusal must
/// cost nothing but the lost band. The old code "recovered" by
/// placing them against the frame it had just seen, which shifted
/// every accepted row after the first refusal.
#[test]
fn a_fast_flick_beyond_the_window_refuses_instead_of_shifting() {
    let page = text_page();
    let steps = [300f32, 380., 440., 520., 640.];
    let (s, refused) = scroll_and_check_prefix(&page, &steps, 2);
    assert!(refused > 0, "640 rows cannot be bridged by a 700-row frame");
    let (pushed, accepted) = s.stats();
    assert!(accepted > 0, "the slow part of the flick must still fold");
    assert!(
        accepted < pushed,
        "{accepted}/{pushed} — did the jump fold?"
    );
}

/// A short region (the matcher's window is only ±48 rows here): a
/// jump beyond it writes NOT ONE BYTE, and a frame that still overlaps
/// the untouched base then lands with its FULL displacement — the
/// self-healing half of "refusals are a state, not an error".
#[test]
fn a_jump_beyond_the_window_refuses_and_keeps_the_canvas_byte_identical() {
    let mut s = sheet();
    let seed = s.canvas().to_vec();
    for top in [70u32, 100, 130] {
        let out = s.push(&frame_at(top));
        assert!(matches!(out, Fold::Refused(_)), "jump to {top}: {out:?}");
        assert_eq!(s.canvas(), seed.as_slice(), "a refusal must not write");
        assert_eq!(s.dimensions(), (W, H));
    }
    let out = s.push(&frame_at(40));
    assert!(
        matches!(out, Fold::GrewBottom { dy: 40, growth: 40 }),
        "{out:?}"
    );
    assert_eq!(s.dimensions(), (W, H + 40));
    assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
}

/// The stall is not the end of the session: while nothing links to
/// the base the sheet simply stops growing, and a single nudge back
/// inside the window resumes it with the page rows that were skipped
/// still on the right coordinates — no gap, no duplicate band.
#[test]
fn a_stalled_session_resumes_exactly_after_one_nudge_back() {
    let mut s = sheet();
    let seed = s.canvas().to_vec();
    for top in [70u32, 140, 210] {
        assert!(matches!(s.push(&frame_at(top)), Fold::Refused(_)));
        assert_eq!(s.canvas(), seed.as_slice(), "a stall must not write");
    }
    // One nudge back inside the window, then keep going down.
    let mut extent = 0u32;
    for top in [40u32, 80, 120] {
        let out = s.push(&frame_at(top));
        assert!(
            matches!(out, Fold::GrewBottom { dy, .. } if dy == top - extent),
            "{out:?}"
        );
        extent = top;
    }
    assert_eq!(s.dimensions(), (W, H + 120));
    assert_eq!(s.canvas(), expected_canvas(0, 120).as_slice());
}

/// A flat TOP region of moving content must NOT be pinned: the
/// band-level structure guard is what separates "flat because
/// chrome" from "flat because blank". Runs long enough for the
/// detection ladder to fire — a blank prefix is "still" in every
/// frame, which is exactly what a pinned navbar looks like, and only
/// the in-band structure test tells them apart.
#[test]
fn flat_top_content_is_not_pinned() {
    const H2: u32 = 800;
    const FLAT: u32 = 100; // > any committed-band minimum
    let frame = |top: u32| -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H2 * 4) as usize);
        for row in 0..H2 {
            let px = if row < FLAT {
                [0xff, 0xff, 0xff, 0xff]
            } else {
                source_row(top + row)
            };
            v.extend_from_slice(&px.repeat(W as usize));
        }
        v
    };
    let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
    let mut extent = 0u32;
    for top in [40u32, 80, 120, 160, 200] {
        let out = s.push(&frame(top));
        assert!(
            matches!(out, Fold::GrewBottom { dy, .. } if dy == top - extent),
            "{out:?}"
        );
        extent = top;
        // Still all-flat rows accumulated a run, but no structure →
        // no commit: the span stays whole-viewport.
        assert_eq!(s.viewport_span(), (top, H2), "band pinned at {top}");
    }
}
