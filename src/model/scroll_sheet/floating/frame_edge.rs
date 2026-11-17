//! Captured window decorations stay on the stitched image's outer perimeter.
//! Pixel masks keep rounded corners separate from document and column content.

use super::{EDGE_CONTRAST, Region, STILL_TOLERANCE, difference, lerp_rgb};

const FRAME_EDGE_FRACTION: usize = 8;

/// Whether `pixel` differs from its translated vertical partner in `partner`
/// at continuous row `coord`. Near-integer coordinates keep the exact single-row
/// check; fractional coordinates require contrast against both straddling rows
/// and their linear blend (+1 LSB for double rounding) so subpixel-filtered
/// glyph edges and gentle ramps do not masquerade as stationary window borders.
fn partner_differs(
    pixel: &[u8],
    partner: &[u8],
    x: usize,
    coord: f64,
    width: usize,
    height: usize,
) -> bool {
    let nearest = coord.round();
    if (coord - nearest).abs() <= 1e-4 {
        let row = (nearest as usize).min(height - 1);
        let j = (row * width + x) * 4;
        return difference(pixel, &partner[j..j + 4]) > STILL_TOLERANCE;
    }
    let s0 = coord.floor() as usize;
    let s1 = (s0 + 1).min(height - 1);
    let frac = (coord - s0 as f64) as f32;
    let j0 = (s0 * width + x) * 4;
    let j1 = (s1 * width + x) * 4;
    let p0 = &partner[j0..j0 + 4];
    let p1 = &partner[j1..j1 + 4];
    difference(pixel, p0) > STILL_TOLERANCE
        && difference(pixel, p1) > STILL_TOLERANCE
        && difference(pixel, &lerp_rgb(p0, p1, frac)) > STILL_TOLERANCE + 1
}

/// Seed window decorations connected to the lower capture edge. Unlike a
/// floating control, a thin frame needs a pixel mask, not a rectangular halo.
pub(in super::super) struct FrameEdge {
    seed: Vec<u8>,
    seed_prefix_rows: usize,
    mask: Vec<bool>,
    width: usize,
    first_row: usize,
    rails: Vec<Option<FrameRail>>,
    bottom_pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
struct FrameRail {
    color: [u8; 4],
    background: [u8; 4],
}

impl FrameEdge {
    pub(in super::super) fn mask(&self) -> &[bool] {
        &self.mask
    }

    pub(in super::super) fn is_empty(&self) -> bool {
        self.first_row == self.mask.len() / self.width
    }

    pub(in super::super) fn seed_prefix_rows(&self) -> usize {
        self.seed_prefix_rows
    }

    /// Track how many top rows remain identical to the seed frame without
    /// running the bottom window-perimeter flood fill (used when a fixed
    /// textured backdrop owns the background plane).
    pub(in super::super) fn observe_seed_prefix(&mut self, current: &[u8]) {
        let row = self.width * 4;
        if let Some(y) = (0..self.seed_prefix_rows)
            .find(|&y| current[y * row..(y + 1) * row] != self.seed[y * row..(y + 1) * row])
        {
            self.seed_prefix_rows = y;
        }
    }

    pub(super) fn first_row(&self) -> usize {
        self.first_row
    }

    pub(super) fn seed_pixel(&self, x: usize, y: usize) -> &[u8] {
        let at = (y * self.width + x) * 4;
        &self.seed[at..at + 4]
    }

    pub(super) fn new(seed: &[u8], width: usize) -> Self {
        let height = seed.len() / (width * 4);
        Self {
            seed: seed.to_vec(),
            seed_prefix_rows: height,
            mask: vec![false; seed.len() / 4],
            width,
            first_row: height,
            rails: vec![None; width],
            bottom_pixels: seed[(height - height / FRAME_EDGE_FRACTION) * width * 4..].to_vec(),
        }
    }

