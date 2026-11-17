//! Match scrolling cards/text over a textured screen-fixed backdrop.
//! A column mask cannot separate two motions that share every column.

use super::{
    BAND_COUNT, BandWeights, MIN_EVIDENCE_ROWS, OffsetEstimate, RowProfile, candidate_offsets,
    overlap_window,
};

/// Minimum active bands required to separate a screen-fixed backdrop from
/// scrolling foreground; mirrors `MotionMask`'s 3-band floor so a narrow
/// 1–2 band strip (scrollbar, chat sidebar, playlist) never masquerades as
/// full-page foreground motion.
const MIN_ACTIVE_BANDS: usize = 3;

/// Maximum full-row (`pixel_step = 1`) candidate verifications per frame.
const MAX_VERIFIED_CANDIDATES: usize = 12;

fn active_band_count(weights: &BandWeights) -> usize {
    weights.iter().filter(|&&w| w > 0.0).count()
}

fn min_band_consensus(active_bands: usize) -> usize {
    active_bands.div_ceil(2).max(MIN_ACTIVE_BANDS)
}

/// RMS amplitude ratio of two 2-tap vertical blends `((64 - p64)/64, p64/64)`
/// and `((64 - c64)/64, c64/64)` relative to an unblended row difference.
///
/// Dividing a bi-phase subpixel foreground MAD by this factor keeps unrelated
/// or periodic runner-up offsets on the same cost scale as [`cost`].
fn phase_gain_64(p64: usize, c64: usize) -> f32 {
    let a = p64 as f32 * (1.0 / 64.0);
    let b = c64 as f32 * (1.0 / 64.0);
    (0.5 * ((1.0 - a) * (1.0 - a) + a * a + (1.0 - b) * (1.0 - b) + b * b)).sqrt()
}

fn consensus_cost(
    rows: &mut [(usize, f32, u8)],
    first: usize,
    end: usize,
    active_bands: usize,
) -> Option<f32> {
    // Check the evidence floor before the 2/3 consensus trim (matching
    // `row_cost`) so HiDPI profiles (`step = 6..8`) do not starve on brisk
    // steps that still satisfy `min_overlap`.
    if rows.len() < MIN_EVIDENCE_ROWS {
        return None;
    }
    // A newly uncovered gap can show the backdrop at both compared
    // positions even though both changed from foreground pixels. Such
    // rows do not translate. Require a two-thirds row consensus, while
    // keeping the original pixel residual threshold for that consensus.
    let keep = rows.len() - rows.len() / 3;
    rows.select_nth_unstable_by(keep - 1, |a, b| a.1.total_cmp(&b.1));
    let retained = &rows[..keep];
    let first_evidence = retained.iter().map(|&(y, _, _)| y).min()?;
    let last_evidence = retained.iter().map(|&(y, _, _)| y).max()?;
    if last_evidence - first_evidence < end.saturating_sub(first) / 2 {
        return None;
    }
    let min_rows_per_band = (keep / 6).max(2);
    let mut band_hits = [0usize; BAND_COUNT];
    for &(_, _, mask) in retained {
        for (band, hits) in band_hits.iter_mut().enumerate() {
            if mask & (1u8 << band) != 0 {
                *hits += 1;
            }
        }
    }
    let moving_bands = band_hits
        .iter()
        .filter(|&&hits| hits >= min_rows_per_band)
        .count();
    if moving_bands < min_band_consensus(active_bands) {
        return None;
    }
    Some(retained.iter().map(|&(_, value, _)| value).sum::<f32>() / keep as f32)
}

