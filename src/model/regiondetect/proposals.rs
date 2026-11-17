//! Candidate generation and scoring: proposal sources (closed contours,
//! connected components, edge-bounded background, paired long lines,
//! and orthogonal rounded-box completion), four-side border-support
//! scoring with texture penalties, and original-resolution side
//! refinement that undoes downscale blur without snapping to shadows.

use super::edges::{EdgeField, connected_components, contour_rects};
use super::image::{GrayImage, RgbaView, color_difference};
use super::{IntRect, RegionCandidate, Sources};
// ── Proposals ────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub(super) struct Proposal {
    pub(super) rect: IntRect,
    pub(super) source: Sources,
}

/// A run of edge pixels along one scanline.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Segment {
    fixed: i32,
    start: i32,
    end: i32,
}

/// Extract directional segments (gaps ≤ 3 px allowed, minimum span
/// `min_len` px), deduplicated and capped at `max_keep` per direction.
fn extract_segments(
    edges: &GrayImage,
    vertical: bool,
    min_len: i32,
    max_keep: usize,
) -> Vec<Segment> {
    let across = if vertical { edges.width } else { edges.height };
    let along = if vertical { edges.height } else { edges.width };
    let mut segments: Vec<Segment> = Vec::new();
    for fixed in 0..across {
        let mut start: i32 = -1;
        let mut last = 0;
        for position in 0..=along {
            let hit = position < along
                && (if vertical {
                    edges.at(fixed, position)
                } else {
                    edges.at(position, fixed)
                }) != 0;
            if hit {
                if start < 0 {
                    start = position;
                }
                last = position;
            } else if start >= 0 && (position == along || position - last > 3) {
                if last + 1 - start >= min_len {
                    segments.push(Segment {
                        fixed,
                        start,
                        end: last + 1,
                    });
                }
                start = -1;
            }
        }
    }

    // Longest first; near-duplicates (parallel double edges of one
    // border) collapse to a single segment.
    segments.sort_by_key(|s| std::cmp::Reverse(s.end - s.start));
    let mut unique: Vec<Segment> = Vec::new();
    for line in segments {
        let duplicate = unique.iter().any(|other| {
            (line.fixed - other.fixed).abs() <= 2
                && (line.start - other.start).abs() <= 5
                && (line.end - other.end).abs() <= 5
        });
        if !duplicate {
            unique.push(line);
            if unique.len() >= max_keep {
                break;
            }
        }
    }
    unique
}

fn long_segments(edges: &GrayImage, vertical: bool) -> Vec<Segment> {
    extract_segments(edges, vertical, 25, 192)
}

/// Proposals from closed contour bounding boxes.
pub(super) fn contour_proposals(edges: &GrayImage) -> Vec<Proposal> {
    contour_rects(edges)
        .into_iter()
        .map(|rect| Proposal {
            rect,
            source: Sources::CONTOUR,
        })
        .collect()
}

/// Proposals from connected components — foreground blobs (controls
/// with solid fills) and, in background mode, the panels delimited by
/// edges: the edge-bounded background regions reveal panel/splitter
/// layouts that no single contour covers.
pub(super) fn component_proposals(image: &GrayImage, background: bool) -> Vec<Proposal> {
    connected_components(image, !background)
        .into_iter()
        .skip(1)
        .filter(|item| item.pixels >= 24)
        .filter_map(|item| {
            let rect = IntRect::new(item.left, item.top, item.right, item.bottom);
            // A background region only counts when it is nearly filled:
            // an L-shaped leftover around overlapping windows is not a
            // selectable panel.
            if background && f64::from(item.pixels) / (rect.area().max(1) as f64) < 0.72 {
                return None;
            }
            Some(Proposal {
                rect,
                source: if background {
                    Sources::BACKGROUND
                } else {
                    Sources::COMPONENT
                },
            })
        })
        .collect()
}

