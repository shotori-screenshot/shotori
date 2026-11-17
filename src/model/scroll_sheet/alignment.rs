//! Bounded horizontal correction against the last accepted document position.
//! Only a coherent translation with observed empty side margins can be written;
//! reflow, clipped content and independently moving regions retain the canvas.

use super::{Axis, Placement, Refusal, RowProfile, ScrollSheet};

const MAX_SHIFT_PIXELS: i32 = 16;
const MIN_DETAIL_SAMPLES: usize = 8;
const DETAIL_CONTRAST: u8 = 12;
const REGIONS: usize = 3;
/// Per-channel RGB tolerance for flat side margins, matching `STILL_TOLERANCE`
/// in `floating.rs` so 1–2 LSB compositor dithering or color-management noise
/// does not block horizontal recovery.
const MARGIN_TOLERANCE: u8 = 3;

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
        if self.axis != Axis::Vertical || self.fixed_backdrop {
            return Ok(None);
        }
        let trim = (self.sticky.top, self.sticky.bot);
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
            let (subpixel_dy, cost) =
                match self.classify_with(&self.stable_profile, &profile, document_motion, false) {
                    Placement::Match(estimate) => (estimate.subpixel_dy, estimate.cost),
                    Placement::Still => (0.0, 0.0),
                    Placement::Reject(_) => continue,
                };
            let support = translation_support(
                &self.stable_profile,
                &profile,
                subpixel_dy,
                trim,
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

#[inline]
fn margin_pixel_matches(pixel: &[u8; 4], anchor: &[u8; 4]) -> bool {
    pixel[..3]
        .iter()
        .zip(&anchor[..3])
        .all(|(a, b)| a.abs_diff(*b) <= MARGIN_TOLERANCE)
        && pixel[3] == anchor[3]
}

/// Verify that `shift_pixels(frame, width, offset)` neither clips content on
/// the discarded side (`|offset| + 1` uniform margin pixels) nor stretches
/// content touching the extended side (at least `2` uniform margin pixels).
fn has_empty_margins(frame: &[u8], width: usize, offset: i32) -> bool {
    if width < 4 {
        return false;
    }
    if offset == 0 {
        return true;
    }
    let k = offset.unsigned_abs() as usize;
    let (left_pad, right_pad) = if offset > 0 { (k + 1, 2) } else { (2, k + 1) };
    if left_pad.max(right_pad) >= width / 2 {
        return false;
    }
    frame.chunks_exact(width * 4).all(|row| {
        let pixels = row.as_chunks::<4>().0;
        let left_anchor = &pixels[0];
        let right_anchor = &pixels[width - 1];
        pixels[..left_pad]
            .iter()
            .all(|pixel| margin_pixel_matches(pixel, left_anchor))
            && pixels[width - right_pad..]
                .iter()
                .all(|pixel| margin_pixel_matches(pixel, right_anchor))
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

#[inline]
fn blend_luma(b0: u8, b1: u8, frac: f32) -> u8 {
    ((1.0 - frac) * f32::from(b0) + frac * f32::from(b1)).round() as u8
}

/// Every detailed region inside the non-sticky content window must
/// independently explain the same continuous vertical translation.
/// Flat pixels do not vote; a large blank margin cannot hide mismatched text.
fn translation_support(
    base: &RowProfile,
    current: &RowProfile,
    subpixel_dy: f32,
    trim: (u32, u32),
    tolerance: f32,
) -> TranslationSupport {
    let mut support = TranslationSupport {
        rows: 0,
        columns: 0,
        mismatched: 0,
    };
    let height = (current.height.min(base.height)) as usize;
    let top = trim.0 as usize;
    let bot = height.saturating_sub(trim.1 as usize);
    if bot <= top
        || base.stride != current.stride
        || base.height != current.height
        || !subpixel_dy.is_finite()
    {
        return support;
    }
    let min_y = (f64::from(top as u32) - f64::from(subpixel_dy))
        .ceil()
        .max(top as f64);
    let max_y = (f64::from((bot - 1) as u32) - f64::from(subpixel_dy))
        .floor()
        .min((bot - 1) as f64);
    if max_y < min_y {
        return support;
    }
    let first = min_y as usize;
    let end = (max_y as usize) + 1;
    let width = current.stride;
    let mut differences = [0u64; REGIONS * REGIONS];
    let mut counts = [0usize; REGIONS * REGIONS];
    for y in (first..end).step_by(current.step.max(1)) {
        let src_y = y as f32 + subpixel_dy;
        if src_y < top as f32 || src_y > (bot - 1) as f32 {
            continue;
        }
        let s0 = src_y.floor() as usize;
        let s1 = (s0 + 1).min(bot - 1);
        let frac = src_y - s0 as f32;
        for x in (1..width).step_by(2) {
            let a = current.data[y * width + x];
            let b = blend_luma(base.data[s0 * width + x], base.data[s1 * width + x], frac);
            let b_left = blend_luma(
                base.data[s0 * width + x - 1],
                base.data[s1 * width + x - 1],
                frac,
            );
            if a.abs_diff(current.data[y * width + x - 1]) < DETAIL_CONTRAST
                && b.abs_diff(b_left) < DETAIL_CONTRAST
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

#[cfg(test)]
mod tests {
    use super::{has_empty_margins, normalize};
    use crate::model::scroll_sheet::{Fold, ScrollSheet, SheetLimits};

    fn make_row_frame(
        width: usize,
        height: usize,
        left_margin: usize,
        right_margin: usize,
    ) -> Vec<u8> {
        let mut frame = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                let px = if x < left_margin || x >= width - right_margin {
                    [244, 246, 248, 255]
                } else {
                    let v = ((x * 31 + y * 17) % 180) as u8;
                    [v, v.wrapping_add(20), v.wrapping_add(40), 255]
                };
                frame[idx..idx + 4].copy_from_slice(&px);
            }
        }
        frame
    }

    #[test]
    fn asymmetric_margins_and_compositor_dither_pass_directional_checks() {
        let width = 80;
        let height = 16;

        // `offset = +6`: discards 6 left columns (`left_pad = 7`) and extends the
        // right boundary (`right_pad = 2`). A 20px left margin with only a 3px
        // right margin must succeed.
        let mut asym_right = make_row_frame(width, height, 20, 3);
        assert!(has_empty_margins(&asym_right, width, 6));
        assert!(normalize(&asym_right, width, 6).is_some());

        // Inject ±2 LSB compositor dithering relative to the margin anchor:
        // must still pass `MARGIN_TOLERANCE` (3).
        for y in 0..height {
            for x in (1..20).chain(width - 3..width - 1) {
                let idx = (y * width + x) * 4;
                let delta = ((x + y) % 5) as i16 - 2; // -2..=+2 around [244, 246, 248]
                asym_right[idx] = (244 + delta) as u8;
                asym_right[idx + 1] = (246 - delta) as u8;
                asym_right[idx + 2] = (248 + delta / 2) as u8;
            }
        }
        assert!(has_empty_margins(&asym_right, width, 6));
        assert!(normalize(&asym_right, width, 6).is_some());

        // Exceeding `MARGIN_TOLERANCE` (> 3 LSB) in the discarded margin must fail.
        let mut noisy = make_row_frame(width, height, 20, 3);
        noisy[2 * 4] = 244 - 5;
        assert!(!has_empty_margins(&noisy, width, 6));

        // Content at `x = 3` on the discarded left (`< left_pad = 7`) must fail.
        let mut clipped_left = make_row_frame(width, height, 20, 3);
        clipped_left[3 * 4..3 * 4 + 4].copy_from_slice(&[20, 20, 20, 255]);
        assert!(!has_empty_margins(&clipped_left, width, 6));
        assert!(normalize(&clipped_left, width, 6).is_none());

        // Content touching `x = width - 1` on the extended right must fail.
        let mut touching_right = make_row_frame(width, height, 20, 3);
        let right_edge = (width - 1) * 4;
        touching_right[right_edge..right_edge + 4].copy_from_slice(&[20, 20, 20, 255]);
        assert!(!has_empty_margins(&touching_right, width, 6));
        assert!(normalize(&touching_right, width, 6).is_none());

        // Mirror direction (`offset = -6`): 3px left margin + 20px right margin succeeds.
        let asym_left = make_row_frame(width, height, 3, 20);
        assert!(has_empty_margins(&asym_left, width, -6));
        assert!(normalize(&asym_left, width, -6).is_some());

        // `offset == 0` is an identity copy when `width >= 4`.
        let edge_to_edge = make_row_frame(width, height, 0, 0);
        assert!(has_empty_margins(&edge_to_edge, width, 0));
        assert!(!has_empty_margins(&edge_to_edge[..12], 3, 0));
    }

    fn mix32(mut v: u32) -> u32 {
        v = v.wrapping_mul(2654435761).wrapping_add(0x9e3779b9);
        v ^= v >> 15;
        v = v.wrapping_mul(2246822519);
        v ^ (v >> 13)
    }

    fn doc_pixel(x: i32, doc_y: u32, width: u32) -> [u8; 4] {
        let paper = [248, 248, 250, 255];
        if x < 24 || x >= width as i32 - 24 {
            return paper;
        }
        let ux = (x - 24) as u32;
        // Render synthetic text lines of varying length and spacing so every
        // scroll offset has a unique band signature and realistic glyph edges.
        let block = doc_y / 28;
        let local_y = doc_y % 28;
        let block_hash = mix32(block.wrapping_mul(7919).wrapping_add(17));
        let indent = (block_hash % 3) * 8;
        let line_end = 72 + (block_hash >> 4) % 40;
        let active_rows = 8 + (block_hash >> 10) % 10; // 8..18px tall text line
        if local_y >= active_rows || ux < indent || ux >= line_end {
            return paper;
        }
        let glyph_idx = (ux - indent) / 6;
        let gx = (ux - indent) % 6;
        let gh = mix32(block.wrapping_mul(313).wrapping_add(glyph_idx));
        if gx == 5 {
            return paper;
        }
        // Vertical stem + diagonal stroke + 1px top/mid/bot bars per pseudo-glyph
        // so every text row differs from its vertical neighbour (preventing integer
        // row-trimming from collapsing a 0.5px shift onto an integer offset).
        let has_top = gh & 1 != 0;
        let has_mid = gh & 2 != 0;
        let has_bot = gh & 4 != 0;
        let diag = gx == (local_y + (gh >> 5)) % 5;
        let left_stem = gx == 0;
        let right_stem = (gh & 8 != 0) && gx == 4;
        let ink = left_stem
            || right_stem
            || diag
            || (has_top && local_y == 0)
            || (has_mid && local_y == active_rows / 2)
            || (has_bot && local_y + 1 == active_rows);
        if ink {
            let shade = 24 + ((gh >> 8) & 0x1f) as u8;
            [shade, shade + 4, shade + 10, 255]
        } else {
            paper
        }
    }

    fn render_page_frame(
        width: u32,
        height: u32,
        doc_top: f32,
        lateral_dx: i32,
        sticky_header_rows: u32,
        sticky_footer_rows: u32,
    ) -> Vec<u8> {
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                let src_x = x as i32 - lateral_dx;
                let px = if y < sticky_header_rows {
                    // Stationary structured header bar with uniform side margins.
                    if src_x < 24 || src_x >= width as i32 - 24 {
                        [38, 42, 50, 255]
                    } else if (src_x as u32 / 4 + y / 2).is_multiple_of(3)
                        && (4..sticky_header_rows - 4).contains(&y)
                    {
                        [230, 234, 240, 255]
                    } else {
                        [38, 42, 50, 255]
                    }
                } else if y >= height - sticky_footer_rows {
                    let fy = y - (height - sticky_footer_rows);
                    if src_x < 24 || src_x >= width as i32 - 24 {
                        [44, 48, 56, 255]
                    } else if (src_x as u32 / 5 + fy / 2).is_multiple_of(3)
                        && (3..sticky_footer_rows.saturating_sub(3)).contains(&fy)
                    {
                        [224, 228, 236, 255]
                    } else {
                        [44, 48, 56, 255]
                    }
                } else {
                    let sy = y as f32 + doc_top;
                    let y0 = sy.floor() as u32;
                    let frac = sy - y0 as f32;
                    if frac == 0.0 {
                        doc_pixel(src_x, y0, width)
                    } else {
                        let p0 = doc_pixel(src_x, y0, width);
                        let p1 = doc_pixel(src_x, y0 + 1, width);
                        [
                            ((1.0 - frac) * f32::from(p0[0]) + frac * f32::from(p1[0])).round()
                                as u8,
                            ((1.0 - frac) * f32::from(p0[1]) + frac * f32::from(p1[1])).round()
                                as u8,
                            ((1.0 - frac) * f32::from(p0[2]) + frac * f32::from(p1[2])).round()
                                as u8,
                            255,
                        ]
                    }
                };
                rgba[idx..idx + 4].copy_from_slice(&px);
            }
        }
        rgba
    }

    #[test]
    fn committed_sticky_header_and_footer_do_not_block_lateral_shift_recovery() {
        const W: u32 = 160;
        const H: u32 = 400;
        const HDR: u32 = 24;
        const FTR: u32 = 16;

        let seed = render_page_frame(W, H, 0.0, 0, HDR, FTR);
        let Ok(mut sheet) = ScrollSheet::new(W, H, &seed, SheetLimits::default()) else {
            panic!("valid viewport must initialize");
        };

        // Advance 6 scrolling frames so both sticky header and footer commit.
        for step in 1..=6 {
            let frame = render_page_frame(W, H, (step * 24) as f32, 0, HDR, FTR);
            let outcome = sheet.push(&frame);
            assert_eq!(
                outcome,
                Fold::GrewBottom { dy: 24, growth: 24 },
                "pre-shift frame {step}: {outcome:?}"
            );
        }
        assert_eq!(sheet.sticky.top, HDR);
        assert_eq!(sheet.sticky.bot, FTR);

        // Next scrolling frame has a +4 px lateral shift: `translation_support`
        // must trim `sticky.top` and `sticky.bot` instead of rejecting as `LayoutShift`.
        let shifted = render_page_frame(W, H, 168.0, 4, HDR, FTR);
        let outcome = sheet.push(&shifted);
        assert_eq!(outcome, Fold::GrewBottom { dy: 24, growth: 24 });
        assert_eq!(sheet.horizontal_offset, 4);
    }

    #[test]
    fn fractional_vertical_scroll_with_lateral_shift_recovers_cleanly() {
        const W: u32 = 160;
        const H: u32 = 320;

        let seed = render_page_frame(W, H, 0.0, 0, 0, 0);
        let Ok(mut sheet) = ScrollSheet::new(W, H, &seed, SheetLimits::default()) else {
            panic!("valid viewport must initialize");
        };

        // Half-pixel vertical scroll (`dy = 18.5`) combined with a +4 px lateral shift.
        // Verify that un-interpolated integer lookup (`19.0`) would reject with
        // regional mismatches, whereas continuous `subpixel_dy` (`18.5`) recovers cleanly.
        let shifted_half = render_page_frame(W, H, 18.5, 4, 0, 0);
        let aligned_pixels = super::shift_pixels(&shifted_half, W as usize, 4);
        let aligned_prof = super::RowProfile::from_rgba(&aligned_pixels, W, H);
        let uninterp = super::translation_support(
            &sheet.stable_profile,
            &aligned_prof,
            19.0,
            (0, 0),
            sheet.options.cost_threshold,
        );
        assert!(
            uninterp.mismatched > 0,
            "integer row lookup on half-pixel text edges must exhibit regional mismatch"
        );

        let outcome = sheet.push(&shifted_half);
        assert!(
            matches!(outcome, Fold::GrewBottom { dy: 18 | 19, .. }),
            "expected GrewBottom on half-pixel + lateral shift, got {outcome:?}"
        );
        assert_eq!(sheet.horizontal_offset, 4);
    }
}
