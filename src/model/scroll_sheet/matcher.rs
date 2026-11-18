//! The offset matcher: given two consecutive frame profiles, find the
//! vertical translation (or its refusal verdict). Two-stage — eight
//! band-mean signals rank a candidate sweep, a step-spaced full-row
//! MAD verifies the best few — with a structured refusal taxonomy
//! (mid-animation tear, periodic texture, flat region) the capture
//! loop treats as a state, not an error.

use super::profile::{BAND_COUNT, BandWeights, MIN_EVIDENCE_ROWS, RowProfile};
use super::{Refusal, SheetLimits};
use subpixel::refine_basin;

/// The best offset found for a frame pair and the evidence for it.
pub(super) struct OffsetEstimate {
    /// Viewport movement in rows, positive = down (canvas-append side).
    pub(super) dy: i32,
    /// Continuous sub-row displacement (positive = down); `dy` is its
    /// nearest integer (`subpixel_dy.round() as i32`).
    pub(super) subpixel_dy: f32,
    /// Mean |Δ| of the verified row signal at `subpixel_dy` (grayscale units).
    pub(super) cost: f32,
    /// Best cost among offsets outside the ±2-row peak around `dy`
    /// (`f32::INFINITY` when there is no such candidate).
    pub(super) runner_up: f32,
    /// The largest offset the search was allowed to consider. A bad cost
    /// right at this edge reads as "the motion was bigger than the
    /// window", not as "the content changed" (see `classify`).
    pub(super) window: i32,
    /// Moving pixels agree while a textured backdrop stays in screen coordinates.
    pub(super) foreground: bool,
}

/// One matcher verdict against a specific comparison base (see
/// `ScrollSheet::place`).
pub(super) enum Placement {
    /// A usable offset (cost and margin pass, dy ≠ 0).
    Match(OffsetEstimate),
    /// Cost passes at dy 0 — content changed without moving.
    Still,
    /// Unusable, with the taxonomy reason.
    Reject(Refusal),
}

/// Scan all admissible offsets for the best alignment between the anchor
/// frame's profile (`prev`) and the incoming frame's (`cur`).
///
/// With viewport movement `d` (positive = down), the current frame's row
/// `j` shows what the anchor showed at row `j + d` — scrolling down
/// brings content that sat lower in the viewport up to its top — so the
/// comparison window is the overlap `height − |d|`. Candidates are
/// capped at `height × max_motion_ratio` and must leave `min_overlap`
/// rows. The comparison is full-row pixel MAD on a step-spaced grid of
/// rows (see [`RowProfile`] for why means are not enough). `sticky_trim`
/// extends the static edge trim by the committed sticky bands (see
/// [`StickyBands`]) so detected chrome leaves the scan too. Returns
/// [`None`] when no candidate is admissible at all.
/// Current-frame row range compared for viewport movement `d`:
/// `[first, end)`. Positive `d` means the page scrolled down, so the
/// current frame's row `j` matches the anchor's row `j + d`.
fn overlap_window(h: i32, d: i32, trim_top: i32, trim_bot: i32) -> (usize, usize) {
    let end = (h - trim_bot - d.max(0)).max(0) as usize;
    let first = ((-d).max(0).max(trim_top)) as usize;
    (first, end)
}

/// Weighted band-mean cost: the cheap signal that ranks candidate
/// offsets before any full-row work. Masked (fixed-sidebar) bands are
/// skipped and the worst active band drops out (a trimmed mean), so one
/// animated band cannot dominate the ranking.
fn band_cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    min_overlap: i32,
) -> Option<f32> {
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim_top, trim_bot);
    let rows = end.saturating_sub(first);
    if rows < min_overlap as usize {
        return None;
    }
    // Only rows that can testify count (see `RowProfile::row_weight`):
    // a candidate that lands on blank content in both frames is not a
    // cheap match, it is NO match, and must not outrank a real one.
    let mut sums = [0.0f32; BAND_COUNT];
    let mut evidence = 0.0f32;
    let mut counted = 0usize;
    for j in first..end {
        let pj = (j as i32 + d) as usize;
        let w = cur.row_weight[j].min(prev.row_weight[pj]);
        if w <= 0.0 {
            continue;
        }
        evidence += w;
        counted += 1;
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            sums[band] += w * (cur.band_means[j][band] - prev.band_means[pj][band]).abs();
        }
    }
    if counted < MIN_EVIDENCE_ROWS || evidence <= 0.0 {
        return None;
    }
    let mut active: Vec<f32> = sums
        .iter()
        .zip(weights.iter())
        .filter(|&(_, &w)| w > 0.0)
        .map(|(sum, _)| *sum / evidence)
        .collect();
    if active.is_empty() {
        return None;
    }
    if active.len() >= 4 {
        let worst = active.iter().copied().fold(0.0f32, f32::max);
        if let Some(pos) = active.iter().position(|&v| v == worst) {
            active.swap_remove(pos);
        }
    }
    Some(active.iter().sum::<f32>() / active.len() as f32)
}