/// Pair two long parallel segments of similar extent into a rect.
/// Windows and panels whose border is only partially detected (rounded
/// corners, shadows) still produce two solid long sides.
pub(super) fn line_proposals(field: &EdgeField) -> Vec<Proposal> {
    let mut proposals = Vec::new();
    for vertical in [true, false] {
        let edges = if vertical {
            &field.vertical
        } else {
            &field.horizontal
        };
        let mut segments = long_segments(edges, vertical);
        segments.sort_by_key(|s| s.fixed);
        for index in 0..segments.len() {
            let first = segments[index];
            let aligned: Vec<Segment> = segments[index + 1..]
                .iter()
                .copied()
                .filter(|second| {
                    let tolerance = pair_tolerance(first, *second);
                    (first.fixed - second.fixed).abs() >= 8
                        && (first.start - second.start).abs() <= tolerance
                        && (first.end - second.end).abs() <= tolerance
                })
                .collect();
            if aligned.is_empty() {
                continue;
            }
            // A few pairs only: the nearest partners plus the farthest —
            // exhaustive pairing was measured as noise, not coverage.
            let mut pairs: Vec<Segment> = aligned.iter().copied().take(2).collect();
            if let Some(&last) = aligned.last()
                && !pairs.contains(&last)
            {
                pairs.push(last);
            }
            for second in pairs {
                let tolerance = pair_tolerance(first, second);
                if (first.fixed - second.fixed).abs() < 8
                    || (first.start - second.start).abs() > tolerance
                    || (first.end - second.end).abs() > tolerance
                {
                    continue;
                }
                let lo = first.fixed.min(second.fixed);
                let hi = first.fixed.max(second.fixed) + 1;
                let start = first.start.min(second.start);
                let end = first.end.max(second.end);
                proposals.push(Proposal {
                    rect: if vertical {
                        IntRect::new(lo, start, hi, end)
                    } else {
                        IntRect::new(start, lo, end, hi)
                    },
                    source: Sources::LINES,
                });
            }
        }
    }
    proposals
}

