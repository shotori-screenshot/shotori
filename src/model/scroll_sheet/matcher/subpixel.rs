//! Sub-row bi-phase verification and L1 cone refinement for [`super::estimate_offset`].

use super::super::profile::{BandWeights, RowProfile};
use super::{overlap_window, row_cost, trimmed_weighted_mean};

/// RMS amplitude ratio of two 2-tap vertical blends `((64 - p64)/64, p64/64)`
/// and `((64 - c64)/64, c64/64)` relative to an unblended row difference.
///
/// Dividing a bi-phase subpixel MAD by this factor keeps unrelated/torn row
/// pairs on the exact same cost scale as [`row_cost`] (`phase_gain_64(0, 0) == 1`)
/// so vertical box-filtering cannot sneak noisy or periodic runner-up offsets
/// below `cost_threshold` or shrink `ambiguity_margin`.
fn phase_gain_64(p64: usize, c64: usize) -> f32 {
    let a = p64 as f32 * (1.0 / 64.0);
    let b = c64 as f32 * (1.0 / 64.0);
    (0.5 * ((1.0 - a) * (1.0 - a) + a * a + (1.0 - b) * (1.0 - b) + b * b)).sqrt()
}

/// Raw MAD comparing `cur` vertically blended by `((64 - p64)/64, p64/64)`
/// against `prev` at row offset `d_base` blended by `((64 - c64)/64, c64/64)`
/// (`p64, c64 ∈ 0..64`). Keeping `p64` and `c64` loop-invariant outside the
/// pixel scan lets LLVM auto-vectorize each band slice.
#[allow(clippy::too_many_arguments)]
fn row_cost_bi_phase_64(
    prev: &RowProfile,
    cur: &RowProfile,
    d_base: i32,
    p64: u32,
    c64: u32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    row_sums: &mut Vec<(f32, f32)>,
) -> Option<f32> {
    if p64 == 0 && c64 == 0 {
        return row_cost(prev, cur, d_base, weights, trim_top, trim_bot);
    }
    let h = prev.height.min(cur.height) as i32;
    let stride = cur.stride;
    row_sums.clear();

    if p64 == 0 {
        let (first0, end0) = overlap_window(h, d_base, trim_top, trim_bot);
        let (first1, end1) = overlap_window(h, d_base + 1, trim_top, trim_bot);
        let first = first0.max(first1).div_ceil(cur.step) * cur.step;
        let end = end0.min(end1);
        let inv_c = 64 - c64;
        let mut j = first;
        while j < end {
            let pj = (j as i32 + d_base) as usize;
            let w = cur.row_weight[j].min(prev.row_weight[pj].max(prev.row_weight[pj + 1]));
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
                let p0 = &prev.data[pj * stride + start..pj * stride + stop];
                let p1 = &prev.data[(pj + 1) * stride + start..(pj + 1) * stride + stop];
                for ((&cv, &a), &b) in c.iter().zip(p0.iter()).zip(p1.iter()) {
                    let interp = ((u32::from(a) * inv_c + u32::from(b) * c64 + 32) >> 6) as u8;
                    sum += u32::from(cv.abs_diff(interp));
                }
                width += stop - start;
            }
            if width == 0 {
                let c = &cur.data[j * stride..(j + 1) * stride];
                let p0 = &prev.data[pj * stride..(pj + 1) * stride];
                let p1 = &prev.data[(pj + 1) * stride..(pj + 2) * stride];
                for ((&cv, &a), &b) in c.iter().zip(p0.iter()).zip(p1.iter()) {
                    let interp = ((u32::from(a) * inv_c + u32::from(b) * c64 + 32) >> 6) as u8;
                    sum += u32::from(cv.abs_diff(interp));
                }
                width = stride;
            }
            row_sums.push((sum as f32 / width as f32, w));
            j += cur.step;
        }
        return trimmed_weighted_mean(row_sums);
    }

    let cur_pair_limit = (h - trim_bot - 1).max(0) as usize;
    let inv_p = 64 - p64;
    if c64 == 0 {
        let (first0, end0) = overlap_window(h, d_base, trim_top, trim_bot);
        let first = first0.div_ceil(cur.step) * cur.step;
        let end = end0.min(cur_pair_limit);
        let mut j = first;
        while j < end {
            let pj = (j as i32 + d_base) as usize;
            let w = prev.row_weight[pj].min(cur.row_weight[j].max(cur.row_weight[j + 1]));
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
                let p = &prev.data[pj * stride + start..pj * stride + stop];
                let c0 = &cur.data[j * stride + start..j * stride + stop];
                let c1 = &cur.data[(j + 1) * stride + start..(j + 1) * stride + stop];
                for ((&pv, &a), &b) in p.iter().zip(c0.iter()).zip(c1.iter()) {
                    let interp = ((u32::from(a) * inv_p + u32::from(b) * p64 + 32) >> 6) as u8;
                    sum += u32::from(pv.abs_diff(interp));
                }
                width += stop - start;
            }
            if width == 0 {
                let p = &prev.data[pj * stride..(pj + 1) * stride];
                let c0 = &cur.data[j * stride..(j + 1) * stride];
                let c1 = &cur.data[(j + 1) * stride..(j + 2) * stride];
                for ((&pv, &a), &b) in p.iter().zip(c0.iter()).zip(c1.iter()) {
                    let interp = ((u32::from(a) * inv_p + u32::from(b) * p64 + 32) >> 6) as u8;
                    sum += u32::from(pv.abs_diff(interp));
                }
                width = stride;
            }
            row_sums.push((sum as f32 / width as f32, w));
            j += cur.step;
        }
        return trimmed_weighted_mean(row_sums);
    }

    let (first0, end0) = overlap_window(h, d_base, trim_top, trim_bot);
    let (first1, end1) = overlap_window(h, d_base + 1, trim_top, trim_bot);
    let first = first0.max(first1).div_ceil(cur.step) * cur.step;
    let end = end0.min(end1).min(cur_pair_limit);
    let inv_c = 64 - c64;
    let mut j = first;
    while j < end {
        let pj = (j as i32 + d_base) as usize;
        let w = cur.row_weight[j]
            .max(cur.row_weight[j + 1])
            .min(prev.row_weight[pj].max(prev.row_weight[pj + 1]));
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
            let c0 = &cur.data[j * stride + start..j * stride + stop];
            let c1 = &cur.data[(j + 1) * stride + start..(j + 1) * stride + stop];
            let p0 = &prev.data[pj * stride + start..pj * stride + stop];
            let p1 = &prev.data[(pj + 1) * stride + start..(pj + 1) * stride + stop];
            for (((&ca, &cb), &pa), &pb) in c0.iter().zip(c1.iter()).zip(p0.iter()).zip(p1.iter()) {
                let cv = ((u32::from(ca) * inv_p + u32::from(cb) * p64 + 32) >> 6) as u8;
                let pv = ((u32::from(pa) * inv_c + u32::from(pb) * c64 + 32) >> 6) as u8;
                sum += u32::from(cv.abs_diff(pv));
            }
            width += stop - start;
        }
        if width == 0 {
            let c0 = &cur.data[j * stride..(j + 1) * stride];
            let c1 = &cur.data[(j + 1) * stride..(j + 2) * stride];
            let p0 = &prev.data[pj * stride..(pj + 1) * stride];
            let p1 = &prev.data[(pj + 1) * stride..(pj + 2) * stride];
            for (((&ca, &cb), &pa), &pb) in c0.iter().zip(c1.iter()).zip(p0.iter()).zip(p1.iter()) {
                let cv = ((u32::from(ca) * inv_p + u32::from(cb) * p64 + 32) >> 6) as u8;
                let pv = ((u32::from(pa) * inv_c + u32::from(pb) * c64 + 32) >> 6) as u8;
                sum += u32::from(cv.abs_diff(pv));
            }
            width = stride;
        }
        row_sums.push((sum as f32 / width as f32, w));
        j += cur.step;
    }
    trimmed_weighted_mean(row_sums)
}