fn trimmed_weighted_mean(row_sums: &mut Vec<(f32, f32)>) -> Option<f32> {
    let n = row_sums.len();
    if n < MIN_EVIDENCE_ROWS {
        return None;
    }
    let keep = n - n / 7;
    row_sums.select_nth_unstable_by(keep - 1, |a, b| a.0.total_cmp(&b.0));
    row_sums.truncate(keep);
    let weight_sum: f32 = row_sums.iter().map(|(_, w)| w).sum();
    if weight_sum <= 0.0 {
        return None;
    }
    Some(row_sums.iter().map(|(cost, w)| cost * w).sum::<f32>() / weight_sum)
}

/// The exact signal that decides a match: per-row pixel MAD over the
/// step grid, weighted by row evidence and trimmed at the expensive end
/// (fixed edge chrome misaligns under every translation, and dropping the
/// worst rows absorbs it). `None` when the overlap holds too little
/// evidence to judge — see [`MIN_EVIDENCE_ROWS`].
fn row_cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
) -> Option<f32> {
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim_top, trim_bot);
    let first = first.div_ceil(cur.step) * cur.step;
    let stride = cur.stride;
    // (cost, evidence weight) per sampled row.
    let mut row_sums: Vec<(f32, f32)> = Vec::with_capacity(16);
    let mut j = first;
    while j < end {
        let w = cur.row_weight[j].min(prev.row_weight[(j as i32 + d) as usize]);
        if w <= 0.0 {
            j += cur.step;
            continue;
        }
        let mut sum = 0u32;
        let mut width = 0usize;
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            let c = &cur.data[j * stride + start..j * stride + stop];
            let p = &prev.data
                [(j as i32 + d) as usize * stride + start..(j as i32 + d) as usize * stride + stop];
            sum += c
                .iter()
                .zip(p.iter())
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum::<u32>();
            width += stop - start;
        }
        if width == 0 {
            // Defensive: a mask that zeroed every band is meaningless;
            // fall back to the full interior row.
            let c = &cur.data[j * stride..(j + 1) * stride];
            let p =
                &prev.data[(j as i32 + d) as usize * stride..(j as i32 + d + 1) as usize * stride];
            sum = c
                .iter()
                .zip(p.iter())
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum();
            width = stride;
        }
        row_sums.push((sum as f32 / width as f32, w));
        j += cur.step;
    }
    trimmed_weighted_mean(&mut row_sums)
}

/// Row spacing of the coarse sweep. Kept in sync with the ±3 refinement
/// in [`estimate_offset`] (a step of `2·SWEEP_STEP + 1` or less leaves no
/// hole between one basin's refinement and the next).
const SWEEP_STEP: i32 = 3;