/// Require stationary texture, not merely the blank margins of a document.
/// Require an unchanged textured 3×3 patch distributed across both vertical
/// zones and multiple horizontal bands so neither fixed headers/footers nor
/// one-sided split panes look like a full-pane backdrop.
fn has_backdrop(prev: &RowProfile, cur: &RowProfile, weights: &BandWeights) -> bool {
    let active_bands = active_band_count(weights);
    if active_bands < MIN_ACTIVE_BANDS {
        return false;
    }
    let mut sampled = [0usize; 3];
    let mut still = 0usize;
    let mut structured = [0usize; 3];
    let mut band_sampled = [0usize; BAND_COUNT];
    let mut band_structured = [0usize; BAND_COUNT];
    let height = prev.height.min(cur.height) as usize;
    for y in (1..height).step_by(cur.step.max(1) * 4) {
        let zone = (y * 3 / height).min(2);
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            for x in (start.max(1)..stop.min(cur.stride.saturating_sub(1))).step_by(4) {
                let i = y * cur.stride + x;
                let c = cur.data[i];
                sampled[zone] += 1;
                band_sampled[band] += 1;
                if c.abs_diff(prev.data[i]) > 2 {
                    continue;
                }
                still += 1;
                let left = i - 1;
                let above = i - cur.stride;
                let edge = (c.abs_diff(cur.data[left]) > 2
                    && cur.data[left].abs_diff(prev.data[left]) <= 2)
                    || (c.abs_diff(cur.data[above]) > 2
                        && cur.data[above].abs_diff(prev.data[above]) <= 2);
                let patch_still = y + 1 < height
                    && (y - 1..=y + 1).all(|py| {
                        (x - 1..=x + 1).all(|px| {
                            let at = py * cur.stride + px;
                            cur.data[at].abs_diff(prev.data[at]) <= 2
                        })
                    });
                let hit = usize::from(edge && patch_still);
                structured[zone] += hit;
                band_structured[band] += hit;
            }
        }
    }
    let total: usize = sampled.iter().sum();
    let backdrop_bands = (0..BAND_COUNT)
        .filter(|&b| {
            weights[b] > 0.0 && band_sampled[b] > 0 && band_structured[b] * 100 >= band_sampled[b]
        })
        .count();
    total > 0
        && still * 3 >= total
        && structured.iter().sum::<usize>() * 20 >= total
        // A header/footer is not a full-height backdrop, and a stationary
        // side pane is not a full-width backdrop.
        && sampled
            .iter()
            .zip(structured)
            .all(|(&count, edges)| count > 0 && edges * 100 >= count)
        && backdrop_bands >= min_band_consensus(active_bands)
}

/// Whether foreground pixels changed across enough width and horizontal
/// bands at screen position `d = 0` to demand a foreground displacement
/// match (separating genuine scroll / fast flicks from cursor shimmer or a
/// narrow side-strip scroller).
fn has_foreground_motion(
    prev: &RowProfile,
    cur: &RowProfile,
    weights: &BandWeights,
    trim: (i32, i32),
) -> bool {
    const PIXEL_STEP: usize = 4;
    let active_bands = active_band_count(weights);
    if active_bands < MIN_ACTIVE_BANDS {
        return false;
    }
    let mut band_samples = [0usize; BAND_COUNT];
    let mut total_samples = 0usize;
    for (band, &weight) in weights.iter().enumerate() {
        if weight > 0.0 {
            let (start, stop) = cur.bands[band];
            let samples = stop.saturating_sub(start).div_ceil(PIXEL_STEP);
            band_samples[band] = samples;
            total_samples += samples;
        }
    }
    if total_samples == 0 {
        return false;
    }

    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, 0, trim.0, trim.1);
    let step = cur.step.max(1);
    let first = first.div_ceil(step) * step;
    let stride = cur.stride;

    let mut changed_rows = 0usize;
    let mut band_hits = [0usize; BAND_COUNT];
    for y in (first..end).step_by(step) {
        let mut row_changed = 0usize;
        let mut row_mask = 0u8;
        for (band, &samples) in band_samples.iter().enumerate() {
            if samples == 0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            let mut band_changed = 0usize;
            for x in (start..stop).step_by(PIXEL_STEP) {
                let i = y * stride + x;
                if cur.data[i].abs_diff(prev.data[i]) > 2 {
                    band_changed += 1;
                }
            }
            if band_changed * 8 >= samples {
                row_mask |= 1u8 << band;
            }
            row_changed += band_changed;
        }
        if row_mask.count_ones() >= 2
            && row_changed * 8 >= total_samples
            && row_changed * PIXEL_STEP >= 12
        {
            changed_rows += 1;
            for (band, hits) in band_hits.iter_mut().enumerate() {
                if row_mask & (1u8 << band) != 0 {
                    *hits += 1;
                }
            }
        }
    }
    if changed_rows < MIN_EVIDENCE_ROWS {
        return false;
    }
    let min_rows_per_band = (changed_rows / 6).max(2);
    let moving_bands = band_hits
        .iter()
        .filter(|&&hits| hits >= min_rows_per_band)
        .count();
    moving_bands >= min_band_consensus(active_bands)
}