/// Sub-row interpolation counterpart of [`row_cost`] at fractional offset
/// `d_floor + frac_eighths / 8.0` (`frac_eighths ∈ 1..8`), returning raw
/// per-phase MADs across all eight eighth-pixel anchor phases `p_phase ∈ 0..8`.
///
/// When `prev` (`stable_profile`) was captured at fractional document phase
/// `α ≈ p_phase / 8` and `cur` moved by `d_floor + q / 8`, `cur` sits at
/// document phase `β = frac(α + q / 8) ≈ c_phase / 8` (`c_phase = (p_phase + q) % 8`,
/// `p_shift = (p_phase + q) / 8`). Because 1D 2-tap filters commute
/// (`h_β * h_α = h_α * h_β`), blending `prev[j + d_floor + p_shift .. +1]` by
/// `c_phase / 8` and `cur[j .. j + 1]` by `p_phase / 8` cancels the 3-row
/// second-difference blur error across arbitrary fractional-to-fractional
/// steps (`0.25 → 0.50`, `0.50 → 0.75`, `0.70 → 0.30`), while reducing to
/// single-sided forward (`p_phase = 0`) and reverse (`p_phase = 8 - q`)
/// interpolation whenever either frame lands on a crisp integer row.
#[allow(clippy::too_many_arguments)]
fn row_cost_subpixel_phases(
    prev: &RowProfile,
    cur: &RowProfile,
    d_floor: i32,
    frac_eighths: u8,
    max_admissible: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    row_sums: &mut Vec<(f32, f32)>,
) -> [Option<f32>; 8] {
    let mut out = [None; 8];
    if frac_eighths == 0 {
        out[0] = row_cost(prev, cur, d_floor, weights, trim_top, trim_bot);
        return out;
    }
    if frac_eighths >= 8 {
        out[0] = row_cost(prev, cur, d_floor + 1, weights, trim_top, trim_bot);
        return out;
    }
    let q64 = usize::from(frac_eighths) * 8;
    for (p_phase, slot) in out.iter_mut().enumerate() {
        let p64 = p_phase * 8;
        let sum = p64 + q64;
        let d_base = d_floor + (sum >> 6) as i32;
        let c64 = sum & 63;
        let in_bounds = d_base >= -max_admissible
            && if c64 == 0 {
                d_base <= max_admissible
            } else {
                d_base < max_admissible
            };
        if in_bounds {
            *slot = row_cost_bi_phase_64(
                prev, cur, d_base, p64 as u32, c64 as u32, weights, trim_top, trim_bot, row_sums,
            );
        }
    }
    out
}

