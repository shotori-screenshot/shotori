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

    let verified: Vec<(i32, f32)> = shortlist
        .into_iter()
        .filter_map(|d| {
            cost(prev, cur, d, weights, trim, min_overlap, 1, &mut rows).map(|v| (d, v))
        })
        .collect();

    let Some((dy, score)) = verified
        .iter()
        .copied()
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())))
    else {
        // Foreground changed across multiple bands over an active backdrop,
        // yet no admissible offset retained enough overlapping foreground
        // rows: surface a window-edge refusal (`Refusal::NoOverlap`) rather
        // than falling back to ordinary `d = 0` `Still`.
        let edge = if prediction < 0 { -window } else { window };
        return Some(OffsetEstimate {
            dy: edge,
            cost: f32::INFINITY,
            runner_up: f32::INFINITY,
            window,
            foreground: true,
        });
    };
    let runner_up = verified
        .iter()
        .filter(|(d, _)| (d - dy).abs() > 2)
        .map(|(_, score)| *score)
        .fold(f32::INFINITY, f32::min);
    Some(OffsetEstimate {
        dy,
        cost: score,
        runner_up,
        window,
        foreground: true,
    })
}