/// Only pixels that changed at BOTH screen positions may testify about
/// foreground motion. The sparse pass locates basins; final verification
/// checks every pixel in the active bands on the normal row grid.
///
/// `d == 0` is intentionally excluded: at zero displacement the `> 2`
/// change filter forces every included pixel to contribute `>= 3`, so `d = 0`
/// can never clear `cost_threshold` and would only distort `min_by` or
/// `runner_up`. Stationary frames exit earlier via [`has_foreground_motion`].
#[allow(clippy::too_many_arguments)]
fn cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    pixel_step: usize,
    rows: &mut Vec<(usize, f32, u8)>,
) -> Option<f32> {
    if d == 0 {
        return None;
    }
    let active_bands = active_band_count(weights);
    if active_bands < MIN_ACTIVE_BANDS {
        return None;
    }
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim.0, trim.1);
    if end.saturating_sub(first) < min_overlap as usize {
        return None;
    }

    let mut band_samples = [0usize; BAND_COUNT];
    let mut total_samples = 0usize;
    for (band, &weight) in weights.iter().enumerate() {
        if weight > 0.0 {
            let (start, stop) = cur.bands[band];
            let samples = stop.saturating_sub(start).div_ceil(pixel_step);
            band_samples[band] = samples;
            total_samples += samples;
        }
    }
    if total_samples == 0 {
        return None;
    }

    let step = cur.step.max(1);
    let first = first.div_ceil(step) * step;
    let stride = cur.stride;
    rows.clear();
    for y in (first..end).step_by(step) {
        let py = (y as i32 + d) as usize;
        let mut sum = 0u32;
        let mut count = 0usize;
        let mut row_mask = 0u8;
        for (band, &samples) in band_samples.iter().enumerate() {
            if samples == 0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            let mut band_moving = 0usize;
            for x in (start..stop).step_by(pixel_step) {
                let c = cur.data[y * stride + x];
                let p = prev.data[py * stride + x];
                if c.abs_diff(prev.data[y * stride + x]) > 2
                    && p.abs_diff(cur.data[py * stride + x]) > 2
                {
                    sum += u32::from(c.abs_diff(p));
                    band_moving += 1;
                }
            }
            if band_moving * 8 >= samples {
                row_mask |= 1u8 << band;
            }
            count += band_moving;
        }
        if row_mask.count_ones() >= 2 && count * 8 >= total_samples && count * pixel_step >= 12 {
            rows.push((y, sum as f32 / count as f32, row_mask));
        }
    }
    consensus_cost(rows, first, end, active_bands)
}