/// Complete four-sided rounded boxes from orthogonal segment pairs.
///
/// Hollow rounded-corner cards, inputs, and dialogs on same-color
/// backgrounds leave ~6..28 px open corner gaps where antialiased
/// quarter-circle arcs fall below directional thresholds: 1 px closing
/// cannot bridge them, background components leak through the open
/// corners, and 2-line pairing (`line_proposals`) clips `r` pixels off
/// the perpendicular ends so `border_sides` misses the horizontal or
/// vertical borders completely. Witnessing all four straight sides
/// within a plausible corner-radius gap recovers the full bounding rect
/// and qualifies as a `CONTOUR` candidate rather than a 2-line guess.
pub(super) fn quad_proposals(field: &EdgeField) -> Vec<Proposal> {
    const MIN_SEG_LEN: i32 = 12;
    const MAX_SEGS_PER_AXIS: usize = 256;
    const MAX_CORNER_GAP: i32 = 30;
    const CORNER_OVERSHOOT: i32 = 3;
    const MAX_PER_LEFT_SEG: usize = 8;
    const MAX_QUADS: usize = 512;

    let mut v_segs = extract_segments(&field.vertical, true, MIN_SEG_LEN, MAX_SEGS_PER_AXIS);
    let mut h_segs = extract_segments(&field.horizontal, false, MIN_SEG_LEN, MAX_SEGS_PER_AXIS);
    v_segs.sort_by_key(|s| s.fixed);
    h_segs.sort_by_key(|s| s.fixed);

    let mut scored: Vec<(i32, Proposal)> = Vec::new();
    let mut per_left: Vec<(i32, Proposal)> = Vec::new();
    let mut top_matches: Vec<(i32, Segment)> = Vec::new();
    let mut bot_matches: Vec<(i32, Segment)> = Vec::new();

    for i in 0..v_segs.len() {
        let left_seg = v_segs[i];
        per_left.clear();
        for right_seg in v_segs[i + 1..].iter().copied() {
            let left = left_seg.fixed;
            let right = right_seg.fixed + 1;
            let width = right - left;
            if width < 12 {
                continue;
            }

            let top_min = (left_seg.start - MAX_CORNER_GAP).max(right_seg.start - MAX_CORNER_GAP);
            let top_max =
                (left_seg.start + CORNER_OVERSHOOT).min(right_seg.start + CORNER_OVERSHOOT);
            if top_min > top_max {
                continue;
            }
            let bot_min =
                (left_seg.end - CORNER_OVERSHOOT - 1).max(right_seg.end - CORNER_OVERSHOOT - 1);
            let bot_max =
                (left_seg.end + MAX_CORNER_GAP - 1).min(right_seg.end + MAX_CORNER_GAP - 1);
            if bot_min > bot_max {
                continue;
            }

            let top_lo = h_segs.partition_point(|s| s.fixed < top_min);
            let top_hi = h_segs.partition_point(|s| s.fixed <= top_max);
            top_matches.clear();
            for &seg in &h_segs[top_lo..top_hi] {
                let tl_x = seg.start - left;
                let tr_x = right - seg.end;
                if (-CORNER_OVERSHOOT..=MAX_CORNER_GAP).contains(&tl_x)
                    && (-CORNER_OVERSHOOT..=MAX_CORNER_GAP).contains(&tr_x)
                {
                    let tl_y = left_seg.start - seg.fixed;
                    let tr_y = right_seg.start - seg.fixed;
                    let err = (tl_x - tl_y).abs() + (tr_x - tr_y).abs();
                    top_matches.push((err, seg));
                }
            }
            if top_matches.is_empty() {
                continue;
            }
            top_matches.sort_by_key(|(err, _)| *err);
            top_matches.truncate(2);

            let bot_lo = h_segs.partition_point(|s| s.fixed < bot_min);
            let bot_hi = h_segs.partition_point(|s| s.fixed <= bot_max);
            bot_matches.clear();
            for &seg in &h_segs[bot_lo..bot_hi] {
                let bl_x = seg.start - left;
                let br_x = right - seg.end;
                if (-CORNER_OVERSHOOT..=MAX_CORNER_GAP).contains(&bl_x)
                    && (-CORNER_OVERSHOOT..=MAX_CORNER_GAP).contains(&br_x)
                {
                    let bottom = seg.fixed + 1;
                    let bl_y = bottom - left_seg.end;
                    let br_y = bottom - right_seg.end;
                    let err = (bl_x - bl_y).abs() + (br_x - br_y).abs();
                    bot_matches.push((err, seg));
                }
            }
            if bot_matches.is_empty() {
                continue;
            }
            bot_matches.sort_by_key(|(err, _)| *err);
            bot_matches.truncate(2);

            for &(_, top_seg) in &top_matches {
                let top = top_seg.fixed;
                for &(_, bot_seg) in &bot_matches {
                    let bottom = bot_seg.fixed + 1;
                    let height = bottom - top;
                    if height < 12 {
                        continue;
                    }
                    let r_max = (width.min(height) / 2 + 1).clamp(4, MAX_CORNER_GAP);
                    let in_gap = |gap: i32| (-CORNER_OVERSHOOT..=r_max).contains(&gap);
                    let tl_x = top_seg.start - left;
                    let tr_x = right - top_seg.end;
                    let bl_x = bot_seg.start - left;
                    let br_x = right - bot_seg.end;
                    let tl_y = left_seg.start - top;
                    let bl_y = bottom - left_seg.end;
                    let tr_y = right_seg.start - top;
                    let br_y = bottom - right_seg.end;
                    if !(in_gap(tl_x)
                        && in_gap(tr_x)
                        && in_gap(bl_x)
                        && in_gap(br_x)
                        && in_gap(tl_y)
                        && in_gap(bl_y)
                        && in_gap(tr_y)
                        && in_gap(br_y))
                    {
                        continue;
                    }
                    // Each corner's horizontal and vertical gaps come from the
                    // same arc, so reject asymmetric accidental crossings where
                    // one segment touches the wall and the other stops far away.
                    let corner_balanced = |gx: i32, gy: i32| (gx.max(0) - gy.max(0)).abs() <= 14;
                    if !(corner_balanced(tl_x, tl_y)
                        && corner_balanced(tr_x, tr_y)
                        && corner_balanced(bl_x, bl_y)
                        && corner_balanced(br_x, br_y))
                    {
                        continue;
                    }
                    // Rank by corner symmetry (not raw gap size, which would
                    // starve r=16..24 rounded dialogs behind sharp grid cells
                    // that contours already cover).
                    let asymmetry = (tl_x - tl_y).abs()
                        + (tr_x - tr_y).abs()
                        + (bl_x - bl_y).abs()
                        + (br_x - br_y).abs()
                        + (tl_x - tr_x).abs()
                        + (bl_x - br_x).abs();
                    per_left.push((
                        asymmetry,
                        Proposal {
                            rect: IntRect::new(left, top, right, bottom),
                            source: Sources::CONTOUR,
                        },
                    ));
                }
            }
        }
        per_left.sort_by_key(|(asymmetry, _)| *asymmetry);
        per_left.truncate(MAX_PER_LEFT_SEG);
        scored.extend_from_slice(&per_left);
    }

    scored.sort_by_key(|(asymmetry, _)| *asymmetry);
    let mut proposals: Vec<Proposal> = Vec::with_capacity(scored.len().min(MAX_QUADS));
    for (_, proposal) in scored {
        let duplicate = proposals.iter().any(|existing| {
            (existing.rect.left - proposal.rect.left).abs() <= 2
                && (existing.rect.top - proposal.rect.top).abs() <= 2
                && (existing.rect.right - proposal.rect.right).abs() <= 2
                && (existing.rect.bottom - proposal.rect.bottom).abs() <= 2
        });
        if !duplicate {
            proposals.push(proposal);
            if proposals.len() >= MAX_QUADS {
                break;
            }
        }
    }
    proposals
}