    pub(super) fn observe(&mut self, current: &[u8], reference: &[u8], subpixel_displacement: f64) {
        let width = self.width;
        if width == 0 {
            return;
        }
        let height = self.mask.len() / width;
        if height == 0 || subpixel_displacement.abs() < 0.5 {
            return;
        }
        self.observe_seed_prefix(current);
        let first = height - height / FRAME_EDGE_FRACTION;
        // Antialiasing changes color with the document beneath the rim. Keep
        // its established coverage while fresh opaque edge pixels attest the
        // same frame; losing that support clears the whole mask below.
        let mut candidates = self.mask.clone();
        let mut fixed_bottom = 0;
        let max_y = (height - 1) as f64;
        for y in first..height {
            let forward = y as f64 + subpixel_displacement;
            let reverse = y as f64 - subpixel_displacement;
            for x in 0..width {
                let i = y * width + x;
                let pixel = &current[i * 4..i * 4 + 4];
                if difference(pixel, &self.seed[i * 4..i * 4 + 4]) > STILL_TOLERANCE
                    || difference(pixel, &reference[i * 4..i * 4 + 4]) > STILL_TOLERANCE
                {
                    continue;
                }
                let displaced = if (0.0..=max_y).contains(&forward) {
                    partner_differs(pixel, reference, x, forward, width, height)
                } else if (0.0..=max_y).contains(&reverse) {
                    partner_differs(
                        &reference[i * 4..i * 4 + 4],
                        current,
                        x,
                        reverse,
                        width,
                        height,
                    )
                } else {
                    false
                };
                candidates[i] = displaced || self.mask[i];
                if y == height - 1 && candidates[i] {
                    fixed_bottom += 1;
                }
            }
        }
        // A local button must not establish ownership of the window perimeter.
        let bottom = (height - 1) * width;
        if fixed_bottom < width / 2 {
            self.mask.fill(false);
            self.first_row = height;
            self.rails.fill(None);
            return;
        }
        self.mask.fill(false);
        self.first_row = height;
        let mut pending = Vec::new();
        for x in 0..width {
            if candidates[bottom + x] {
                candidates[bottom + x] = false;
                pending.push(bottom + x);
            }
        }
        while let Some(i) = pending.pop() {
            let (x, y) = (i % width, i / width);
            self.mask[i] = true;
            self.first_row = self.first_row.min(y);
            for (nx, ny) in [
                (x.saturating_sub(1), y),
                ((x + 1).min(width - 1), y),
                (x, y.saturating_sub(1).max(first)),
                (x, (y + 1).min(height - 1)),
            ] {
                let next = ny * width + nx;
                if candidates[next] {
                    candidates[next] = false;
                    pending.push(next);
                }
            }
        }
        // Only constant, contrasting outer columns attest vertical frame rails.
        // A blank page margin beside text is insufficient evidence.
        let edge = (width / 64).max(4).min(width / 8);
        for x in (0..edge).chain(width - edge..width) {
            let at = (height / 2 * width + x) * 4;
            let color = &self.seed[at..at + 4];
            let inside = if x < edge {
                edge * 2
            } else {
                width - edge * 2 - 1
            };
            let rows = height / 4..height * 3 / 4;
            let mut contrasted = 0;
            let fixed = rows
                .clone()
                .filter(|&y| {
                    let i = (y * width + x) * 4;
                    let j = (y * width + inside) * 4;
                    contrasted +=
                        usize::from(difference(color, &current[j..j + 4]) > EDGE_CONTRAST);
                    difference(color, &self.seed[i..i + 4]) <= STILL_TOLERANCE
                        && difference(color, &current[i..i + 4]) <= STILL_TOLERANCE
                })
                .count();
            let inside_at = (height / 2 * width + inside) * 4;
            let background = &self.seed[inside_at..inside_at + 4];
            self.rails[x] =
                (fixed * 4 >= rows.len() * 3 && contrasted * 4 >= rows.len() * 3).then(|| {
                    FrameRail {
                        color: [color[0], color[1], color[2], color[3]],
                        background: [background[0], background[1], background[2], background[3]],
                    }
                });
        }
        // A perimeter has a broad bottom strip and two edge-connected corners.
        // Unrestricted connectivity can climb from that strip through still
        // page whitespace, then permanently erase text that enters it later.
        let mut strip = height;
        while strip > first
            && [0..width / 2, width / 2..width].iter().all(|half| {
                self.mask[(strip - 1) * width + half.start..(strip - 1) * width + half.end]
                    .iter()
                    .filter(|&&masked| masked)
                    .count()
                    * 2
                    > half.len()
            })
        {
            strip -= 1;
        }
        if strip == height {
            self.mask.fill(false);
            self.first_row = height;
            self.rails.fill(None);
            return;
        }
        // A blank bottom margin on a frameless page looks "displaced" whenever a
        // wide contrasting banner or code block crosses the reverse sample row,
        // flooding uniform whitespace up to `strip` and overwriting later text.
        // Require either bilateral vertical rails or a stationary horizontal
        // perimeter contrast against the interior row immediately above `strip`.
        let has_side_rails = self.rails[..edge].iter().any(Option::is_some)
            && self.rails[width - edge..].iter().any(Option::is_some);
        let above_y = strip.saturating_sub(1);
        let has_bottom_border = !has_side_rails && {
            let contrasting = (0..width)
                .filter(|&x| {
                    let above = (above_y * width + x) * 4;
                    let seed_above = &self.seed[above..above + 4];
                    let cur_above = &current[above..above + 4];
                    (strip..height).any(|y| {
                        let i = y * width + x;
                        if !self.mask[i] {
                            return false;
                        }
                        let at = i * 4;
                        difference(&self.seed[at..at + 4], seed_above) > EDGE_CONTRAST
                            && difference(&current[at..at + 4], cur_above) > EDGE_CONTRAST
                    })
                })
                .count();
            contrasting * 2 > width
        };
        if !has_side_rails && !has_bottom_border {
            self.mask.fill(false);
            self.first_row = height;
            self.rails.fill(None);
            return;
        }
        let mut left = width / 2;
        let mut right = width / 2;
        for y in (first..strip).rev() {
            let row = &mut self.mask[y * width..(y + 1) * width];
            left = left.min(
                (0..left)
                    .take_while(|&x| row[x] || self.rails[x].is_some())
                    .count(),
            );
            right = right.min(
                (0..right)
                    .take_while(|&x| row[width - 1 - x] || self.rails[width - 1 - x].is_some())
                    .count(),
            );
            row[left..width - right].fill(false);
        }
        self.first_row = self
            .mask
            .iter()
            .position(|&pixel| pixel)
            .map_or(height, |i| i / width);
    }