/// Verify the pixel MAD directly at the interpolated continuous displacement
/// `subpixel_dy` (`1/64`-pixel resolution) around `best_p_phase` (`±1/16`
/// anchor-phase neighbourhood) so non-eighth steps (`0.3`, `13.6`, `40/3`) on
/// dense ink get their true verified MAD without unverified extrapolation.
#[allow(clippy::too_many_arguments)]
fn verify_subpixel_64(
    prev: &RowProfile,
    cur: &RowProfile,
    subpixel_dy: f32,
    best_p_phase: usize,
    max_admissible: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    row_sums: &mut Vec<(f32, f32)>,
) -> Option<f32> {
    let d_floor = subpixel_dy.floor() as i32;
    let q64 = ((subpixel_dy - d_floor as f32) * 64.0)
        .round()
        .clamp(0.0, 64.0) as usize;
    if q64 == 0 {
        return (d_floor.abs() <= max_admissible)
            .then(|| row_cost(prev, cur, d_floor, weights, trim_top, trim_bot))
            .flatten();
    }
    if q64 >= 64 {
        let d1 = d_floor + 1;
        return (d1.abs() <= max_admissible)
            .then(|| row_cost(prev, cur, d1, weights, trim_top, trim_bot))
            .flatten();
    }
    let center_p64 = best_p_phase * 8;
    let candidates = [
        center_p64,
        (center_p64 + 60) & 63,
        (center_p64 + 4) & 63,
        0,
        64 - q64,
    ];
    let mut best_norm: Option<f32> = None;
    for (idx, &p64) in candidates.iter().enumerate() {
        if candidates[..idx].contains(&p64) {
            continue;
        }
        let sum = p64 + q64;
        let d_base = d_floor + (sum >> 6) as i32;
        let c64 = sum & 63;
        let in_bounds = d_base >= -max_admissible
            && if c64 == 0 {
                d_base <= max_admissible
            } else {
                d_base < max_admissible
            };
        if !in_bounds {
            continue;
        }
        if let Some(raw) = row_cost_bi_phase_64(
            prev, cur, d_base, p64 as u32, c64 as u32, weights, trim_top, trim_bot, row_sums,
        ) {
            let norm = raw / phase_gain_64(p64, c64);
            if best_norm.is_none_or(|b| norm < b) {
                best_norm = Some(norm);
            }
        }
    }
    best_norm
}