/// Alignment tolerance for a pair, proportional to the shorter segment
/// (12%), clamped to [4, 16] px.
fn pair_tolerance(a: Segment, b: Segment) -> i32 {
    ((a.end - a.start).min(b.end - b.start) / 12).clamp(4, 16)
}

// ── Scoring ──────────────────────────────────────────────────────────

fn span_support(
    edges: &GrayImage,
    fixed: i32,
    limit: i32,
    start: i32,
    end: i32,
    vertical: bool,
) -> f32 {
    if end <= start {
        return 0.;
    }
    let step = ((end - start) / 160).max(1);
    let mut best = 0f32;
    let across_max = (fixed + 2).min(limit - 1);
    for across in (fixed - 2).max(0)..=across_max {
        let (mut supported, mut samples) = (0i32, 0i32);
        let mut along = start;
        while along < end {
            let hit = if vertical {
                edges.at(across, along)
            } else {
                edges.at(along, across)
            } != 0;
            supported += i32::from(hit);
            samples += 1;
            along += step;
        }
        best = best.max(supported as f32 / samples.max(1) as f32);
    }
    best
}

/// Continuous support of ONE rectangle side: the fraction of sampled
/// positions (10%-trimmed ends, stride to cap at ~160 samples) where a
/// directional edge sits within ±2 px of the side. Sides at the image
/// border are trusted at 0.85 — a panel touching the screenshot edge
/// must not lose points for the edge it cannot show.
///
/// Pill buttons and large-radius dialogs curve inward past the 10% trim
/// (`min(12, len/10)`), so sides ≥ 16 px also evaluate a corner-trimmed
/// central span (`min(24, len/4)`); only a nearly continuous straight
/// center (≥ 0.85) earns the 0.95-discounted score so short interior
/// dashes still cannot masquerade as full borders.
fn side_support(edges: &GrayImage, fixed: i32, start: i32, end: i32, vertical: bool) -> f32 {
    let limit = if vertical { edges.width } else { edges.height };
    if fixed <= 1 || fixed + 2 >= limit {
        return 0.85;
    }
    let len = end - start;
    let margin = (len / 10).min(12);
    let standard_best = span_support(edges, fixed, limit, start + margin, end - margin, vertical);
    if len >= 16 {
        let corner_margin = (len / 4).min(24);
        if corner_margin > margin {
            let central_best = span_support(
                edges,
                fixed,
                limit,
                start + corner_margin,
                end - corner_margin,
                vertical,
            );
            if central_best >= 0.85 {
                return standard_best.max(central_best * 0.95);
            }
        }
    }
    standard_best
}

/// Support of all four sides: left, top, right, bottom.
pub(super) fn border_sides(field: &EdgeField, rect: IntRect) -> [f32; 4] {
    [
        side_support(&field.vertical, rect.left, rect.top, rect.bottom, true),
        side_support(&field.horizontal, rect.top, rect.left, rect.right, false),
        side_support(&field.vertical, rect.right - 1, rect.top, rect.bottom, true),
        side_support(
            &field.horizontal,
            rect.bottom - 1,
            rect.left,
            rect.right,
            false,
        ),
    ]
}