/// Bi-phase sub-row counterpart of [`cost`] comparing `cur` vertically blended
/// by `((64 - p64)/64, p64/64)` against `prev` at row offset `d_base` blended
/// by `((64 - c64)/64, c64/64)` (`p64, c64 ∈ 0..64`).
#[allow(clippy::too_many_arguments)]
fn cost_bi_phase_64(
    prev: &RowProfile,
    cur: &RowProfile,
    d_base: i32,
    p64: u32,
    c64: u32,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    rows: &mut Vec<(usize, f32, u8)>,
) -> Option<f32> {
    if p64 == 0 && c64 == 0 {
        return cost(prev, cur, d_base, weights, trim, min_overlap, 1, rows);
    }
    if d_base == 0 || (d_base == -1 && p64 == 0 && c64 > 0) {
        return None;
    }
    let active_bands = active_band_count(weights);
    if active_bands < MIN_ACTIVE_BANDS {
        return None;
    }
    let h = prev.height.min(cur.height) as i32;
    let (mut first, mut end) = overlap_window(h, d_base, trim.0, trim.1);
    if c64 > 0 {
        let (first1, end1) = overlap_window(h, d_base + 1, trim.0, trim.1);
        first = first.max(first1);
        end = end.min(end1);
    }
    if p64 > 0 {
        let cur_pair_limit = (h - trim.1 - 1).max(0) as usize;
        end = end.min(cur_pair_limit);
    }
    if end.saturating_sub(first) < min_overlap as usize {
        return None;
    }

    let mut band_samples = [0usize; BAND_COUNT];
    let mut total_samples = 0usize;
    for (band, &weight) in weights.iter().enumerate() {
        if weight > 0.0 {
            let (start, stop) = cur.bands[band];
            let samples = stop.saturating_sub(start);
            band_samples[band] = samples;
            total_samples += samples;
        }
    }
    if total_samples == 0 {
        return None;
    }

    let step = cur.step.max(1);
    let first = first.div_ceil(step) * step;
    let stride = cur.stride;
    let inv_p = 64 - p64;
    let inv_c = 64 - c64;
    let p_step = usize::from(p64 > 0);
    let c_step = usize::from(c64 > 0);
    rows.clear();
    for y in (first..end).step_by(step) {
        let py = (y as i32 + d_base) as usize;
        let y1 = y + p_step;
        let py1 = py + c_step;
        let mut sum = 0u32;
        let mut count = 0usize;
        let mut row_mask = 0u8;
        for (band, &samples) in band_samples.iter().enumerate() {
            if samples == 0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            let mut band_moving = 0usize;
            for x in start..stop {
                let c0 = cur.data[y * stride + x];
                let c1 = cur.data[y1 * stride + x];
                let p0 = prev.data[py * stride + x];
                let p1 = prev.data[py1 * stride + x];
                let changed_y = c0.abs_diff(prev.data[y * stride + x]) > 2
                    || (p64 > 0 && c1.abs_diff(prev.data[y1 * stride + x]) > 2);
                let changed_py = p0.abs_diff(cur.data[py * stride + x]) > 2
                    || (c64 > 0 && p1.abs_diff(cur.data[py1 * stride + x]) > 2);
                if changed_y && changed_py {
                    let cv = ((u32::from(c0) * inv_p + u32::from(c1) * p64 + 32) >> 6) as u8;
                    let pv = ((u32::from(p0) * inv_c + u32::from(p1) * c64 + 32) >> 6) as u8;
                    sum += u32::from(cv.abs_diff(pv));
                    band_moving += 1;
                }
            }
            if band_moving * 8 >= samples {
                row_mask |= 1u8 << band;
            }
            count += band_moving;
        }
        if row_mask.count_ones() >= 2 && count * 8 >= total_samples && count >= 12 {
            rows.push((y, sum as f32 / count as f32, row_mask));
        }
    }
    consensus_cost(rows, first, end, active_bands)
}

