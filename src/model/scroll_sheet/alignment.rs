//! Bounded horizontal correction against the last accepted document position.
//! Only a coherent translation with observed empty side margins can be written;
//! reflow, clipped content and independently moving regions retain the canvas.

use super::{Axis, Placement, Refusal, RowProfile, ScrollSheet};

const MAX_SHIFT_PIXELS: i32 = 16;
const MIN_DETAIL_SAMPLES: usize = 8;
const DETAIL_CONTRAST: u8 = 12;
const REGIONS: usize = 3;

pub(super) struct AlignedFrame {
    pub(super) pixels: Vec<u8>,
    pub(super) profile: RowProfile,
    pub(super) offset: i32,
}

impl ScrollSheet {
    /// Recover a small lateral translation without advancing the stable anchor.
    /// Ambiguous candidates and partial layout matches never authorize a write.
    pub(super) fn recover_horizontal(
        &self,
        frame: &[u8],
        document_motion: Option<i32>,
    ) -> Result<Option<AlignedFrame>, Refusal> {
        if self.axis != Axis::Vertical {
            return Ok(None);
        }
        let mut best: Option<(f32, AlignedFrame)> = None;
        let mut runner_up = f32::INFINITY;
        let mut layout_changed = false;
        for offset in -MAX_SHIFT_PIXELS..=MAX_SHIFT_PIXELS {
            if offset == self.horizontal_offset {
                continue;
            }
            // The profile may diagnose a lateral move even if the full frame
            // cannot be corrected without inventing pixels at its boundary.
            let pixels = shift_pixels(frame, self.width as usize, offset);
            let profile = RowProfile::from_rgba(&pixels, self.width, self.viewport_height);
            let (dy, cost) = match self.classify(&self.stable_profile, &profile, document_motion) {
                Placement::Match(estimate) => (estimate.dy, estimate.cost),
                Placement::Still => (0, 0.0),
                Placement::Reject(_) => continue,
            };
            let support = translation_support(
                &self.stable_profile,
                &profile,
                dy,
                self.options.cost_threshold,
            );
            if !support.distributed(2) {
                continue;
            }
            layout_changed = true;
            if !support.distributed(REGIONS as u32)
                || support.mismatched != 0
                || !has_empty_margins(frame, self.width as usize, offset)
                || self.sidebar.is_some()
            {
                continue;
            }
            let candidate = AlignedFrame {
                pixels,
                profile,
                offset,
            };
            if best.as_ref().is_none_or(|(previous, _)| cost < *previous) {
                if let Some((previous, _)) = best.replace((cost, candidate)) {
                    runner_up = runner_up.min(previous);
                }
            } else {
                runner_up = runner_up.min(cost);
            }
        }
        if let Some((cost, candidate)) = best
            && runner_up - cost >= self.options.ambiguity_margin
        {
            return Ok(Some(candidate));
        }
        if layout_changed {
            Err(Refusal::LayoutShift)
        } else {
            Ok(None)
        }
    }
}

/// Reuse a confirmed offset only while its side padding remains observable.
pub(super) fn normalize(frame: &[u8], width: usize, offset: i32) -> Option<Vec<u8>> {
    has_empty_margins(frame, width, offset).then(|| shift_pixels(frame, width, offset))
}

fn has_empty_margins(frame: &[u8], width: usize, offset: i32) -> bool {
    let padding = offset.unsigned_abs() as usize + 1;
    if padding >= width / 2 {
        return false;
    }
    frame.chunks_exact(width * 4).all(|row| {
        row[..padding * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel == &row[..4])
            && row[(width - padding) * 4..]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &row[(width - 1) * 4..])
    })
}

fn shift_pixels(frame: &[u8], width: usize, offset: i32) -> Vec<u8> {
    let mut pixels = vec![0; frame.len()];
    for (source, destination) in frame
        .chunks_exact(width * 4)
        .zip(pixels.chunks_exact_mut(width * 4))
    {
        let padding = (offset.unsigned_abs() as usize).min(width) * 4;
        let retained = source.len() - padding;
        if offset >= 0 {
            destination[..retained].copy_from_slice(&source[padding..]);
            for pixel in destination[retained..].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&source[source.len() - 4..]);
            }
        } else {
            destination[padding..].copy_from_slice(&source[..retained]);
            for pixel in destination[..padding].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&source[..4]);
            }
        }
    }
    pixels
}

struct TranslationSupport {
    rows: u8,
    columns: u8,
    mismatched: usize,
}

impl TranslationSupport {
    fn distributed(&self, rows: u32) -> bool {
        self.rows.count_ones() >= rows && self.columns.count_ones() >= 2
    }
}

/// Every detailed region must independently explain the same translation.
/// Flat pixels do not vote; a large blank margin cannot hide mismatched text.
fn translation_support(
    base: &RowProfile,
    current: &RowProfile,
    dy: i32,
    tolerance: f32,
) -> TranslationSupport {
    let mut support = TranslationSupport {
        rows: 0,
        columns: 0,
        mismatched: 0,
    };
    let first = (-dy).max(0) as usize;
    let end = (current.height as i32 - dy.max(0)).max(0) as usize;
    if end <= first || base.stride != current.stride {
        return support;
    }
    let width = current.stride;
    let mut differences = [0u64; REGIONS * REGIONS];
    let mut counts = [0usize; REGIONS * REGIONS];
    for y in (first..end).step_by(current.step.max(1)) {
        let previous = (y as i32 + dy) as usize;
        for x in (1..width).step_by(2) {
            let a = current.data[y * width + x];
            let b = base.data[previous * width + x];
            if a.abs_diff(current.data[y * width + x - 1]) < DETAIL_CONTRAST
                && b.abs_diff(base.data[previous * width + x - 1]) < DETAIL_CONTRAST
            {
                continue;
            }
            let region = ((y - first) * REGIONS / (end - first)) * REGIONS + x * REGIONS / width;
            differences[region] += u64::from(a.abs_diff(b));
            counts[region] += 1;
        }
    }
    for (region, (&count, difference)) in counts.iter().zip(differences).enumerate() {
        if count < MIN_DETAIL_SAMPLES {
            continue;
        }
        if difference as f32 / count as f32 <= tolerance {
            support.rows |= 1 << (region / REGIONS);
            support.columns |= 1 << (region % REGIONS);
        } else {
            support.mismatched += 1;
        }
    }
    support
}
