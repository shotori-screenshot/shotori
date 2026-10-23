//! Captured window decorations stay on the stitched image's outer perimeter.
//! Pixel masks keep rounded corners separate from document and column content.

use super::{EDGE_CONTRAST, Region, STILL_TOLERANCE, difference};

const FRAME_EDGE_FRACTION: usize = 8;

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

    pub(super) fn observe(&mut self, current: &[u8], reference: &[u8], displacement: i64) {
        if displacement == 0 {
            return;
        }
        let width = self.width;
        let height = self.mask.len() / width;
        let row = width * 4;
        if let Some(y) = (0..self.seed_prefix_rows)
            .find(|&y| current[y * row..(y + 1) * row] != self.seed[y * row..(y + 1) * row])
        {
            self.seed_prefix_rows = y;
        }
        let first = height - height / FRAME_EDGE_FRACTION;
        // Antialiasing changes color with the document beneath the rim. Keep
        // its established coverage while fresh opaque edge pixels attest the
        // same frame; losing that support clears the whole mask below.
        let mut candidates = self.mask.clone();
        let mut fixed_bottom = 0;
        for y in first..height {
            let forward = y as i64 + displacement;
            let reverse = y as i64 - displacement;
            for x in 0..width {
                let i = y * width + x;
                let pixel = &current[i * 4..i * 4 + 4];
                if difference(pixel, &self.seed[i * 4..i * 4 + 4]) > STILL_TOLERANCE
                    || difference(pixel, &reference[i * 4..i * 4 + 4]) > STILL_TOLERANCE
                {
                    continue;
                }
                let displaced = if (0..height as i64).contains(&forward) {
                    let j = (forward as usize * width + x) * 4;
                    difference(pixel, &reference[j..j + 4]) > STILL_TOLERANCE
                } else if (0..height as i64).contains(&reverse) {
                    let j = (reverse as usize * width + x) * 4;
                    difference(&reference[i * 4..i * 4 + 4], &current[j..j + 4]) > STILL_TOLERANCE
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