/// Edge density inside the rect (3 px inset, ≤32×32 sample grid) — the
/// texture signal: text blocks and icon grids light up.
pub(super) fn interior_density(edges: &GrayImage, rect: IntRect) -> f32 {
    const INSET: i32 = 3;
    if rect.width() <= INSET * 2 || rect.height() <= INSET * 2 {
        return 0.;
    }
    let (mut count, mut samples) = (0i32, 0i32);
    let y_step = (rect.height() / 32).max(1);
    let x_step = (rect.width() / 32).max(1);
    let mut y = rect.top + INSET;
    while y < rect.bottom - INSET {
        let mut x = rect.left + INSET;
        while x < rect.right - INSET {
            count += i32::from(edges.at(x, y) != 0);
            samples += 1;
            x += x_step;
        }
        y += y_step;
    }
    count as f32 / samples.max(1) as f32
}

/// Border support of the rect inset by 4 px: a solid border (panel)
/// keeps support; a cluster of inner edges (text) collapses.
pub(super) fn inset_support(field: &EdgeField, rect: IntRect) -> f32 {
    if rect.width() < 12 || rect.height() < 12 {
        return 0.;
    }
    let inner = IntRect::new(rect.left + 4, rect.top + 4, rect.right - 4, rect.bottom - 4);
    let sides = border_sides(field, inner);
    (sides[0] + sides[1] + sides[2] + sides[3]) / 4.
}

/// Analysis-px rect → original-resolution px. Rounding is `(v·to +
/// from/2)/from` so edges land on the pixel they came from.
pub(super) fn scale_rect(
    rect: IntRect,
    analysis_w: i32,
    analysis_h: i32,
    width: i32,
    height: i32,
) -> IntRect {
    let convert = |value: i32, from: i32, to: i32| {
        ((i64::from(value) * i64::from(to) + i64::from(from) / 2) / i64::from(from)) as i32
    };
    IntRect::new(
        convert(rect.left, analysis_w, width),
        convert(rect.top, analysis_h, height),
        convert(rect.right, analysis_w, width),
        convert(rect.bottom, analysis_h, height),
    )
}

#[allow(clippy::too_many_arguments)]
fn refine_side_span(
    view: &RgbaView<'_>,
    original: i32,
    start: i32,
    end: i32,
    vertical: bool,
    radius: i32,
    threshold: u8,
    min_coverage: f32,
) -> Option<(i32, f32)> {
    let limit = if vertical { view.width } else { view.height };
    let along_limit = if vertical { view.height } else { view.width };
    let start = start.max(0);
    let end = end.min(along_limit);
    if end <= start {
        return None;
    }
    let step = ((end - start) / 128).max(1);
    let sample_px = |normal: i32, along: i32| {
        if vertical {
            view.px(normal, along)
        } else {
            view.px(along, normal)
        }
    };
    let min_diff = threshold.min(8);
    let mut best = None;
    let mut best_score = 0f32;
    for position in (original - radius).max(1)..=(original + radius).min(limit - 1) {
        let (mut count, mut strength, mut samples) = (0i32, 0f32, 0i32);
        let mut along = start;
        while along < end {
            let p_m1 = sample_px(position - 1, along);
            let p_0 = sample_px(position, along);
            let diff = color_difference(p_m1, p_0);
            let diff_prev = if position >= 2 {
                color_difference(sample_px(position - 2, along), p_m1)
            } else {
                0
            };
            let diff_next = if position + 1 < limit {
                color_difference(p_0, sample_px(position + 1, along))
            } else {
                0
            };
            count += i32::from(diff >= min_diff);
            // Saturating contrast at 48 made a diff≈50 drop-shadow band tie a
            // crisp diff=180 panel edge, leaving downscaled estimates stuck on
            // the outer shadow halo. Keep headroom up to 192 and reward local
            // normal sharpness (at least one flat flank + dominance over
            // adjacent steps) so crisp step/1px borders beat diffuse ramps.
            let diff_f = f32::from(diff);
            let contrast = 0.35 * (diff_f / 48.).min(1.) + 0.65 * (diff_f / 192.).min(1.);
            let sharpness = if diff > 0 {
                let flank_min = f32::from(diff_prev.min(diff_next));
                let flank_max = f32::from(diff_prev.max(diff_next));
                let step_isolation = (1. - flank_min / diff_f).clamp(0., 1.);
                let peak_dominance = ((diff_f - flank_max * 0.5) / diff_f).clamp(0., 1.);
                0.6 * step_isolation + 0.4 * peak_dominance
            } else {
                0.
            };
            strength += contrast * (0.4 + 0.6 * sharpness);
            samples += 1;
            along += step;
        }
        let coverage = count as f32 / samples.max(1) as f32;
        let avg_strength = strength / samples.max(1) as f32;
        let score =
            coverage * 0.72 + avg_strength * 0.28 - (position - original).abs() as f32 * 0.010;
        if coverage >= min_coverage && score > best_score {
            best_score = score;
            best = Some((position, score));
        }
    }
    best
}

