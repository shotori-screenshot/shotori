//! Local stationary overlays and overlap composition. Only pixels actually
//! exposed by another accepted viewport may replace an occluded pixel.

use std::ops::Range;

mod frame_edge;
pub(super) use frame_edge::FrameEdge;

const TILE_PIXELS: usize = 8;
const EDGE_CONTRAST: u8 = 12;
const STILL_TOLERANCE: u8 = 3;
const MIN_EDGE_SAMPLES: usize = 4;
// A clipped or translucent rim may leave only the glyph as evidence. Include
// its padded face and shadow, allowing for one tile of quantization error.
const HALO_PIXELS: usize = 3 * TILE_PIXELS;
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
        // Full-width edge bars belong to StickyBands, but a localized control
        // can touch a capture edge. Clipping its halo must not discard it.
        self.right - self.left <= width / 2
            && self.bottom - self.top <= height / 4
            && (self.right - self.left) * (self.bottom - self.top) <= width * height / 12
    }
}

pub(super) struct FloatingOverlays {
    regions: Vec<Region>,
    reference: Vec<u8>,
    displacement: i64,
    samples: Vec<RowSamples>,
    seed_pending: bool,
    pub(super) frame_edge: FrameEdge,
}

/// Provenance stays with each retained span, including a masked center within
/// a refreshed row. An undrawn or obstructed sample remains repairable.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Sample {
    source_y: usize,
    needs_repair: bool,
}

#[derive(Clone, Copy)]
struct SampleSpan {
    end: usize,
    sample: Sample,
}

#[derive(Clone)]
enum RowSamples {
    Uniform(Sample),
    Split(Vec<SampleSpan>),
}

impl RowSamples {
    fn source_at(&self, x: usize) -> Sample {
        match self {
            Self::Uniform(sample) => *sample,
            Self::Split(spans) => spans.iter().find(|span| x < span.end).map_or(
                Sample {
                    source_y: 0,
                    needs_repair: true,
                },
                |span| span.sample,
            ),
        }
    }
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
    pub(super) fn new(seed: &[u8], width: u32) -> Self {
        Self {
            regions: Vec::new(),
            reference: seed.to_vec(),
            displacement: 0,
            seed_pending: true,
            frame_edge: FrameEdge::new(seed, width as usize),
            samples: (0..seed.len() / (width as usize * 4))
                .map(|source_y| {
                    RowSamples::Uniform(Sample {
                        source_y,
                        needs_repair: false,
                    })
                })
                .collect(),
        }
    }

    /// Mirror a canvas insertion before the new rows receive captured pixels.
    pub(super) fn insert_rows(&mut self, at: usize, count: usize) {
        self.samples.splice(
            at..at,
            std::iter::repeat_n(
                RowSamples::Uniform(Sample {
                    source_y: 0,
                    needs_repair: true,
                }),
                count,
            ),
        );
    }

    pub(super) fn is_empty(&self) -> bool {
        self.regions.is_empty() && self.frame_edge.is_empty()
    }

    /// Retrospectively exclude confirmed outer chrome from a child column.
    /// Provenance locates earlier copies; matching the seed pixels prevents
    /// clearing content that was already observed without the decoration.
    pub(super) fn remove_frame_edge(
        &mut self,
        canvas: &mut [u8],
        width: usize,
        left: usize,
        edge: &FrameEdge,
        background: [u8; 4],
    ) {
        for (y, previous) in self.samples.iter_mut().enumerate() {
            let mut boundaries = vec![0, width];
            let mut sources = Vec::new();
            match &*previous {
                RowSamples::Uniform(sample) => sources.push(sample.source_y),
                RowSamples::Split(spans) => {
                    boundaries.extend(spans.iter().map(|span| span.end));
                    sources.extend(spans.iter().map(|span| span.sample.source_y));
                }
            }
            if sources.iter().all(|&source| source < edge.first_row()) {
                continue;
            }
            for source in sources {
                let mut full = Vec::new();
                edge.split_row(source, &mut full);
                boundaries.extend(
                    full.into_iter()
                        .filter(|x| (left..left + width).contains(x))
                        .map(|x| x - left),
                );
            }
            boundaries.sort_unstable();
            boundaries.dedup();
            let mut selected: Vec<SampleSpan> = Vec::new();
            for span in boundaries.windows(2) {
                let mut sample = previous.source_at(span[0]);
                if edge.contains(left + span[0], sample.source_y) {
                    for x in span[0]..span[1] {
                        let target = (y * width + x) * 4;
                        if difference(
                            &canvas[target..target + 4],
                            edge.seed_pixel(left + x, sample.source_y),
                        ) <= STILL_TOLERANCE
                        {
                            canvas[target..target + 4].copy_from_slice(&background);
                            sample.needs_repair = true;
                        }
                    }
                }
                if let Some(last) = selected.last_mut().filter(|last| last.sample == sample) {
                    last.end = span[1];
                } else {
                    selected.push(SampleSpan {
                        end: span[1],
                        sample,
                    });
                }
            }
            *previous = if selected.len() == 1 {
                RowSamples::Uniform(selected[0].sample)
            } else {
                RowSamples::Split(selected)
            };
        }
    }