    pub(super) fn contains(&self, x: usize, y: usize) -> bool {
        y >= self.first_row && self.mask[y * self.width + x]
    }

    pub(super) fn intersects(&self, region: Region) -> bool {
        (region.top.max(self.first_row)..region.bottom)
            .any(|y| (region.left..region.right).any(|x| self.contains(x, y)))
    }

    pub(super) fn split_row(&self, y: usize, edges: &mut Vec<usize>) {
        if y < self.first_row {
            return;
        }
        for x in 1..self.width {
            if self.contains(x - 1, y) != self.contains(x, y) {
                edges.push(x);
            }
        }
    }

    /// Preserve the captured frame once, outside both document and column
    /// planes. Call after column composition, which must not own the frame.
    pub(in super::super) fn composite(&mut self, canvas: &mut [u8], frame: &[u8], position: usize) {
        let row = self.width * 4;
        let height = canvas.len() / row;
        let viewport = self.mask.len() / self.width;
        let first = viewport - viewport / FRAME_EDGE_FRACTION;
        if position + viewport == height {
            self.bottom_pixels.copy_from_slice(&frame[first * row..]);
        }
        for (x, rail) in self
            .rails
            .iter()
            .enumerate()
            .filter_map(|(x, c)| c.map(|c| (x, c)))
        {
            for y in viewport / FRAME_EDGE_FRACTION..height {
                let at = y * row + x * 4;
                // Only extend a rail across synthetic column padding. Keep
                // observed antialiased pixels over actual text and graphics.
                if difference(&canvas[at..at + 4], &rail.background) <= STILL_TOLERANCE {
                    canvas[at..at + 4].copy_from_slice(&rail.color);
                }
            }
        }
        for y in self.first_row..viewport {
            for x in 0..self.width {
                if self.contains(x, y) {
                    let source = (y - first) * row + x * 4;
                    let target = (height - viewport + y) * row + x * 4;
                    canvas[target..target + 4]
                        .copy_from_slice(&self.bottom_pixels[source..source + 4]);
                }
            }
        }
    }

