//! Local stationary overlays and overlap composition. Only pixels actually
//! exposed by another accepted viewport may replace an occluded pixel.

use std::ops::Range;

const TILE_PIXELS: usize = 8;
const EDGE_CONTRAST: u8 = 12;
const STILL_TOLERANCE: u8 = 3;
const MIN_EDGE_SAMPLES: usize = 4;
const HALO_PIXELS: usize = 16;
/// A multi-frame baseline separates slow page motion from stationary glyphs.
const REFERENCE_TRAVEL_PIXELS: i64 = 64;

#[derive(Clone, Copy, Debug)]
struct Region {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Region {
    fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    fn local(self, width: usize, height: usize) -> bool {
        // Edge bars belong to StickyBands. Broad stationary backdrops do not
        // provide enough evidence to identify a removable foreground object.
        self.top > 0
            && self.bottom < height
            && self.right - self.left <= width / 2
            && self.bottom - self.top <= height / 4
            && (self.right - self.left) * (self.bottom - self.top) <= width * height / 12
    }
}

pub(super) struct FloatingOverlays {
    regions: Vec<Region>,
    reference: Vec<u8>,
    displacement: i64,
}

fn difference(a: &[u8], b: &[u8]) -> u8 {
    a[..3]
        .iter()
        .zip(&b[..3])
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

impl FloatingOverlays {
    pub(super) fn new(seed: &[u8]) -> Self {
        Self {
            regions: Vec::new(),
            reference: seed.to_vec(),
            displacement: 0,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Find localized edges that stay at viewport coordinates despite an
    /// independently verified page translation. Flat backgrounds cannot vote;
    /// edges that also match the page translation cannot vote either. Call
    /// only for accepted frames: `dy` is relative to the previous acceptance,
    /// while the detection reference can span several smaller movements.
    pub(super) fn observe(&mut self, current: &[u8], size: (u32, u32), dy: i32) {
        self.displacement += i64::from(dy);
        let previous = &self.reference;
        let (width, height) = (size.0 as usize, size.1 as usize);
        let cols = width.div_ceil(TILE_PIXELS);
        let rows = height.div_ceil(TILE_PIXELS);
        let mut tiles = vec![false; cols * rows];
        let mut stationary = vec![false; cols * rows];
        for ty in 0..rows {
            for tx in 0..cols {
                let mut edges = 0;
                let mut still = 0;
                let mut displaced = 0;
                for y in ty * TILE_PIXELS..((ty + 1) * TILE_PIXELS).min(height - 1) {
                    let shifted = y as i64 + self.displacement;
                    for x in tx * TILE_PIXELS..((tx + 1) * TILE_PIXELS).min(width - 1) {
                        let i = (y * width + x) * 4;
                        let p = &current[i..i + 4];
                        if difference(p, &current[i + 4..i + 8]) < EDGE_CONTRAST
                            && difference(p, &current[i + width * 4..i + width * 4 + 4])
                                < EDGE_CONTRAST
                        {
                            continue;
                        }
                        edges += 1;
                        if difference(p, &previous[i..i + 4]) <= STILL_TOLERANCE {
                            still += 1;
                            if (0..height as i64).contains(&shifted) {
                                let j = (shifted as usize * width + x) * 4;
                                if difference(p, &previous[j..j + 4]) > EDGE_CONTRAST {
                                    displaced += 1;
                                }
                            }
                        }
                    }
                }
                stationary[ty * cols + tx] = still >= MIN_EDGE_SAMPLES && still * 4 >= edges * 3;
                tiles[ty * cols + tx] = stationary[ty * cols + tx]
                    && displaced >= MIN_EDGE_SAMPLES
                    && displaced * 2 >= still;
            }
        }

        let mut regions = Vec::new();
        for index in 0..tiles.len() {
            if !tiles[index] {
                continue;
            }
            tiles[index] = false;
            let mut pending = vec![index];
            let mut region = Region {
                left: width,
                top: height,
                right: 0,
                bottom: 0,
            };
            let mut count = 0;
            while let Some(i) = pending.pop() {
                count += 1;
                let (x, y) = (i % cols, i / cols);
                region = region.union(Region {
                    left: x * TILE_PIXELS,
                    top: y * TILE_PIXELS,
                    right: ((x + 1) * TILE_PIXELS).min(width),
                    bottom: ((y + 1) * TILE_PIXELS).min(height),
                });
                for ny in y.saturating_sub(1)..=(y + 1).min(rows - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(cols - 1) {
                        let neighbor = ny * cols + nx;
                        if tiles[neighbor] {
                            tiles[neighbor] = false;
                            pending.push(neighbor);
                        }
                    }
                }
            }
            if count < 2 {
                continue;
            }
            // Include the untextured interior and antialiased perimeter, not
            // just the glyph strokes that provided motion evidence.
            region.left = region.left.saturating_sub(HALO_PIXELS);
            region.top = region.top.saturating_sub(HALO_PIXELS);
            region.right = (region.right + HALO_PIXELS).min(width);
            region.bottom = (region.bottom + HALO_PIXELS).min(height);
            if region.local(width, height) {
                // Changing content behind a button changes which edge tiles
                // testify. Preserve its extent while fresh evidence overlaps.
                for old in &self.regions {
                    let union = region.union(*old);
                    if region.intersects(*old) && union.local(width, height) {
                        region = union;
                    }
                }
                regions.push(region);
            }
        }
        // Tiny steps may not move a glyph by its own stroke width. Once an
        // overlay is located, fresh stationary edges keep it masked across
        // baseline refreshes; disappearance removes that supporting evidence.
        for old in &self.regions {
            let mut support = 0;
            for y in old.top / TILE_PIXELS..old.bottom.div_ceil(TILE_PIXELS) {
                for x in old.left / TILE_PIXELS..old.right.div_ceil(TILE_PIXELS) {
                    support += usize::from(stationary[y * cols + x]);
                }
            }
            if support >= 2 && !regions.iter().any(|r| r.intersects(*old)) {
                regions.push(*old);
            }
        }
        self.regions = regions;
        if self.displacement.abs() >= REFERENCE_TRAVEL_PIXELS.min(height as i64 / 4) {
            self.reference.copy_from_slice(current);
            self.displacement = 0;
        }
    }

    /// Write visible content, preserving previously captured pixels under a
    /// floating overlay. Newly exposed rows have no replacement observation:
    /// retain the actual capture there, never synthesize background or text.
    pub(super) fn composite(
        &self,
        canvas: &mut [u8],
        frame: &[u8],
        width: usize,
        destination: usize,
        source: Range<usize>,
        retained: Range<usize>,
    ) {
        let row = width * 4;
        for y in source {
            let target = destination + y;
            let src = &frame[y * row..(y + 1) * row];
            let dst = &mut canvas[target * row..(target + 1) * row];
            if !retained.contains(&target) || self.regions.is_empty() {
                dst.copy_from_slice(src);
                continue;
            }
            let mut spans: Vec<_> = self
                .regions
                .iter()
                .filter(|r| (r.top..r.bottom).contains(&y))
                .map(|r| r.left..r.right)
                .collect();
            spans.sort_unstable_by_key(|span| span.start);
            let mut x = 0;
            for span in spans {
                if x < span.start {
                    dst[x * 4..span.start * 4].copy_from_slice(&src[x * 4..span.start * 4]);
                }
                x = x.max(span.end);
            }
            dst[x * 4..].copy_from_slice(&src[x * 4..]);
        }
    }
}
