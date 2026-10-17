//! Frame profiles and the band-mask: the cheap per-frame signal the
//! matcher ranks with. [`RowProfile`] reduces one frame to grayscale
//! interior rows plus the per-band statistics; [`MotionMask`] learns,
//! from consecutive frame pairs, which column bands are fixed chrome
//! and drops them from every later comparison.

/// A frame reduced for matching: **full grayscale rows** over the
/// interior columns [w/8, 7w/8) — edges are excluded (scrollbars and
/// fixed sidebars live there and would poison the comparison).
///
/// Row MEANS are deliberately NOT used: real text defeats them (measured
/// on a terminal — every line's glyph mix converges to nearly the same
/// mean, so a whole-line-height shift leaves the mean signal unchanged
/// and the matcher reports Still at d = 0). Pixel rows keep the
/// per-line glyph detail that distinguishes offsets.
///
/// The cost scan walks a `step`-spaced grid of CUR-frame rows against
/// the FULL prev storage (arbitrary offsets stay comparable); step is
/// sized to keep a scan ≈ 100k byte-ops.
#[derive(Debug, Clone)]
pub(super) struct RowProfile {
    pub(super) height: u32,
    /// Interior width in bytes (one luma per pixel).
    pub(super) stride: usize,
    /// Kept-curve row spacing for the cost scan.
    pub(super) step: usize,
    /// `height` rows × `stride` luma bytes, row-major.
    pub(super) data: Vec<u8>,
    /// Eight narrow sample bands inside `stride`, as `[start, end)` column
    /// ranges. Used by the cheap candidate search and the sidebar mask.
    pub(super) bands: [(usize, usize); BAND_COUNT],
    /// Per-row band means: the cheap signal the offset search ranks
    /// candidates with. Full-row pixel MAD still decides the winner.
    /// Cell `[row][band]` is 0–255 luma.
    pub(super) band_means: Vec<[f32; BAND_COUNT]>,
    /// Largest per-row interior mean spread, precomputed once. A frame
    /// whose rows are (near-)uniform carries no alignment information:
    /// every translation window looks identical — the conservative
    /// answer is Still; a later textured frame brings something to
    /// match against. Flat PDF margins hit this.
    pub(super) spread: u8,
    /// Per-row 0–1 evidence weight: how much this row differs from the
    /// row above it. A vertical translation can only be observed through
    /// vertical VARIATION — a row byte-identical to its neighbour could
    /// sit anywhere in the other frame and still "match". Averaging over
    /// such rows is not merely useless, it is actively harmful: on a page
    /// that is mostly blank two blank regions match at ANY distance with
    /// cost ~0, so the cheapest offset is decided by whatever leftover
    /// texture happens to line up. Measured on the sparse-page fixture:
    /// after two refusals the matcher accepted `dy = -474` for a 272-row
    /// scroll and rewrote the sheet in the wrong place. Weights turn
    /// "average over everything" into "average over the rows that can
    /// testify".
    pub(super) row_weight: Vec<f32>,
}

/// `row_weight` reaches 1.0 once a row differs from its neighbour by this
/// much mean luma (0–255) — the ramp stops small antialiasing shimmer
/// from counting as evidence without drowning real text (which scores
/// far above the cap).
pub(super) const ROW_EVIDENCE_SATURATION: f32 = 24.;
/// Sampled rows of evidence a verdict needs. Below this the overlap is
/// too flat to place: refuse and let a textured frame do it. Without a
/// floor the cheapest offset among a handful of informative rows wins by
/// luck.
pub(super) const MIN_EVIDENCE_ROWS: usize = 16;

/// Samples compared per candidate offset, topside for the cost scan.
pub(super) const SCAN_BUDGET: usize = 120_000;
/// Narrow bands sampled across the interior: cheap per-row signal for
/// candidate ranking and the fixed-column motion mask. Eight thin strips
/// retain per-glyph detail that full-row means lose on text, at ~1/stride
/// the cost.
pub(super) const BAND_COUNT: usize = 8;
/// A band whose relative frame-to-frame change stays below this is fixed
/// chrome, not scrolling content.
const BAND_MOTION_FLOOR: f32 = 0.10;
/// 3–7 moving bands means "some bands are fixed chrome". Fewer than 3
/// is too little signal to trust the mask.
const MIN_MASKED_BANDS: usize = 3;

pub(super) type BandWeights = [f32; BAND_COUNT];
pub(super) const ALL_BANDS: BandWeights = [1.0; BAND_COUNT];

/// Column ranges for the band means: a 4 %–96 % stair, so scrollbars and
/// borders fall outside and each band stays narrow.
fn band_spans(stride: usize) -> [(usize, usize); BAND_COUNT] {
    let mut spans = [(0, 0); BAND_COUNT];
    if stride == 0 {
        return spans;
    }
    let span = (0.96 - 0.04) / BAND_COUNT as f32;
    for (b, out) in spans.iter_mut().enumerate() {
        let start = 0.04 + span * b as f32;
        let first = (stride as f32 * start).round() as usize;
        let last = (stride as f32 * (start + span * 0.95)).round() as usize;
        let first = first.min(stride - 1);
        let last = last.clamp(first, stride - 1);
        *out = (first, last + 1);
    }
    spans
}