#[allow(clippy::too_many_arguments)]
fn cost_subpixel_phases(
    prev: &RowProfile,
    cur: &RowProfile,
    d_floor: i32,
    frac_eighths: u8,
    window: i32,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    rows: &mut Vec<(usize, f32, u8)>,
) -> [Option<f32>; 8] {
    let mut out = [None; 8];
    if frac_eighths == 0 {
        out[0] = cost(prev, cur, d_floor, weights, trim, min_overlap, 1, rows);
        return out;
    }
    if frac_eighths >= 8 {
        out[0] = cost(prev, cur, d_floor + 1, weights, trim, min_overlap, 1, rows);
        return out;
    }
    let q64 = usize::from(frac_eighths) * 8;
    for (p_phase, slot) in out.iter_mut().enumerate() {
        let p64 = p_phase * 8;
        let sum = p64 + q64;
        let d_base = d_floor + (sum >> 6) as i32;
        let c64 = sum & 63;
        let in_bounds = d_base != 0
            && d_base >= -window
            && if c64 == 0 {
                d_base <= window
            } else {
                d_base < window
            };
        if in_bounds {
            *slot = cost_bi_phase_64(
                prev,
                cur,
                d_base,
                p64 as u32,
                c64 as u32,
                weights,
                trim,
                min_overlap,
                rows,
            );
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn verify_subpixel_64(
    prev: &RowProfile,
    cur: &RowProfile,
    subpixel_dy: f32,
    best_p_phase: usize,
    window: i32,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    rows: &mut Vec<(usize, f32, u8)>,
) -> Option<f32> {
    let d_floor = subpixel_dy.floor() as i32;
    let q64 = ((subpixel_dy - d_floor as f32) * 64.0)
        .round()
        .clamp(0.0, 64.0) as usize;
    if q64 == 0 {
        return (d_floor != 0 && d_floor.abs() <= window)
            .then(|| cost(prev, cur, d_floor, weights, trim, min_overlap, 1, rows))
            .flatten();
    }
    if q64 >= 64 {
        let d1 = d_floor + 1;
        return (d1 != 0 && d1.abs() <= window)
            .then(|| cost(prev, cur, d1, weights, trim, min_overlap, 1, rows))
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
        let in_bounds = d_base != 0
            && d_base >= -window
            && if c64 == 0 {
                d_base <= window
            } else {
                d_base < window
            };
        if !in_bounds {
            continue;
        }
        if let Some(raw) = cost_bi_phase_64(
            prev,
            cur,
            d_base,
            p64 as u32,
            c64 as u32,
            weights,
            trim,
            min_overlap,
            rows,
        ) {
            let norm = raw / phase_gain_64(p64, c64);
            if best_norm.is_none_or(|b| norm < b) {
                best_norm = Some(norm);
            }
        }
    }
    best_norm
}

#[allow(clippy::too_many_arguments)]
fn refine_foreground_basin(
    prev: &RowProfile,
    cur: &RowProfile,
    center_d: i32,
    center_cost: f32,
    window: i32,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    rows: &mut Vec<(usize, f32, u8)>,
    int_cache: &mut Vec<(i32, Option<f32>)>,
) -> (i32, f32, f32) {
    if center_cost == 0.0 {
        return (center_d, center_d as f32, 0.0);
    }
    let mut best_int_d = center_d;
    let mut best_int_cost = center_cost;
    for d1 in [center_d - 1, center_d + 1, center_d - 2, center_d + 2] {
        if d1 == 0 || d1.abs() > window {
            continue;
        }
        let c1 = if let Some(&(_, cached)) = int_cache.iter().find(|&&(k, _)| k == d1) {
            cached
        } else {
            let val = cost(prev, cur, d1, weights, trim, min_overlap, 1, rows);
            int_cache.push((d1, val));
            val
        };
        if let Some(c1) = c1
            && (c1 < best_int_cost || (c1 == best_int_cost && d1.abs() < best_int_d.abs()))
        {
            best_int_d = d1;
            best_int_cost = c1;
        }
    }

    if best_int_cost == 0.0 {
        return (best_int_d, best_int_d as f32, 0.0);
    }

    let mut grid_phases = [[None; 8]; 17];
    for (idx, d) in [(0usize, best_int_d - 1), (16usize, best_int_d + 1)] {
        if d != 0 && d.abs() <= window {
            grid_phases[idx][0] =
                if let Some(&(_, cached)) = int_cache.iter().find(|&&(k, _)| k == d) {
                    cached
                } else {
                    let val = cost(prev, cur, d, weights, trim, min_overlap, 1, rows);
                    int_cache.push((d, val));
                    val
                };
        }
    }
    grid_phases[8][0] = Some(best_int_cost);

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

    for k in [2usize, 4, 6, 10, 12, 14] {
        let (d_floor, q) = if k < 8 {
            (best_int_d - 1, k as u8)
        } else {
            (best_int_d, (k - 8) as u8)
        };
        let valid_floor = if k < 8 {
            best_int_d > -window && best_int_d <= window
        } else {
            best_int_d >= -window && best_int_d < window
        };
        if valid_floor {
            grid_phases[k] = cost_subpixel_phases(
                prev,
                cur,
                d_floor,
                q,
                window,
                weights,
                trim,
                min_overlap,
                rows,
            );
        }
    }

    let mut even_best: Option<(usize, f32)> = None;
    for k in (0..=16).step_by(2) {
        let q = k & 7;
        let candidate_dy = (best_int_d - 1) as f32 + (k as f32) * 0.125;
        if candidate_dy.round() as i32 == 0 {
            continue;
        }
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
        for k in [ek.saturating_sub(1), (ek + 1).min(16)] {
            if !(1..=15).contains(&k) || k == 8 || grid_phases[k].iter().any(Option::is_some) {
                continue;
            }
            let (d_floor, q) = if k < 8 {
                (best_int_d - 1, k as u8)
            } else {
                (best_int_d, (k - 8) as u8)
            };
            let valid_floor = if k < 8 {
                best_int_d > -window && best_int_d <= window
            } else {
                best_int_d >= -window && best_int_d < window
            };
            if valid_floor {
                grid_phases[k] = cost_subpixel_phases(
                    prev,
                    cur,
                    d_floor,
                    q,
                    window,
                    weights,
                    trim,
                    min_overlap,
                    rows,
                );
            }
        }
    }

    let mut best_sub: Option<(usize, usize, f32, f32)> = None;
    for k in (1..=7).chain(9..=15) {
        let q = k & 7;
        let candidate_dy = (best_int_d - 1) as f32 + (k as f32) * 0.125;
        if candidate_dy.round() as i32 == 0 {
            continue;
        }
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
        let in_bounds = d_int != 0
            && d_int >= -window
            && if p64 == 0 {
                d_int <= window
            } else {
                d_int < window
            };
        if !in_bounds {
            return None;
        }
        cost_bi_phase_64(prev, cur, d_int, p64, p64, weights, trim, min_overlap, rows)
    };

    let grid_dy = (best_int_d - 1) as f32 + (best_k as f32) * 0.125;
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
    let subpixel_dy = (grid_dy + delta).clamp(-window as f32, window as f32);
    let r_dy = subpixel_dy.round() as i32;
    if r_dy == 0 {
        return (min_int_d, min_int_d as f32, min_int_cost);
    }
    let verified_cost = if delta.abs() > 1e-4 {
        verify_subpixel_64(
            prev,
            cur,
            subpixel_dy,
            best_p_phase,
            window,
            weights,
            trim,
            min_overlap,
            rows,
        )
        .map_or(best_norm_cost, |c| c.min(best_norm_cost))
    } else {
        best_norm_cost
    };
    (r_dy, subpixel_dy, verified_cost)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn estimate(
    prev: &RowProfile,
    cur: &RowProfile,
    weights: &BandWeights,
    trim: (i32, i32),
    min_overlap: i32,
    prediction: i32,
    window: i32,
    latched_backdrop: bool,
) -> Option<OffsetEstimate> {
    if !latched_backdrop && !has_backdrop(prev, cur, weights) {
        return None;
    }
    if !has_foreground_motion(prev, cur, weights, trim) {
        return None;
    }

    let h = prev.height.min(cur.height) as usize;
    let mut rows = Vec::with_capacity(h.div_ceil(cur.step.max(1)));
    let mut candidates = candidate_offsets(window, prediction);

    // Background-contaminated row means can exclude the true foreground
    // offset entirely, so rank a sparse foreground sweep first.
    let mut ranked: Vec<(i32, f32)> = candidates
        .iter()
        .copied()
        .filter(|&d| d != 0)
        .filter_map(|d| {
            cost(prev, cur, d, weights, trim, min_overlap, 8, &mut rows).map(|v| (d, v))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    let mut basins: Vec<i32> = Vec::with_capacity(3);
    for &(d, _) in &ranked {
        if basins.len() == 3 {
            break;
        }
        if basins.iter().all(|&other| (other - d).abs() > 6) {
            basins.push(d);
        }
    }
    // Refine the three best basins at ±3 rows in the sparse pass before
    // choosing the capped full-row verification shortlist.
    for basin in basins {
        for d in (basin - 3).max(-window)..=(basin + 3).min(window) {
            if d != 0 && !candidates.contains(&d) {
                candidates.push(d);
                if let Some(value) = cost(prev, cur, d, weights, trim, min_overlap, 8, &mut rows) {
                    ranked.push((d, value));
                }
            }
        }
    }
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    let mut shortlist: Vec<i32> = Vec::with_capacity(MAX_VERIFIED_CANDIDATES);
    if let Some(&(best_d, _)) = ranked.first() {
        shortlist.push(best_d);
        for neighbour in [best_d - 1, best_d + 1] {
            if neighbour != 0
                && neighbour.abs() <= window
                && shortlist.len() < MAX_VERIFIED_CANDIDATES
                && !shortlist.contains(&neighbour)
            {
                shortlist.push(neighbour);
            }
        }
    }
    for &(d, _) in &ranked {
        if shortlist.len() >= MAX_VERIFIED_CANDIDATES {
            break;
        }
        if shortlist.iter().all(|&other| (other - d).abs() > 2) {
            shortlist.push(d);
        }
    }

    let mut int_evaluated: Vec<(i32, f32)> = Vec::with_capacity(shortlist.len());
    let mut int_cache: Vec<(i32, Option<f32>)> = Vec::with_capacity(shortlist.len() + 16);
    for d in shortlist {
        let c = cost(prev, cur, d, weights, trim, min_overlap, 1, &mut rows);
        int_cache.push((d, c));
        if let Some(c) = c {
            int_evaluated.push((d, c));
        }
    }
    int_evaluated.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    if int_evaluated.is_empty() {
        // Foreground changed across multiple bands over an active backdrop,
        // yet no admissible offset retained enough overlapping foreground
        // rows: surface a window-edge refusal (`Refusal::NoOverlap`) rather
        // than falling back to ordinary `d = 0` `Still`.
        let edge = if prediction < 0 { -window } else { window };
        return Some(OffsetEstimate {
            dy: edge,
            subpixel_dy: edge as f32,
            cost: f32::INFINITY,
            runner_up: f32::INFINITY,
            window,
            foreground: true,
        });
    }

    // Refine the top candidate basins (and the best non-neighbour runner-up
    // basin) at subpixel resolution before picking `best` and `runner_up` so
    // fractional foreground scrolling recovers continuous `subpixel_dy` while
    // periodic foreground stripes still fail `ambiguity_margin`.
    let mut evaluated: Vec<(i32, f32, f32, bool)> = Vec::with_capacity(int_evaluated.len());
    let mut refined_count = 0usize;
    let mut min_refined_cost = f32::INFINITY;
    for &(d, c) in &int_evaluated {
        let near_refined = evaluated
            .iter()
            .any(|&(rd, _, _, is_ref)| is_ref && (rd - d).abs() <= 2);
        let limit = if min_refined_cost <= 14.0 { 2 } else { 4 };
        if refined_count < limit && !near_refined {
            let (r_dy, r_sub, r_cost) = refine_foreground_basin(
                prev,
                cur,
                d,
                c,
                window,
                weights,
                trim,
                min_overlap,
                &mut rows,
                &mut int_cache,
            );
            min_refined_cost = min_refined_cost.min(r_cost);
            evaluated.push((r_dy, r_sub, r_cost, true));
            refined_count += 1;
        } else {
            evaluated.push((d, d as f32, c, false));
        }
    }

    let best_entry = |items: &[(i32, f32, f32, bool)]| {
        items.iter().copied().min_by(|a, b| {
            a.2.total_cmp(&b.2)
                .then_with(|| a.1.abs().total_cmp(&b.1.abs()))
        })
    };

    if let Some((mut best_dy, mut best_sub, mut best_cost, _)) = best_entry(&evaluated)
        && best_cost <= 20.0
    {
        loop {
            let already_ambiguous = evaluated.iter().any(|&(d, sub, c, is_ref)| {
                is_ref
                    && (d - best_dy).abs() > 2
                    && (sub - best_sub).abs() > 2.0
                    && c - best_cost < 1.5
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
                .and_then(|(idx, &(_, _, _, is_ref))| (!is_ref).then_some(idx));
            let Some(idx) = next_unrefined else {
                break;
            };
            let (d, _, c, _) = evaluated[idx];
            let (r_dy, r_sub, r_cost) = refine_foreground_basin(
                prev,
                cur,
                d,
                c,
                window,
                weights,
                trim,
                min_overlap,
                &mut rows,
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

    let (best_dy, best_sub, best_cost, _) = best_entry(&evaluated)?;
    let runner_up = evaluated
        .iter()
        .filter(|(d, sub, _, _)| (*d - best_dy).abs() > 2 && (*sub - best_sub).abs() > 2.0)
        .map(|(_, _, score, _)| *score)
        .fold(f32::INFINITY, f32::min);
    Some(OffsetEstimate {
        dy: best_dy,
        subpixel_dy: best_sub,
        cost: best_cost,
        runner_up,
        window,
        foreground: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::scroll_sheet::{Axis, Fold, ScrollSheet, SheetLimits};

    fn ink_hash(x: usize, y: usize) -> u32 {
        let mut h = (x as u32)
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add((y as u32).wrapping_mul(0x85EB_CA6B));
        h ^= h >> 16;
        h = h.wrapping_mul(0x7FEB_352D);
        h ^ (h >> 15)
    }

    fn backdrop_luma(x: usize, y: usize) -> u8 {
        15 + (ink_hash(x, y) % 55) as u8
    }

    fn foreground_card_sample(x: usize, doc_y: isize, periodic: bool) -> Option<u8> {
        let y = if periodic {
            doc_y.rem_euclid(96) as usize
        } else {
            doc_y.max(0) as usize
        };
        let line = y % 96;
        if (40..148).contains(&x) && (8..76).contains(&line) {
            Some(96 + (ink_hash(x, y.wrapping_mul(317)) % 148) as u8)
        } else if (164..300).contains(&x)
            && (16..36).contains(&line)
            && !(x / 3 + line / 3).is_multiple_of(4)
        {
            Some(120 + (ink_hash(x / 2, y) % 125) as u8)
        } else {
            None
        }
    }

    fn render_foreground_frame(
        width: usize,
        height: usize,
        scroll_y: f32,
        periodic: bool,
    ) -> Vec<u8> {
        let y_floor = scroll_y.floor() as isize;
        let frac = scroll_y - scroll_y.floor();
        let mut rgba = vec![255u8; width * height * 4];
        for y in 0..height {
            let doc0 = y as isize + y_floor;
            let doc1 = doc0 + 1;
            for x in 0..width {
                let bg = backdrop_luma(x, y);
                let fg0 = foreground_card_sample(x, doc0, periodic);
                let fg1 = foreground_card_sample(x, doc1, periodic);
                let v = match (fg0, fg1) {
                    (Some(a), Some(b)) => {
                        (f32::from(a) * (1.0 - frac) + f32::from(b) * frac).round() as u8
                    }
                    (Some(a), None) if frac == 0.0 => a,
                    _ => bg,
                };
                let idx = (y * width + x) * 4;
                rgba[idx] = v;
                rgba[idx + 1] = v;
                rgba[idx + 2] = v;
            }
        }
        rgba
    }

    #[test]
    fn fractional_foreground_scroll_over_fixed_backdrop_recovers_subpixel_dy() {
        let width = 320usize;
        let height = 400usize;
        let weights = [1.0; BAND_COUNT];
        let base_frame = render_foreground_frame(width, height, 0.0, false);
        let prev = RowProfile::from_rgba(&base_frame, width as u32, height as u32);

        for &true_shift in &[12.5f32, 15.375f32] {
            let moved_frame = render_foreground_frame(width, height, true_shift, false);
            let cur = RowProfile::from_rgba(&moved_frame, width as u32, height as u32);
            let est = estimate(&prev, &cur, &weights, (24, 8), 80, 0, 160, false)
                .expect("fractional foreground scroll should produce an estimate");
            assert!(est.foreground);
            assert!(
                (est.subpixel_dy - true_shift).abs() <= 0.15,
                "expected subpixel_dy near {true_shift}, got {} (dy={}, cost={})",
                est.subpixel_dy,
                est.dy,
                est.cost
            );
            assert_eq!(est.dy, est.subpixel_dy.round() as i32);
            assert!(
                est.cost < 5.0,
                "expected low refined cost, got {}",
                est.cost
            );
            assert!(
                est.runner_up - est.cost >= 1.5,
                "expected clear ambiguity margin, got runner_up={} cost={}",
                est.runner_up,
                est.cost
            );
        }

        let mut sheet = ScrollSheet::new_internal(
            width as u32,
            height as u32,
            &base_frame,
            SheetLimits::default(),
            Axis::Vertical,
            false,
        )
        .expect("valid seed");
        for step in 1..=4 {
            let frame = render_foreground_frame(width, height, step as f32 * 12.5, false);
            let fold = sheet.push(&frame);
            assert!(
                matches!(fold, Fold::GrewBottom { .. }),
                "step {step} should grow bottom, got {fold:?}"
            );
        }
        assert_eq!(sheet.height(), height as u32 + 50);
    }

    #[test]
    fn periodic_foreground_stripes_over_fixed_backdrop_remain_ambiguous() {
        let width = 320usize;
        let height = 400usize;
        let weights = [1.0; BAND_COUNT];
        let f0 = render_foreground_frame(width, height, 0.0, true);
        let f1 = render_foreground_frame(width, height, 20.5, true);
        let prev = RowProfile::from_rgba(&f0, width as u32, height as u32);
        let cur = RowProfile::from_rgba(&f1, width as u32, height as u32);
        let est = estimate(&prev, &cur, &weights, (24, 8), 80, 0, 160, false)
            .expect("periodic foreground should produce an estimate");
        assert!(
            est.runner_up - est.cost < 1.5,
            "periodic foreground aliases must both refine and fail ambiguity_margin: cost={}, runner_up={}",
            est.cost,
            est.runner_up
        );
    }
}