    /// Window pixels are not observations of an independent column's content.
    /// Its existing gutter supplies padding until an unobstructed row is seen.
    pub(in super::super) fn remove_from_frame(&self, frame: &mut [u8], background: [u8; 4]) {
        for y in self.first_row..self.mask.len() / self.width {
            for x in 0..self.width {
                if self.contains(x, y) {
                    let at = (y * self.width + x) * 4;
                    frame[at..at + 4].copy_from_slice(&background);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::FloatingOverlays;
    use std::ops::Range;

    fn paint_text_lines(page: &mut [u8], width: usize, rows: Range<usize>) {
        let mut y = rows.start;
        let mut line = 0usize;
        while y + 8 < rows.end {
            for dy in 0..3 {
                for x in 24..width - 24 {
                    // Varied word gaps keep horizontal edge structure realistic.
                    if (x + line * 11) % 29 >= 6 {
                        let at = ((y + dy) * width + x) * 4;
                        page[at..at + 4].copy_from_slice(&[35, 35, 35, 255]);
                    }
                }
            }
            y += 18 + (line % 3) * 2;
            line += 1;
        }
    }

    fn slice(page: &[u8], width: usize, height: usize, top: usize) -> Vec<u8> {
        let row = width * 4;
        page[top * row..(top + height) * row].to_vec()
    }

    #[test]
    fn frameless_blank_bottom_margin_with_wide_banner_preserves_later_text() {
        let (width, height, page_height) = (240usize, 320usize, 800usize);
        let row = width * 4;
        let mut seed_page = vec![255u8; width * page_height * 4];
        paint_text_lines(&mut seed_page, width, 20..240);
        paint_text_lines(&mut seed_page, width, 360..760);

        // Once scrolling starts, a full-width dark banner/code-block sits at
        // page rows 295..320 so at `top = 40` (`reverse = y - 40` for `y` in
        // 295..320) the reverse sample lands squarely inside the dark block
        // while `280..320` on both seed and current are blank white margin.
        let mut scrolled_page = seed_page.clone();
        for y in 295..320 {
            for x in 12..width - 12 {
                let at = (y * width + x) * 4;
                scrolled_page[at..at + 4].copy_from_slice(&[24, 28, 36, 255]);
            }
        }

        let seed = slice(&seed_page, width, height, 0);
        let mut canvas = seed.clone();
        let mut floating = FloatingOverlays::new(&seed, width as u32);
        for top in (40..=280).step_by(40) {
            let frame = slice(&scrolled_page, width, height, top);
            let retained = 0..canvas.len() / row;
            canvas.resize((top + height) * row, 0);
            floating.observe_frame_edge_subpixel(&frame, 40, 40.0);
            assert!(
                floating.frame_edge.is_empty(),
                "false bottom-margin perimeter lock at top={top}"
            );
            floating.observe(&frame, (width as u32, height as u32), None);
            floating.composite(&mut canvas, &frame, width, top, 0..height, retained);
            floating.frame_edge.composite(&mut canvas, &frame, top);
        }

        // Every bottom text line entering after the blank margin must survive intact.
        let text_range = 360 * row..(280 + height) * row;
        assert_eq!(&canvas[text_range.clone()], &scrolled_page[text_range]);
    }

    #[test]
    fn stationary_bottom_border_without_side_rails_is_preserved() {
        let (width, height, page_height) = (240usize, 320usize, 720usize);
        let row = width * 4;
        let border = [20u8, 130, 230, 255];
        let mut page = vec![255u8; width * page_height * 4];
        paint_text_lines(&mut page, width, 20..700);

        let capture = |top: usize| {
            let mut frame = slice(&page, width, height, top);
            for y in height - 3..height {
                for x in 0..width {
                    let at = (y * width + x) * 4;
                    frame[at..at + 4].copy_from_slice(&border);
                }
            }
            frame
        };

        let seed = capture(0);
        let mut canvas = seed.clone();
        let mut floating = FloatingOverlays::new(&seed, width as u32);
        for top in (24..=240).step_by(24) {
            let frame = capture(top);
            let retained = 0..canvas.len() / row;
            canvas.resize((top + height) * row, 0);
            floating.observe_frame_edge_subpixel(&frame, 24, 24.0);
            assert!(
                !floating.frame_edge.is_empty(),
                "missed bottom border at top={top}"
            );
            assert!(
                floating.frame_edge.rails.iter().all(Option::is_none),
                "frameless sides must not synthesize vertical rails"
            );
            assert_eq!(floating.frame_edge.first_row(), height - 3);
            for y in 0..height - 3 {
                for x in 0..width {
                    assert!(
                        !floating.frame_edge.contains(x, y),
                        "interior pixel ({x}, {y}) masked at top={top}"
                    );
                }
            }
            for y in height - 3..height {
                for x in 0..width {
                    assert!(
                        floating.frame_edge.contains(x, y),
                        "border pixel ({x}, {y}) unmasked at top={top}"
                    );
                }
            }
            floating.observe(&frame, (width as u32, height as u32), None);
            floating.composite(&mut canvas, &frame, width, top, 0..height, retained);
            floating.frame_edge.composite(&mut canvas, &frame, top);
        }

        let interior_end = 240 + height - 3;
        assert_eq!(
            &canvas[..interior_end * row],
            &page[..interior_end * row],
            "stationary bottom border leaked into interior scrolled rows"
        );
        for y in interior_end..240 + height {
            for x in 0..width {
                let at = (y * width + x) * 4;
                assert_eq!(&canvas[at..at + 4], &border);
            }
        }
    }
}