    /// Window decoration evidence uses the whole capture, independently of
    /// column ownership, before the column samples are stripped of chrome.
    pub(super) fn observe_frame_edge(&mut self, current: &[u8], dy: i32) {
        self.displacement += i64::from(dy);
        self.frame_edge
            .observe(current, &self.reference, self.displacement);
    }

    /// Find localized edges that stay at viewport coordinates despite an
    /// independently verified page translation. Flat backgrounds cannot vote;
    /// edges that also match the page translation cannot vote either. Call
    /// after observe_frame_edge and the current column ownership update;
    /// the detection reference can span several accepted small movements.
    /// An independently positioned column owns its own overlays; it cannot
    /// vote for body occlusion or extend a halo back across its gutter.
    pub(super) fn observe(
        &mut self,
        current: &[u8],
        size: (u32, u32),
        column: Option<(usize, Range<usize>)>,
    ) {
        let previous = &self.reference;
        let (width, height) = (size.0 as usize, size.1 as usize);
        let excluded = column.map(|(left, rows)| Region {
            left,
            right: width,
            top: rows.start,
            bottom: rows.end,
        });
        self.regions
            .retain(|region| excluded.is_none_or(|column| !region.intersects(column)));
        let cols = width.div_ceil(TILE_PIXELS);
        let rows = height.div_ceil(TILE_PIXELS);
        let mut tiles = vec![false; cols * rows];
        let mut stationary = vec![0usize; cols * rows];
        for ty in 0..rows {
            for tx in 0..cols {
                let mut edges = 0;
                let mut still = 0;
                let mut displaced = 0;
                let mut stable_edges = 0;
                for y in ty * TILE_PIXELS..((ty + 1) * TILE_PIXELS).min(height - 1) {
                    let shifted = y as i64 + self.displacement;
                    let body_right = excluded
                        .filter(|column| y + 1 >= column.top && y < column.bottom)
                        .map_or(width, |column| column.left);
                    for x in tx * TILE_PIXELS..((tx + 1) * TILE_PIXELS).min(body_right - 1) {
                        if self.frame_edge.contains(x, y) {
                            continue;
                        }
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
                            if difference(&current[i + 4..i + 8], &previous[i + 4..i + 8])
                                <= EDGE_CONTRAST
                                && difference(
                                    &current[i + width * 4..i + width * 4 + 4],
                                    &previous[i + width * 4..i + width * 4 + 4],
                                ) <= EDGE_CONTRAST
                            {
                                stable_edges += 1;
                            }
                            // A valid translated match is page content even
                            // when the reverse sample crosses a newly appearing
                            // overlay. Use the reverse direction only when the
                            // forward comparison falls outside the frame. A
                            // large jump can put that reverse sample beneath a
                            // newly appearing edge bar, so it cannot establish
                            // an overlay without a closer reference.
                            let differs = if (0..height as i64).contains(&shifted) {
                                let j = (shifted as usize * width + x) * 4;
                                !self.frame_edge.contains(x, shifted as usize)
                                    && difference(p, &previous[j..j + 4]) > EDGE_CONTRAST
                            } else {
                                let reverse = y as i64 - self.displacement;
                                if self.displacement.abs() <= REFERENCE_TRAVEL_PIXELS
                                    && (0..height as i64).contains(&reverse)
                                {
                                    let j = (reverse as usize * width + x) * 4;
                                    !self.frame_edge.contains(x, reverse as usize)
                                        && difference(&previous[i..i + 4], &current[j..j + 4])
                                            > EDGE_CONTRAST
                                } else {
                                    false
                                }
                            };
                            displaced += usize::from(differs);
                        }
                    }
                }
                stationary[ty * cols + tx] = stable_edges;
                tiles[ty * cols + tx] = still >= MIN_EDGE_SAMPLES
                    && still * 4 >= edges * 3
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
            if let Some(column) = excluded.filter(|column| region.intersects(*column)) {
                if region.left >= column.left {
                    region.top = region.top.max(column.bottom);
                } else {
                    region.right = column.left;
                }
            }
            if region.top >= region.bottom || region.left >= region.right {
                continue;
            }
            if region.local(width, height) {
                // Changing content behind a button changes which edge tiles
                // testify. Preserve its extent while fresh evidence overlaps.
                for old in &self.regions {
                    let union = region.union(*old);
                    if !self.frame_edge.intersects(*old)
                        && region.intersects(*old)
                        && union.local(width, height)
                    {
                        region = union;
                    }
                }
                regions.push(region);
            }
        }
        // Keep confirmed controls from stable glyph edges rather than tile
        // purity: text behind a translucent face adds moving edges to the tile.
        // Neighbour agreement rejects coincident flat ink pixels; removal of
        // the control must also remove the stationary edge support.
        for old in &self.regions {
            if self.frame_edge.intersects(*old) {
                continue;
            }
            let mut support = 0;
            for y in old.top / TILE_PIXELS..old.bottom.div_ceil(TILE_PIXELS) {
                for x in old.left / TILE_PIXELS..old.right.div_ceil(TILE_PIXELS) {
                    support += stationary[y * cols + x];
                }
            }
            if support >= 2 * MIN_EDGE_SAMPLES && !regions.iter().any(|r| r.intersects(*old)) {
                regions.push(*old);
            }
        }
        self.regions = regions;
        if self.displacement.abs() >= REFERENCE_TRAVEL_PIXELS.min(height as i64 / 4) {
            self.reference.copy_from_slice(current);
            self.displacement = 0;
        }
    }

