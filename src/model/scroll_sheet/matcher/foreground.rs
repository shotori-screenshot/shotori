//! Match scrolling cards/text over a textured screen-fixed backdrop.
//! A column mask cannot separate two motions that share every column.

use super::{
    BandWeights, MIN_EVIDENCE_ROWS, OffsetEstimate, RowProfile, candidate_offsets, overlap_window,
};

/// Require stationary texture, not merely the blank margins of a document.
/// Require an unchanged textured 3×3 patch. Coincidentally matching ink
/// pixels are common in repeated text, but do not make a fixed backdrop.
fn has_backdrop(prev: &RowProfile, cur: &RowProfile, weights: &BandWeights) -> bool {
    let mut sampled = [0; 3];
    let mut still = 0;
    let mut structured = [0; 3];
    let height = prev.height.min(cur.height) as usize;
    for y in (1..height).step_by(cur.step * 4) {
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
                structured[zone] += usize::from(edge && patch_still);
            }
        }
    }
    let total: usize = sampled.iter().sum();
    total > 0
        && still * 3 >= total
        && structured.iter().sum::<usize>() * 20 >= total
        // A header/footer is not a full-height backdrop. Require texture
        // throughout the view so ordinary fixed chrome keeps its own path.
        && sampled.iter().zip(structured).all(|(&count, edges)| count > 0 && edges * 100 >= count)
}

/// Only pixels that changed at BOTH screen positions may testify about
/// foreground motion. The sparse pass locates basins; final verification
/// checks every pixel in the active bands on the normal row grid.
fn cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim: (i32, i32),
    pixel_step: usize,
) -> Option<f32> {
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim.0, trim.1);
    let first = first.div_ceil(cur.step) * cur.step;
    let stride = cur.stride;
    let mut rows = Vec::new();
    for y in (first..end).step_by(cur.step) {
        let py = (y as i32 + d) as usize;
        let mut sum = 0u32;
        let mut count = 0usize;
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            for x in (start..stop).step_by(pixel_step) {
                let c = cur.data[y * stride + x];
                let p = prev.data[py * stride + x];
                if c.abs_diff(prev.data[y * stride + x]) > 2
                    && p.abs_diff(cur.data[py * stride + x]) > 2
                {
                    sum += u32::from(c.abs_diff(p));
                    count += 1;
                }
            }
        }
        if count >= 2 && count * pixel_step >= 8 {
            rows.push((y, sum as f32 / count as f32));
        }
    }
    if rows.len() < MIN_EVIDENCE_ROWS {
        return None;
    }
    // A newly uncovered gap can show the backdrop at both compared
    // positions even though both changed from foreground pixels. Such
    // rows do not translate. Require a two-thirds row consensus, while
    // keeping the original pixel residual threshold for that consensus.
    let keep = rows.len() - rows.len() / 3;
    if keep < MIN_EVIDENCE_ROWS {
        return None;
    }
    rows.select_nth_unstable_by(keep - 1, |a, b| a.1.total_cmp(&b.1));
    let retained = &rows[..keep];
    let first_evidence = retained.iter().map(|&(y, _)| y).min()?;
    let last_evidence = retained.iter().map(|&(y, _)| y).max()?;
    if last_evidence - first_evidence < end.saturating_sub(first) / 2 {
        return None;
    }
    Some(retained.iter().map(|&(_, value)| value).sum::<f32>() / keep as f32)
}

pub(super) fn estimate(
    prev: &RowProfile,
    cur: &RowProfile,
    weights: &BandWeights,
    trim: (i32, i32),
    prediction: i32,
    window: i32,
) -> Option<OffsetEstimate> {
    if !has_backdrop(prev, cur, weights) {
        return None;
    }
    // Background-contaminated row means can exclude the true foreground
    // offset entirely, so this retry must rank its own complete sweep.
    let mut ranked: Vec<(i32, f32)> = candidate_offsets(window, prediction)
        .into_iter()
        .filter_map(|d| cost(prev, cur, d, weights, trim, 8).map(|value| (d, value)))
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));
    let mut basins: Vec<i32> = Vec::new();
    for &(d, _) in &ranked {
        if basins.len() == 3 {
            break;
        }
        if basins.iter().all(|&other| (other - d).abs() > 6) {
            basins.push(d);
        }
    }
    let mut shortlist: Vec<i32> = Vec::new();
    for &(d, _) in &ranked {
        if shortlist.len() == 12 {
            break;
        }
        if shortlist.iter().all(|&other| (other - d).abs() > 2) {
            shortlist.push(d);
        }
    }
    for basin in basins {
        for d in (basin - 3).max(-window)..=(basin + 3).min(window) {
            if !shortlist.contains(&d) {
                shortlist.push(d);
            }
        }
    }
    if !shortlist.contains(&0) {
        shortlist.push(0);
    }
    let verified: Vec<(i32, f32)> = shortlist
        .into_iter()
        .filter_map(|d| cost(prev, cur, d, weights, trim, 1).map(|value| (d, value)))
        .collect();
    let (dy, score) = verified
        .iter()
        .copied()
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
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