/// Candidate offsets for the first search pass: the prediction
/// neighbourhood (the ±128-row window around the last accepted step,
/// where a smooth scroll almost always lands) plus a coarse sweep of the
/// whole range.
///
/// The sweep is anchored at ZERO, never at the range edge. Anchoring it
/// at `-max_admissible` made the grid's PHASE a function of that bound,
/// so which offsets were even looked at depended on a threshold that has
/// nothing to do with the content: a 300-row jump sat on the grid at
/// `max_motion_ratio = 0.6` (480 % 3 == 0) and fell between grid points
/// at 0.8 (640 % 3 == 2), turning a perfect match into Mismatch. Real
/// content hides this (its cost valley is a few rows wide, so a grid
/// point always lands inside one); synthetic one-row-wide valleys do not.
///
/// Small ranges are enumerated fully so tiny test/toolbar-sized frames
/// keep exact behaviour.
fn candidate_offsets(max_admissible: i32, prediction: i32) -> Vec<i32> {
    let mut seen = vec![false; (max_admissible as usize) * 2 + 1];
    let mut candidates = Vec::new();
    let mut push = |d: i32, out: &mut Vec<i32>| {
        if d.abs() <= max_admissible {
            let index = (d + max_admissible) as usize;
            if !seen[index] {
                seen[index] = true;
                out.push(d);
            }
        }
    };
    let prediction = prediction.clamp(-max_admissible, max_admissible);
    for delta in 0..=128 {
        push(prediction - delta, &mut candidates);
        push(prediction + delta, &mut candidates);
    }
    if max_admissible <= 96 {
        for d in -max_admissible..=max_admissible {
            push(d, &mut candidates);
        }
    } else {
        push(0, &mut candidates);
        let mut k = 1;
        while k * SWEEP_STEP <= max_admissible {
            push(k * SWEEP_STEP, &mut candidates);
            push(-k * SWEEP_STEP, &mut candidates);
            k += 1;
        }
        push(max_admissible, &mut candidates);
        push(-max_admissible, &mut candidates);
    }
    candidates
}