    /// Compose original observations. Near an edge control, prefer samples
    /// captured farther from that edge; never mix sharp and faded scanlines
    /// by independently classifying their colors. Local masks still permit
    /// later visible observations to repair an earlier occluded sample.
    pub(super) fn composite(
        &mut self,
        canvas: &mut [u8],
        frame: &[u8],
        width: usize,
        destination: usize,
        source: Range<usize>,
        retained: Range<usize>,
    ) {
        let row = width * 4;
        let height = frame.len() / row;
        self.samples.resize(
            canvas.len() / row,
            RowSamples::Uniform(Sample {
                source_y: 0,
                needs_repair: true,
            }),
        );
        let top_control = self.regions.iter().any(|r| r.top + r.bottom < height / 2);
        let bottom_control = self
            .regions
            .iter()
            .any(|r| r.top + r.bottom > height * 3 / 2);
        // Edge samples must not overwrite clearer interior observations even
        // before a newly appearing bar has accumulated stationary evidence.
        // A detected floating control extends this guard to include its fade.
        let top_guard = if top_control { height / 2 } else { height / 4 };
        let bottom_guard = if bottom_control {
            height / 2
        } else {
            height / 4
        };
        let quality = |y: usize| {
            if y < top_guard {
                y
            } else if y >= height - bottom_guard {
                height - 1 - y
            } else {
                height
            }
        };
        for y in source {
            let target = destination + y;
            let src = &frame[y * row..(y + 1) * row];
            let dst = &mut canvas[target * row..(target + 1) * row];
            let previous = &self.samples[target];
            let mut edges = vec![0, width];
            self.frame_edge.split_row(y, &mut edges);
            if let RowSamples::Split(spans) = previous {
                edges.extend(spans.iter().map(|span| span.end));
                for span in spans {
                    self.frame_edge.split_row(span.sample.source_y, &mut edges);
                }
            } else if let RowSamples::Uniform(sample) = previous {
                self.frame_edge.split_row(sample.source_y, &mut edges);
            }
            // Old and new masks share viewport coordinates, but their samples
            // can come from different rows and different accepted frames.
            for r in &self.regions {
                edges.extend([r.left, r.right]);
            }
            edges.sort_unstable();
            edges.dedup();
            let mut selected: Vec<SampleSpan> = Vec::new();
            for span in edges.windows(2) {
                let (left, right) = (span[0], span[1]);
                let mut old = previous.source_at(left);
                let occluded = |sy| {
                    self.frame_edge.contains(left, sy)
                        || self.regions.iter().any(|r| {
                            (r.left..r.right).contains(&left) && (r.top..r.bottom).contains(&sy)
                        })
                };
                // Only seed pixels need retrospective classification. Later
                // samples remember whether the capture itself was obstructed;
                // a newly detected region cannot invalidate old clear text.
                old.needs_repair |= self.seed_pending && occluded(old.source_y);
                let use_new = !retained.contains(&target)
                    || (!occluded(y) && (old.needs_repair || quality(y) >= quality(old.source_y)));
                let sample = if use_new {
                    dst[left * 4..right * 4].copy_from_slice(&src[left * 4..right * 4]);
                    Sample {
                        source_y: y,
                        needs_repair: occluded(y),
                    }
                } else {
                    old
                };
                if let Some(last) = selected.last_mut().filter(|last| last.sample == sample) {
                    last.end = right;
                } else {
                    selected.push(SampleSpan { end: right, sample });
                }
            }
            self.samples[target] = if selected.len() == 1 {
                RowSamples::Uniform(selected[0].sample)
            } else {
                RowSamples::Split(selected)
            };
        }
        self.seed_pending = false;
    }
}
