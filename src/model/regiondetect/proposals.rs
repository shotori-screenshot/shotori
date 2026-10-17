//! Candidate generation and scoring: four proposal sources (closed
//! contours, connected components, edge-bounded background, paired
//! long lines), the four-side border-support scoring with its texture
//! penalties, and the original-resolution side refinement that undoes
//! the downscale blur.

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

/// Extract sufficiently long directional segments (gaps ≤ 3 px allowed,
/// minimum length 24 px), deduplicated and capped at 192 per direction.
fn long_segments(edges: &GrayImage, vertical: bool) -> Vec<Segment> {
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
                if last - start >= 24 {
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
            if unique.len() >= 192 {
                break;
            }
        }
    }
    unique
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
            let last = *aligned.last().expect("non-empty");
            if !pairs.contains(&last) {
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

/// Alignment tolerance for a pair, proportional to the shorter segment
/// (12%), clamped to [4, 16] px.
fn pair_tolerance(a: Segment, b: Segment) -> i32 {
    ((a.end - a.start).min(b.end - b.start) / 12).clamp(4, 16)
}

// ── Scoring ──────────────────────────────────────────────────────────

/// Continuous support of ONE rectangle side: the fraction of sampled
/// positions (10%-trimmed ends, stride to cap at ~160 samples) where a
/// directional edge sits within ±2 px of the side. Sides at the image
/// border are trusted at 0.85 — a panel touching the screenshot edge
/// must not lose points for the edge it cannot show.
fn side_support(edges: &GrayImage, fixed: i32, start: i32, end: i32, vertical: bool) -> f32 {
    let limit = if vertical { edges.width } else { edges.height };
    if fixed <= 1 || fixed + 2 >= limit {
        return 0.85;
    }
    let margin = ((end - start) / 10).min(12);
    let (start, end) = (start + margin, end - margin);
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

/// Refine ONE side on the original image: search ±radius px around the
/// downscaled estimate for the position with the best coverage of
/// color-difference boundaries (count ≥ min(threshold, 8)), with a tiny
/// strength term and a distance penalty so ties stay near the estimate.
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
    let margin = ((end - start) / 10).clamp(1, 24);
    let along_limit = if vertical { view.height } else { view.width };
    let start = (start + margin).max(0);
    let end = (end - margin).min(along_limit);
    if end <= start {
        return original;
    }
    let step = ((end - start) / 128).max(1);
    let mut best = original;
    let mut best_score = 0f32;
    for position in (original - radius).max(1)..=(original + radius).min(limit - 1) {
        let (mut count, mut strength, mut samples) = (0i32, 0f32, 0i32);
        let mut along = start;
        while along < end {
            let a = if vertical {
                view.px(position - 1, along)
            } else {
                view.px(along, position - 1)
            };
            let b = if vertical {
                view.px(position, along)
            } else {
                view.px(along, position)
            };
            let diff = color_difference(a, b);
            count += i32::from(diff >= threshold.min(8));
            strength += f32::from(diff.min(48)) / 48.;
            samples += 1;
            along += step;
        }
        let coverage = count as f32 / samples.max(1) as f32;
        let score = coverage * 0.85 + strength / samples.max(1) as f32 * 0.15
            - (position - original).abs() as f32 * 0.008;
        if coverage >= 0.55 && score > best_score {
            best_score = score;
            best = position;
        }
    }
    best
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