/// Find the offset with a two-stage search: rank every candidate with
/// the cheap 8-band signature, refine the three best basins at ±3 rows,
/// then verify the twelve best (pairwise >2 rows apart) offsets with the
/// full-row cost. Output semantics match the old brute-force scan — only
/// the work per frame drops by an order of magnitude, so the capture
/// loop stops being matcher-bound.
///
/// `weights` masks fixed sidebars (see [`MotionMask`]). `prediction` is
/// the last accepted step; it only seeds the search order.
#[allow(clippy::too_many_arguments)]
pub(super) fn estimate_offset(
    prev: &RowProfile,
    cur: &RowProfile,
    options: &SheetLimits,
    sticky_trim: (u32, u32),
    weights: &BandWeights,
    prediction: i32,
    fixed_backdrop: bool,
    allow_foreground: bool,
) -> Option<OffsetEstimate> {
    let h = prev.height.min(cur.height) as i32;
    if h <= 0 || prev.stride != cur.stride {
        return None;
    }
    // Sticky-edge trim: fixed chrome inside the selection (site navbars,
    // toolbar strips) breaks the row-translation assumption — a ~4%
    // fixed band alone pushes the mean cost past the threshold and
    // EVERY frame rejects (the "scrolled up past the site header and
    // the preview stopped syncing" report: log showed one Prepend then
    // an unbroken run of Mismatch). Edges leave the SCAN only — dy
    // semantics are unchanged (translation is uniform below the band)
    // and the canvas still writes whole frames. Small frames (tests,
    // tiny selections) keep every row: there is no budget to spare.
    // Detected sticky bands (adaptive, any height up to the fraction
    // cap) extend the static budget — the static trim survives only as
    // the pre-detection fallback.
    let (trim_top, trim_bot) = if h >= 200 {
        (
            (h / 16).min(96).max(sticky_trim.0 as i32),
            (h / 48).min(32).max(sticky_trim.1 as i32),
        )
    } else {
        (sticky_trim.0 as i32, sticky_trim.1 as i32)
    };
    let max_motion = ((h as f32) * options.max_motion_ratio) as i32;
    let max_admissible = max_motion.min(h - trim_top - trim_bot - options.min_overlap as i32);
    if max_admissible < 0 {
        return None;
    }
    let min_overlap = options.min_overlap as i32;

    if fixed_backdrop {
        if let Some(candidate) = foreground::estimate(
            prev,
            cur,
            weights,
            (trim_top, trim_bot),
            min_overlap,
            prediction,
            max_admissible,
            true,
        ) {
            return Some(candidate);
        }
        let cost = row_cost(prev, cur, 0, weights, trim_top, trim_bot)?;
        return Some(OffsetEstimate {
            dy: 0,
            subpixel_dy: 0.0,
            cost,
            runner_up: f32::INFINITY,
            window: max_admissible,
            foreground: false,
        });
    }

    let mut candidates = candidate_offsets(max_admissible, prediction);
    let sig = |d: i32, prev: &RowProfile, cur: &RowProfile| {
        band_cost(prev, cur, d, weights, trim_top, trim_bot, min_overlap)
    };

    // Stage 1: rank with the band signature and pick the three best
    // basins (separated by 6 rows so one valley is not counted thrice).
    let mut ranked: Vec<(i32, f32)> = candidates
        .iter()
        .filter_map(|&d| sig(d, prev, cur).map(|cost| (d, cost)))
        .collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));
    let mut basins: Vec<i32> = Vec::with_capacity(3);
    for &(d, _) in &ranked {
        if basins.len() == 3 {
            break;
        }
        if basins.iter().all(|other| (other - d).abs() > 6) {
            basins.push(d);
        }
    }
    // Stage 2: refine each basin at ±3 rows (the coarse sweep is step 3).
    for &basin in &basins {
        for delta in -3..=3 {
            let d = basin + delta;
            if d.abs() <= max_admissible && !candidates.contains(&d) {
                candidates.push(d);
            }
        }
    }
    let mut ranked: Vec<(i32, f32)> = candidates
        .iter()
        .filter_map(|&d| sig(d, prev, cur).map(|cost| (d, cost)))
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    // Stage 3: verify up to 12 offsets at least 3 rows apart with the
    // full-row cost. d = 0 stays in the pool so flat-content ties still
    // resolve to Still rather than a phantom offset.
    let mut shortlist: Vec<i32> = Vec::with_capacity(12);
    let mut include_zero = false;
    for &(d, _) in &ranked {
        if shortlist.len() == 12 {
            break;
        }
        include_zero |= d == 0;
        if shortlist.iter().all(|other| (other - d).abs() > 2) {
            shortlist.push(d);
        }
    }
    if !include_zero {
        shortlist.push(0);
    }
    let mut int_evaluated: Vec<(i32, f32)> = Vec::with_capacity(shortlist.len());
    let mut int_cache: Vec<(i32, Option<f32>)> = Vec::with_capacity(shortlist.len() + 16);
    for d in shortlist {
        // `None` = the overlap holds too little evidence to judge this
        // offset at all (a blank seam): it is not a candidate, and it is
        // not a runner-up either.
        let cost = row_cost(prev, cur, d, weights, trim_top, trim_bot);
        int_cache.push((d, cost));
        if let Some(cost) = cost {
            int_evaluated.push((d, cost));
        }
    }
    int_evaluated.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    // Refine the top candidate basins (and the best non-neighbour runner-up
    // basin) at subpixel resolution before picking `best` and `runner_up`:
    // refining only `best` would let a periodic striped table at a fractional
    // offset refine `best_cost` down to ~0 while leaving its periodic alias
    // `d ± period` at unrefined half-pixel cost, falsely clearing
    // `ambiguity_margin` instead of surfacing `Refusal::Unclear`.
    // `(dy, subpixel_dy, cost, refined)`
    let mut evaluated: Vec<(i32, f32, f32, bool)> = Vec::with_capacity(int_evaluated.len());
    let mut refined_count = 0usize;
    let mut min_refined_cost = f32::INFINITY;
    let mut crisp_exact_int = false;
    for &(d, cost) in &int_evaluated {
        let near_refined = evaluated
            .iter()
            .any(|&(rd, _, _, is_ref)| is_ref && (rd - d).abs() <= 2);
        let limit = if min_refined_cost <= options.cost_threshold {
            2
        } else {
            4
        };
        let skip_crisp_runner_up = crisp_exact_int && cost > 16.0;
        if refined_count < limit && !near_refined && !skip_crisp_runner_up {
            let (r_dy, r_sub, r_cost) = refine_basin(
                prev,
                cur,
                d,
                cost,
                max_admissible,
                weights,
                trim_top,
                trim_bot,
                !crisp_exact_int,
                &mut int_cache,
            );
            if r_cost == 0.0 && (r_sub - r_dy as f32).abs() <= 1e-4 {
                crisp_exact_int = true;
            }
            min_refined_cost = min_refined_cost.min(r_cost);
            evaluated.push((r_dy, r_sub, r_cost, true));
            refined_count += 1;
        } else {
            evaluated.push((d, d as f32, cost, false));
        }
    }

    let best_entry = |items: &[(i32, f32, f32, bool)]| {
        items.iter().copied().min_by(|a, b| {
            a.2.total_cmp(&b.2)
                .then_with(|| a.1.abs().total_cmp(&b.1.abs()))
        })
    };

    // Ensure the winning runner-up outside the ±2-row correlation peak has
    // also been subpixel-refined when `best` is admissible (`runner_up` is
    // unused once `best_cost > cost_threshold`).
    if let Some((mut best_dy, mut best_sub, mut best_cost, _)) = best_entry(&evaluated)
        && best_cost <= options.cost_threshold
    {
        loop {
            let best_is_crisp_int = best_cost == 0.0 && (best_sub - best_dy as f32).abs() <= 1e-4;
            let already_ambiguous = evaluated.iter().any(|&(d, sub, c, is_ref)| {
                is_ref
                    && (d - best_dy).abs() > 2
                    && (sub - best_sub).abs() > 2.0
                    && c - best_cost < options.ambiguity_margin
            });
            if already_ambiguous {
                break;
            }
            let next_unrefined = evaluated
                .iter()
                .enumerate()
                .filter(|(_, (d, sub, _, _))| {
                    (*d - best_dy).abs() > 2 && (*sub - best_sub).abs() > 2.0
                })
                .min_by(|(_, a), (_, b)| a.2.total_cmp(&b.2))
                .and_then(|(idx, &(_, _, c, is_ref))| {
                    (!is_ref && !(best_is_crisp_int && c > 16.0)).then_some(idx)
                });
            let Some(idx) = next_unrefined else {
                break;
            };
            let (d, _, cost, _) = evaluated[idx];
            let (r_dy, r_sub, r_cost) = refine_basin(
                prev,
                cur,
                d,
                cost,
                max_admissible,
                weights,
                trim_top,
                trim_bot,
                !best_is_crisp_int,
                &mut int_cache,
            );
            evaluated[idx] = (r_dy, r_sub, r_cost, true);
            if r_cost < best_cost || (r_cost == best_cost && r_sub.abs() < best_sub.abs()) {
                best_dy = r_dy;
                best_sub = r_sub;
                best_cost = r_cost;
            }
        }
    }

    let best = best_entry(&evaluated);
    let zero_cost = int_cache
        .iter()
        .find(|&&(k, _)| k == 0)
        .and_then(|&(_, c)| c);
    // A uniform sub-half-pixel micro-scroll (`dy == 0`, `sub != 0.0`) whose
    // whole-row MAD collapsed far below `row_cost(0)` is a genuine subpixel
    // viewport shift, not a stationary backdrop masking jumped foreground.
    let verified_subpixel_micro = best.is_some_and(|(dy, sub, cost, _)| {
        dy == 0
            && sub != 0.0
            && cost <= options.cost_threshold
            && zero_cost.is_some_and(|zc| cost * 2.0 < zc)
    });
    // A fixed backdrop can supply a deceptively confident ordinary match.
    // Validate foreground motion independently before accepting its position,
    // and surface rejected foreground motion rather than letting a stationary
    // backdrop pull `d = 0` below `cost_threshold` as `Still`.
    if allow_foreground
        && !verified_subpixel_micro
        && let Some(candidate) = foreground::estimate(
            prev,
            cur,
            weights,
            (trim_top, trim_bot),
            min_overlap,
            prediction,
            max_admissible,
            false,
        )
        && (candidate.cost <= options.cost_threshold
            || best.is_none_or(|(dy, _, cost, _)| dy == 0 || cost > options.cost_threshold))
    {
        return Some(candidate);
    }
    let (best_dy, best_subpixel_dy, best_cost, _) = best?;
    let runner_up = evaluated
        .iter()
        .filter(|(d, sub, _, _)| (*d - best_dy).abs() > 2 && (*sub - best_subpixel_dy).abs() > 2.0)
        .map(|(_, _, c, _)| *c)
        .min_by(f32::total_cmp)
        .unwrap_or(f32::INFINITY);
    Some(OffsetEstimate {
        dy: best_dy,
        subpixel_dy: best_subpixel_dy,
        cost: best_cost,
        runner_up,
        window: max_admissible,
        foreground: false,
    })
}

mod foreground;
mod subpixel;