impl RowProfile {
    pub(super) fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
        let w = width as usize;
        let interior = w / 8;
        let stride = w - 2 * interior;
        let stride = stride.max(1);
        let mut data = vec![0u8; height as usize * stride];
        for row in 0..height as usize {
            let src = &rgba[row * w * 4..];
            let dst = &mut data[row * stride..(row + 1) * stride];
            for (i, out) in dst.iter_mut().enumerate() {
                let p = (interior + i) * 4;
                // Integer luma (77/150/29 ≈ 0.30/0.59/0.11 over 256):
                // cheap, no float, monotone in brightness. Alpha is
                // ignored on purpose — it carries no geometry.
                let y = u32::from(src[p]) * 77
                    + u32::from(src[p + 1]) * 150
                    + u32::from(src[p + 2]) * 29;
                *out = (y / 256) as u8;
            }
        }
        // Grid spacing keeps one candidate scan ≈ SCAN_BUDGET byte ops.
        let step = (height as usize * stride / SCAN_BUDGET).clamp(1, 8);

        let bands = band_spans(stride);
        let mut band_means = vec![[0.0f32; BAND_COUNT]; height as usize];
        let mut spread = 0u8;
        let mut prev_mean: Option<u8> = None;
        for (row, means) in band_means.iter_mut().enumerate() {
            let line = &data[row * stride..(row + 1) * stride];
            for (band, (start, end)) in bands.iter().copied().enumerate() {
                let width = end - start;
                let samples = width.clamp(1, 9);
                let step = if samples > 1 {
                    (width - 1) as f32 / (samples - 1) as f32
                } else {
                    0.0
                };
                let mut sum = 0.0f32;
                for i in 0..samples {
                    let x = (start as f32 + i as f32 * step).round() as usize;
                    sum += f32::from(line[x.min(end - 1)]);
                }
                means[band] = sum / samples as f32;
            }
            let mean = (line.iter().copied().map(u32::from).sum::<u32>() / stride as u32) as u8;
            if let Some(prev) = prev_mean {
                spread = spread.max(mean.abs_diff(prev));
            }
            prev_mean = Some(mean);
        }
        // Evidence weights: mean |row − row above|, saturated. Row 0 has
        // no row above it and borrows row 1's reading (a frame's first
        // row is not special, only unmeasurable in isolation).
        let mut row_weight = vec![0.0f32; height as usize];
        for row in 1..height as usize {
            let above = &data[(row - 1) * stride..row * stride];
            let here = &data[row * stride..(row + 1) * stride];
            let diff = here
                .iter()
                .zip(above.iter())
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum::<u32>() as f32
                / stride as f32;
            row_weight[row] = (diff / ROW_EVIDENCE_SATURATION).min(1.0);
        }
        if height > 1 {
            row_weight[0] = row_weight[1];
        }
        Self {
            height,
            stride,
            step,
            data,
            bands,
            band_means,
            spread,
            row_weight,
        }
    }

    /// Mean per-row band change over `[from, to)` rows, per band.
    /// Used by the sidebar mask; only the middle rows of the viewport
    /// vote so fixed edge chrome cannot make every band look static.
    ///
    /// Per-ROW deltas, not the delta of row means: a scrolling page's
    /// row averages look almost identical across frames (the texture
    /// distribution is stable), while individual rows change every
    /// time the page moves. The static pane scores ~0 either way.
    fn band_change(&self, other: &RowProfile, from: usize, to: usize) -> [f32; BAND_COUNT] {
        let mut change = [0.0f32; BAND_COUNT];
        let rows = to.saturating_sub(from);
        if rows == 0 {
            return change;
        }
        for (a, b) in self.band_means[from..to]
            .iter()
            .zip(other.band_means[from..to].iter())
        {
            for (total, (x, y)) in change.iter_mut().zip(a.iter().zip(b.iter())) {
                *total += (x - y).abs();
            }
        }
        for total in &mut change {
            *total /= rows as f32;
        }
        change
    }
}

/// Fixed-sidebar mask: while the page scrolls, a thumbnail pane or an
/// outline keeps most of its pixels put. Comparing consecutive band
/// means, normalizing each band by the strongest and down-weighting the
/// still ones is what stops a sidebar's own repaints from poisoning the
/// offset score — a PDF viewer then folds like any other page.
#[derive(Debug, Clone)]
pub(super) struct MotionMask {
    /// Exponentially smoothed relative motion per band.
    pub(super) motion: [f32; BAND_COUNT],
    pub(super) sampled: bool,
    /// 0.0 masks a band out of both the candidate score and the
    /// full-row verification; 1.0 keeps it.
    pub(super) weights: BandWeights,
}

impl Default for MotionMask {
    fn default() -> Self {
        Self {
            motion: [0.0; BAND_COUNT],
            sampled: false,
            weights: ALL_BANDS,
        }
    }
}

impl MotionMask {
    /// Fold one frame pair into the band mask. `prev`/`cur` are the
    /// same-size profiles the matcher is about to compare.
    pub(super) fn observe(&mut self, prev: &RowProfile, cur: &RowProfile) {
        let height = cur.height as usize;
        if height == 0 || prev.height != cur.height || prev.stride != cur.stride {
            return;
        }
        // The middle 80 %: fixed navbars/footers at the edges are not
        // evidence of a static sidebar.
        let from = height / 10;
        let to = height - height / 10;
        if to <= from {
            return;
        }
        let motion = prev.band_change(cur, from, to);
        let strongest = motion.iter().copied().fold(0.0f32, f32::max);
        if strongest < 1.0 {
            return;
        }
        for (band, value) in motion.iter().copied().enumerate() {
            let relative = if value < 1.0 { 0.0 } else { value / strongest };
            self.motion[band] = if self.sampled {
                self.motion[band] * 0.5 + relative * 0.5
            } else {
                relative
            };
        }
        self.sampled = true;
        let weights = self
            .motion
            .map(|value| if value >= BAND_MOTION_FLOOR { 1.0 } else { 0.0 });
        let moving = weights.iter().filter(|&&w| w > 0.0).count();
        self.weights = if (MIN_MASKED_BANDS..BAND_COUNT).contains(&moving) {
            weights
        } else {
            ALL_BANDS
        };
    }
}