/// Refine a shortlist candidate basin across its suppressed `±2`-row
/// integer neighbourhood and eighth-pixel (`0.125` px) sub-row offsets.
///
/// Stage 3 suppresses offsets within `|other - d| <= 2` so one correlation
/// peak cannot consume multiple shortlist slots; on subpixel-blended text
/// `band_cost` can rank `d` off by 1–2 rows from the true `row_cost` valley,
/// so we walk integer neighbours first to anchor on the true valley floor
/// `best_int_d` before sweeping `[best_int_d - 1, best_int_d + 1]` at
/// eighth-pixel resolution and interpolating the L1 (`|Δ|`) cone vertex
/// along the winning anchor phase.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_basin(
    prev: &RowProfile,
    cur: &RowProfile,
    center_d: i32,
    center_cost: f32,
    max_admissible: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    int_cache: &mut Vec<(i32, Option<f32>)>,
) -> (i32, f32, f32) {
    if center_cost == 0.0 {
        return (center_d, center_d as f32, 0.0);
    }
    let mut eval_int = |d: i32| -> Option<f32> {
        if d.abs() > max_admissible {
            return None;
        }
        if let Some(&(_, cached)) = int_cache.iter().find(|&&(k, _)| k == d) {
            return cached;
        }
        let val = row_cost(prev, cur, d, weights, trim_top, trim_bot);
        int_cache.push((d, val));
        val
    };

    let mut best_int_d = center_d;
    let mut best_int_cost = center_cost;
    for d1 in [center_d - 1, center_d + 1, center_d - 2, center_d + 2] {
        if let Some(c1) = eval_int(d1)
            && (c1 < best_int_cost || (c1 == best_int_cost && d1.abs() < best_int_d.abs()))
        {
            best_int_d = d1;
            best_int_cost = c1;
        }
    }

    if best_int_cost == 0.0 {
        return (best_int_d, best_int_d as f32, 0.0);
    }

    // 17-point eighth-pixel grid over `[best_int_d - 1, best_int_d + 1]`:
    // index `k` sits at `(best_int_d - 1) as f32 + k as f32 * 0.125`.
    let mut grid_phases = [[None; 8]; 17];
    grid_phases[0][0] = eval_int(best_int_d - 1);
    grid_phases[8][0] = Some(best_int_cost);
    grid_phases[16][0] = eval_int(best_int_d + 1);

    let mut min_int_d = best_int_d;
    let mut min_int_cost = best_int_cost;
    for (idx, d) in [(0usize, best_int_d - 1), (16usize, best_int_d + 1)] {
        if let Some(c) = grid_phases[idx][0]
            && (c < min_int_cost || (c == min_int_cost && d.abs() < min_int_d.abs()))
        {
            min_int_d = d;
            min_int_cost = c;
        }
    }
    if min_int_cost == 0.0 {
        return (min_int_d, min_int_d as f32, 0.0);
    }

    let mut row_sums: Vec<(f32, f32)> = Vec::with_capacity(16);
    let mut eval_grid_k = |k: usize, grid: &mut [[Option<f32>; 8]; 17]| {
        if !(1..=15).contains(&k) || k == 8 || grid[k].iter().any(Option::is_some) {
            return;
        }
        let (d_floor, q) = if k < 8 {
            (best_int_d - 1, k as u8)
        } else {
            (best_int_d, (k - 8) as u8)
        };
        let valid_floor = if k < 8 {
            best_int_d > -max_admissible && best_int_d <= max_admissible
        } else {
            best_int_d >= -max_admissible && best_int_d < max_admissible
        };
        if valid_floor {
            grid[k] = row_cost_subpixel_phases(
                prev,
                cur,
                d_floor,
                q,
                max_admissible,
                weights,
                trim_top,
                trim_bot,
                &mut row_sums,
            );
        }
    };

    // Quarter-pixel pass (`k ∈ {2, 4, 6, 10, 12, 14}`), then eighth-pixel
    // neighbours (`even_best_k ± 1`) around the best even grid point across
    // `{0, 2, .., 16}`: every odd `k` sits adjacent to the winning even bracket.
    for k in [2usize, 4, 6, 10, 12, 14] {
        eval_grid_k(k, &mut grid_phases);
    }
    let mut even_best: Option<(usize, f32)> = None;
    for k in (0..=16).step_by(2) {
        let q = k & 7;
        let candidate_dy = (best_int_d - 1) as f32 + (k as f32) * 0.125;
        for (p_phase, &slot) in grid_phases[k].iter().enumerate() {
            if let Some(raw) = slot {
                let c_phase = (p_phase + q) & 7;
                let norm = raw / phase_gain_64(p_phase * 8, c_phase * 8);
                if even_best.is_none_or(|(bk, bc)| {
                    let best_dy = (best_int_d - 1) as f32 + (bk as f32) * 0.125;
                    norm < bc || (norm == bc && candidate_dy.abs() < best_dy.abs())
                }) {
                    even_best = Some((k, norm));
                }
            }
        }
    }
    if let Some((ek, _)) = even_best {
        if ek > 0 {
            eval_grid_k(ek - 1, &mut grid_phases);
        }
        if ek < 16 {
            eval_grid_k(ek + 1, &mut grid_phases);
        }
    }

    // `(best_k, best_p_phase, raw_mad, normalized_cost)`
    let mut best_sub: Option<(usize, usize, f32, f32)> = None;
    for k in (1..=7).chain(9..=15) {
        let q = k & 7;
        let candidate_dy = (best_int_d - 1) as f32 + (k as f32) * 0.125;
        for (p_phase, &slot) in grid_phases[k].iter().enumerate() {
            if let Some(raw) = slot {
                let c_phase = (p_phase + q) & 7;
                let norm = raw / phase_gain_64(p_phase * 8, c_phase * 8);
                if best_sub.is_none_or(|(bk, _, _, bc)| {
                    let best_dy = (best_int_d - 1) as f32 + (bk as f32) * 0.125;
                    norm < bc || (norm == bc && candidate_dy.abs() < best_dy.abs())
                }) {
                    best_sub = Some((k, p_phase, raw, norm));
                }
            }
        }
    }

    let Some((best_k, best_p_phase, c_mid, best_norm_cost)) =
        best_sub.filter(|&(_, _, _, norm)| norm < min_int_cost)
    else {
        return (min_int_d, min_int_d as f32, min_int_cost);
    };

    let mut eval_neighbour_raw = |idx: usize| -> Option<f32> {
        if let Some(cached) = grid_phases[idx][best_p_phase] {
            return Some(cached);
        }
        let d_int = match idx {
            0 => best_int_d - 1,
            8 => best_int_d,
            16 => best_int_d + 1,
            _ => return None,
        };
        let p64 = (best_p_phase * 8) as u32;
        let in_bounds = d_int >= -max_admissible
            && if p64 == 0 {
                d_int <= max_admissible
            } else {
                d_int < max_admissible
            };
        if !in_bounds {
            return None;
        }
        row_cost_bi_phase_64(
            prev,
            cur,
            d_int,
            p64,
            p64,
            weights,
            trim_top,
            trim_bot,
            &mut row_sums,
        )
    };

    let grid_dy = (best_int_d - 1) as f32 + (best_k as f32) * 0.125;
    // MAD forms a piecewise-linear L1 cone `C_0 + S * |x - x_true|` rather than
    // an L2 parabola: holding `best_p_phase` fixed across `best_k ± 1` gives
    // `max(c_left - c_mid, c_right - c_mid) == S * h` (`h = 0.125`) and exact
    // vertex offset `0.5 * h * (c_left - c_right) / (S * h)`. Because `C_0 >= 0`,
    // `|x - x_true| <= h * c_mid / (S * h)` also zeroes out quantization-slope
    // asymmetry whenever `x_true` sits directly on an eighth-pixel grid point
    // (`c_mid == 0`, e.g. `12.5` or `95.5`).
    let delta = if c_mid <= 1e-6 {
        0.0
    } else if let (Some(c_left), Some(c_right)) = (
        eval_neighbour_raw(best_k - 1),
        eval_neighbour_raw(best_k + 1),
    ) {
        let slope_left = c_left - c_mid;
        let slope_right = c_right - c_mid;
        let slope_max = slope_left.max(slope_right);
        if slope_max > 1e-6 && slope_left >= 0.0 && slope_right >= 0.0 {
            let raw_delta = 0.0625 * (c_left - c_right) / slope_max;
            let bound = (0.125 * c_mid / slope_max).min(0.0625);
            raw_delta.clamp(-bound, bound)
        } else {
            0.0
        }
    } else {
        0.0
    };
    let subpixel_dy = (grid_dy + delta).clamp(-max_admissible as f32, max_admissible as f32);
    let verified_cost = if delta.abs() > 1e-4 {
        verify_subpixel_64(
            prev,
            cur,
            subpixel_dy,
            best_p_phase,
            max_admissible,
            weights,
            trim_top,
            trim_bot,
            &mut row_sums,
        )
        .map_or(best_norm_cost, |c| c.min(best_norm_cost))
    } else {
        best_norm_cost
    };
    (subpixel_dy.round() as i32, subpixel_dy, verified_cost)
}