/// Refine ONE side on the original image: search ±radius px around the
/// downscaled estimate for the position with the best coverage of
/// color-difference boundaries, contrast headroom, and normal sharpness,
/// comparing a corner-trimmed span (`min(24, len/4)`) so rounded-corner
/// borders do not lose to a straighter outer shadow blur.
fn refine_side(
    view: &RgbaView<'_>,
    original: i32,
    start: i32,
    end: i32,
    vertical: bool,
    radius: i32,
    threshold: u8,
) -> i32 {
    let limit = if vertical { view.width } else { view.height };
    if original <= 1 || original >= limit - 1 {
        return original;
    }
    let len = end - start;
    let margin = (len / 10).clamp(1, 24);
    let standard = refine_side_span(
        view,
        original,
        start + margin,
        end - margin,
        vertical,
        radius,
        threshold,
        0.55,
    );
    if len >= 16 {
        let corner_margin = (len / 4).min(24);
        if corner_margin > margin
            && let Some((central_pos, central_score)) = refine_side_span(
                view,
                original,
                start + corner_margin,
                end - corner_margin,
                vertical,
                radius,
                threshold,
                0.80,
            )
            && standard.is_none_or(|(_, std_score)| central_score * 0.95 > std_score)
        {
            return central_pos;
        }
    }
    standard.map_or(original, |(pos, _)| pos)
}

/// Refine all four sides. Left/right refine first (over the full
/// height), then top/bottom over the REFINED width — a vertical side
/// needs the widest possible evidence span.
pub(super) fn refine_rect(
    view: &RgbaView<'_>,
    rect: IntRect,
    radius: i32,
    threshold: u8,
) -> IntRect {
    let left = refine_side(
        view,
        rect.left,
        rect.top,
        rect.bottom,
        true,
        radius,
        threshold,
    );
    let right = refine_side(
        view,
        rect.right,
        rect.top,
        rect.bottom,
        true,
        radius,
        threshold,
    );
    let top = refine_side(view, rect.top, left, right, false, radius, threshold);
    let bottom = refine_side(view, rect.bottom, left, right, false, radius, threshold);
    if right - left < 8 || bottom - top < 8 {
        return rect; // refinement collapsed: keep the estimate
    }
    IntRect::new(
        left.max(0),
        top.max(0),
        right.min(view.width),
        bottom.min(view.height),
    )
}

/// Two rects are the same candidate when all four edges sit within
/// `tolerance` AND areas are within 20% of each other — equal edges
/// with wildly different areas are a nested rect, not a duplicate.
pub(super) fn nearly_same_rect(a: IntRect, b: IntRect, tolerance: i32) -> bool {
    let (area_a, area_b) = (a.area(), b.area());
    (a.left - b.left).abs() <= tolerance
        && (a.top - b.top).abs() <= tolerance
        && (a.right - b.right).abs() <= tolerance
        && (a.bottom - b.bottom).abs() <= tolerance
        && f64::from(area_a.min(area_b) as u32) / f64::from(area_a.max(area_b).max(1) as u32) > 0.8
}

/// Merge `candidate` into the list: on a near-duplicate keep the
/// higher-quality RECT but union the sources (multi-source = more
/// trustworthy).
pub(super) fn merge_candidate(
    candidates: &mut Vec<RegionCandidate>,
    candidate: RegionCandidate,
    tolerance: i32,
) {
    for existing in candidates.iter_mut() {
        if !nearly_same_rect(existing.rect, candidate.rect, tolerance) {
            continue;
        }
        let sources = existing.sources.union(candidate.sources);
        if candidate.quality > existing.quality {
            *existing = candidate;
        }
        existing.sources = sources;
        return;
    }
    candidates.push(candidate);
}
