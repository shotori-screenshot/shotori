//! # Scroll stitching: assembling a long screenshot from scrolled frames
//!
//! Pure logic half of the long-screenshot feature. The platform half (a
//! Wayland connection that injects wheel steps and re-captures the
//! selection) feeds this stitcher a stream of equal-sized RGBA frames —
//! whatever the viewport currently shows — and this module decides, per
//! frame, how the captured canvas must grow.
//!
//! Design notes (what was borrowed and why):
//! - **Viewport state machine** — the canvas is modeled as
//!   `viewport_height + max_position` rows, monotonic by construction:
//!   scrolling down appends, scrolling back up *prepends* (the canvas
//!   never shrinks — an up-scroll after a down-scroll must not lose the
//!   already-captured content). This survives overshoot, rubber-banding
//!   and manual up/down wiggling without any special cases. (Architecture
//!   proven by Snow Shot's `snow-stitch-images`; their crate is
//!   Apache-2.0 but carries an ORB estimator we do not need — we control
//!   the scroll, so offsets are small and a cheap matcher suffices.)
//! - **Column-sampled matching** — frames are reduced to grayscale
//!   row-means over three interior column bands (edges are excluded:
//!   scrollbars and fixed sidebars live there and would poison the
//!   comparison). The offset search is a mean-absolute-difference scan
//!   over those 1-D signals: O(bands × height) per candidate instead of
//!   a full-frame template match. (Popularized on Linux by
//!   wayscrollshot; validated by its five-algorithm shootout where
//!   column sampling was the speed/accuracy sweet spot for text pages.)
//! - **Rejection taxonomy** — a frame that cannot be placed is *rejected
//!   with a reason*, never force-fit: mid-animation frames (torn between
//!   two scroll positions), periodic textures (code listings, striped
//!   tables — multiple offsets fit equally well) and flat regions (pure
//!   white PDF margins carry no signal) all have distinct outcomes so
//!   the scroll loop can log, retry or stop intelligently.
//! - **Newest-wins canvas** — on every accepted frame the whole viewport
//!   rectangle is rewritten into the canvas (not just the new band).
//!   Where old and new overlap the fresh pixels win, so blinking carets
//!   and lazy-loading images leave the *latest* state in the output.
//! - **Anchor refresh on every non-duplicate frame** — a rejected frame
//!   still becomes the comparison anchor. Differencing a mid-animation
//!   tear against the last *stable* frame forever would never recover;
//!   riding through the change re-anchors on the settled content.
//!
//! Known v1 limitations (deliberate, see ROADMAP): integer offsets only
//! (sub-pixel smooth scrolling can drop/duplicate one seam row per
//! step), no sticky-header region classification (a fixed navbar is
//! duplicated along the seam — users should select below it), single
//! contiguous canvas (no tiling for multi-hundred-MB captures).

/// Tuning knobs for [`ScrollStitcher`]. Defaults are tuned for text
/// content under *our* auto-scroll (small, roughly known steps); manual
/// scrolling only widens the offset distribution, the thresholds hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StitchOptions {
    /// Offsets above this fraction of the viewport height are never
    /// considered — a frame that jumped that far either lost all overlap
    /// or matched a wrong periodic copy. 0.6 keeps ≥ 40 % overlap.
    pub max_motion_ratio: f32,
    /// Mean band-signal |Δ| (0–255 grayscale units) below which an offset
    /// counts as a match. 2.5 absorbs the ±1–2 shimmer of GPU-antialiased
    /// re-renders of identical content.
    pub cost_threshold: f32,
    /// The best offset is only accepted when the best *different* offset
    /// (outside the ±2-row correlation peak) is at least this much worse.
    /// This is the periodic-texture guard: a striped table matches at
    /// `d`, `d±period`, `d±2·period`… all with near-zero cost, and a
    /// margin test surfaces that ambiguity instead of guessing.
    pub ambiguity_margin: f32,
    /// Minimum rows of overlap required to trust any offset.
    pub min_overlap: u32,
    /// Hard cap on canvas height — the auto-scroll loop stops (and
    /// reports) when the stitched image would exceed it. Guards against
    /// runaway captures on infinite-scroll feeds.
    pub max_height: u32,
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self {
            max_motion_ratio: 0.6,
            cost_threshold: 2.5,
            ambiguity_margin: 1.5,
            min_overlap: 48,
            max_height: 20_000,
        }
    }
}

/// Why a frame could not be stitched. These reach the scroll loop (and
/// its logs) verbatim — the taxonomy is the observability of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StitchReject {
    /// Viewport smaller than the minimum workable size.
    TooSmall,
    /// No admissible offset exists at all: every candidate would leave
    /// less than [`StitchOptions::min_overlap`] rows of overlap.
    /// Unreachable with the constructor's size gate and default options —
    /// kept as the matcher's defensive floor for custom options.
    InsufficientOverlap,
    /// Multiple offsets fit equally well (periodic texture). Not
    /// force-fit; a later frame usually escapes the pattern.
    Ambiguous,
    /// The best offset is still a bad match — content changed shape
    /// (mid-animation tear, page navigation, video).
    HighResidual,
    /// Accepting the offset would grow the canvas past
    /// [`StitchOptions::max_height`].
    HeightLimit,
}

/// The verdict on one pushed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StitchOutcome {
    /// Viewport moved down past the captured extent; canvas grew at the
    /// bottom by `growth` rows (`dy` = detected offset magnitude).
    Appended { dy: u32, growth: u32 },
    /// Viewport moved up above the canvas top; canvas grew at the top.
    Prepended { dy: u32, growth: u32 },
    /// Viewport moved within the already-captured extent; canvas
    /// rewritten in place, no growth.
    Contained { dy: i32 },
    /// Byte-identical to the previous frame — the capture poll ran faster
    /// than the repaint. The anchor frame is kept (a duplicate must not
    /// become the new comparison reference).
    Duplicate,
    /// Pixels changed but nothing moved reliably (caret blink, hover
    /// effects). The frame becomes the new anchor; state is untouched.
    NoMotion,
    /// Frame could not be placed — see [`StitchReject`]. The frame still
    /// becomes the new anchor so the next comparison rides through the
    /// change instead of differencing against a stale frame.
    Rejected(StitchReject),
}

/// Which way the viewport left the already-captured extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    Append,
    Prepend,
    Contained,
}

/// The viewport's position inside the canvas coordinate space.
///
/// Invariants: `position ≤ max_position`, and the canvas height is
/// always `viewport_height + max_position` (i64 fields: the transition
/// math temporarily goes negative before clamping; canvas sizes stay
/// far below i64 range by the `max_height` cap).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ViewportState {
    position: i64,
    max_position: i64,
}

impl ViewportState {
    /// Where a viewport movement of `dy` rows (positive = down) lands.
    /// Returns the next state, the branch taken and the canvas growth in
    /// rows. Scrolling above the canvas top re-bases `position` to 0 and
    /// grows at the top; scrolling past the bottom grows at the bottom.
    fn transition(self, dy: i32) -> (ViewportState, Branch, u32) {
        let candidate = self.position + i64::from(dy);
        if candidate < 0 {
            // Lossless: |candidate| ≤ position + |dy| ≤ 2 × max_height,
            // and max_height is a u32 — the negative fits u32 comfortably.
            let growth = (-candidate) as u32;
            (
                Self {
                    position: 0,
                    max_position: self.max_position + i64::from(growth),
                },
                Branch::Prepend,
                growth,
            )
        } else if candidate > self.max_position {
            let growth = (candidate - self.max_position) as u32;
            (
                Self {
                    position: candidate,
                    max_position: candidate,
                },
                Branch::Append,
                growth,
            )
        } else {
            (
                Self {
                    position: candidate,
                    max_position: self.max_position,
                },
                Branch::Contained,
                0,
            )
        }
    }
}

/// A frame reduced for matching: **full grayscale rows** over the
/// interior columns [w/8, 7w/8) — edges are excluded (scrollbars and
/// fixed sidebars live there and would poison the comparison).
///
/// Row MEANS are deliberately NOT used: real text defeats them (measured
/// on a terminal — every line's glyph mix converges to nearly the same
/// mean, so a whole-line-height shift leaves the mean signal unchanged
/// and the matcher reports NoMotion at d = 0). Pixel rows keep the
/// per-line glyph detail that distinguishes offsets.
///
/// The cost scan walks a `step`-spaced grid of CUR-frame rows against
/// the FULL prev storage (arbitrary offsets stay comparable); step is
/// sized to keep a scan ≈ 100k byte-ops.
#[derive(Debug, Clone)]
struct RowProfile {
    height: u32,
    /// Interior width in bytes (one luma per pixel).
    stride: usize,
    /// Kept-curve row spacing for the cost scan.
    step: usize,
    /// `height` rows × `stride` luma bytes, row-major.
    data: Vec<u8>,
}

/// Samples compared per candidate offset, topside for the cost scan.
const SCAN_BUDGET: usize = 120_000;

impl RowProfile {
    /// Largest per-row interior mean spread. A frame whose rows are
    /// (near-)uniform carries no alignment information: every
    /// translation window looks identical — the conservative answer is
    /// NoMotion; a later textured frame brings something to match
    /// against. Flat PDF margins hit this.
    fn row_mean_spread(&self) -> u8 {
        let mut spread = 0u8;
        let mut prev: Option<u8> = None;
        for row in 0..self.height as usize {
            let line = &self.data[row * self.stride..(row + 1) * self.stride];
            let mean = (line.iter().copied().map(u32::from).sum::<u32>()
                / self.stride.max(1) as u32) as u8;
            if let Some(p) = prev {
                spread = spread.max(mean.abs_diff(p));
            }
            prev = Some(mean);
        }
        spread
    }
}

impl RowProfile {
    fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
        let w = width as usize;
        let interior = w / 8;
        let stride = w - 2 * interior;
        let stride = stride.max(1);
        let mut data = vec![0u8; height as usize * stride];
        if stride >= 1 {
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
        }
        // Grid spacing keeps one candidate scan ≈ SCAN_BUDGET byte ops.
        let step = (height as usize * stride / SCAN_BUDGET).clamp(1, 8);
        Self {
            height,
            stride,
            step,
            data,
        }
    }
}

/// The best offset found for a frame pair and the evidence for it.
struct OffsetEstimate {
    /// Viewport movement in rows, positive = down (canvas-append side).
    dy: i32,
    /// Mean |Δ| of the band signals at `dy` (grayscale units).
    cost: f32,
    /// Best cost among offsets outside the ±2-row peak around `dy`
    /// (`f32::INFINITY` when there is no such candidate).
    runner_up: f32,
}

/// Scan all admissible offsets for the best alignment between the anchor
/// frame's profile (`prev`) and the incoming frame's (`cur`).
///
/// With viewport movement `d` (positive = down), the current frame's row
/// `j` shows what the anchor showed at row `j + d` — scrolling down
/// brings content that sat lower in the viewport up to its top — so the
/// comparison window is the overlap `height − |d|`. Candidates are
/// capped at `height × max_motion_ratio` and must leave `min_overlap`
/// rows. The comparison is full-row pixel MAD on a step-spaced grid of
/// rows (see [`RowProfile`] for why means are not enough). Returns
/// [`None`] when no candidate is admissible at all.
fn estimate_offset(
    prev: &RowProfile,
    cur: &RowProfile,
    options: &StitchOptions,
) -> Option<OffsetEstimate> {
    let h = prev.height.min(cur.height) as i32;
    if h <= 0 || prev.stride != cur.stride {
        return None;
    }
    let stride = cur.stride;
    let max_motion = ((h as f32) * options.max_motion_ratio) as i32;
    let max_admissible = max_motion.min(h - options.min_overlap as i32);
    if max_admissible < 0 {
        return None;
    }

    let cost = |d: i32| -> f32 {
        let overlap_top = h - d.max(0); // exclusive upper bound on cur rows
        let first = ((-d).max(0) as usize).div_ceil(cur.step) * cur.step;
        let mut sum = 0u64;
        let mut compared = 0usize;
        let mut j = first;
        while j < overlap_top as usize {
            let c = &cur.data[j * stride..(j + 1) * stride];
            let pj = (j as i32 + d) as usize;
            let p = &prev.data[pj * stride..(pj + 1) * stride];
            sum += c
                .iter()
                .zip(p.iter())
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum::<u64>();
            compared += stride;
            j += cur.step;
        }
        if compared == 0 {
            f32::INFINITY
        } else {
            sum as f32 / compared as f32
        }
    };

    // Collect every admissible candidate, then pick the best and the best
    // non-local one in one sweep — cheaper to reason about than a
    // streaming top-2, and the candidate count (≤ viewport height) is
    // small. d = 0 is scanned first so flat-content ties resolve to
    // NoMotion rather than a phantom offset.
    let mut candidates: Vec<(i32, f32)> = Vec::with_capacity(max_admissible as usize * 2 + 1);
    for d in 0..=max_admissible {
        candidates.push((d, cost(d)));
        if d > 0 {
            candidates.push((-d, cost(-d)));
        }
    }
    let mut best: Option<(i32, f32)> = None;
    for &(d, c) in &candidates {
        if best.is_none_or(|(_, bc)| c < bc) {
            best = Some((d, c));
        }
    }
    let (best_dy, best_cost) = best?;
    let runner_up = candidates
        .iter()
        .filter(|(d, _)| (*d - best_dy).abs() > 2)
        .map(|(_, c)| *c)
        .min_by(f32::total_cmp)
        .unwrap_or(f32::INFINITY);
    Some(OffsetEstimate {
        dy: best_dy,
        cost: best_cost,
        runner_up,
    })
}

/// Incremental long-screenshot assembler. Feed it the first viewport
/// frame at construction, then every subsequent capture of the same
/// rectangle; read the stitched image from [`ScrollStitcher::canvas`].
pub(crate) struct ScrollStitcher {
    width: u32,
    viewport_height: u32,
    options: StitchOptions,
    /// The stitched image so far: RGBA, `height()` rows of `width` px.
    canvas: Vec<u8>,
    state: ViewportState,
    /// The last frame used as comparison anchor (raw bytes — the exact
    /// duplicate test memcmp's it).
    anchor_raw: Vec<u8>,
    anchor_profile: RowProfile,
    /// Diagnostics for the scroll loop's logs / progress UI.
    frames_pushed: usize,
    frames_accepted: usize,
}

impl ScrollStitcher {
    /// Start a stitch from the viewport's current content. Fails with
    /// [`StitchReject::TooSmall`] when the selection is too small to
    /// ever match (the height must clear the overlap budget with room
    /// for motion).
    pub(crate) fn new(
        width: u32,
        viewport_height: u32,
        first_frame: &[u8],
        options: StitchOptions,
    ) -> Result<Self, StitchReject> {
        if width < 16 || viewport_height <= options.min_overlap + 32 {
            return Err(StitchReject::TooSmall);
        }
        debug_assert_eq!(
            first_frame.len(),
            width as usize * viewport_height as usize * 4,
            "frame size must match the declared viewport"
        );
        let profile = RowProfile::from_rgba(first_frame, width, viewport_height);
        Ok(Self {
            width,
            viewport_height,
            options,
            canvas: first_frame.to_vec(),
            state: ViewportState {
                position: 0,
                max_position: 0,
            },
            anchor_raw: first_frame.to_vec(),
            anchor_profile: profile,
            frames_pushed: 1,
            frames_accepted: 1,
        })
    }

    fn frame_bytes(&self) -> usize {
        self.width as usize * self.viewport_height as usize * 4
    }

    fn height(&self) -> u32 {
        (i64::from(self.viewport_height) + self.state.max_position) as u32
    }

    /// The stitched image so far (RGBA, `width` × `height()`).
    pub(crate) fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height())
    }

    /// How many rows the viewport has advanced beyond the initial view —
    /// the number a progress UI reports as "captured".
    pub(crate) fn captured_extent(&self) -> u32 {
        self.state.max_position as u32
    }

    /// (frames pushed, frames folded into the canvas) — health signals
    /// for the scroll loop's logs.
    pub(crate) fn stats(&self) -> (usize, usize) {
        (self.frames_pushed, self.frames_accepted)
    }

    /// Judge one capture of the selection rectangle (same size as the
    /// first frame) and fold it into the canvas.
    pub(crate) fn push(&mut self, frame: &[u8]) -> StitchOutcome {
        self.frames_pushed += 1;
        debug_assert_eq!(
            frame.len(),
            self.frame_bytes(),
            "frame size must match the declared viewport"
        );
        if frame == self.anchor_raw.as_slice() {
            // Poll outran the repaint: not even a NoMotion — the anchor
            // must stay so the next real frame diffs against fresh bytes.
            return StitchOutcome::Duplicate;
        }

        let profile = RowProfile::from_rgba(frame, self.width, self.viewport_height);
        let outcome = self.place(frame, &profile);
        // Every non-duplicate frame becomes the new anchor, including
        // rejected ones — see the module docs ("anchor refresh").
        self.anchor_raw.clear();
        self.anchor_raw.extend_from_slice(frame);
        self.anchor_profile = profile;
        outcome
    }

    fn place(&mut self, frame: &[u8], profile: &RowProfile) -> StitchOutcome {
        // Low-information guard (Snow Shot's "low-information" stage): a
        // near-uniform anchor cannot be aligned by any translation — the
        // conservative answer is NoMotion; a later textured frame brings
        // something to match against. Flat PDF margins hit this.
        if self.anchor_profile.row_mean_spread() <= 1 {
            return StitchOutcome::NoMotion;
        }
        let Some(estimate) = estimate_offset(&self.anchor_profile, profile, &self.options) else {
            return StitchOutcome::Rejected(StitchReject::InsufficientOverlap);
        };
        if estimate.cost > self.options.cost_threshold {
            // Nothing aligns: the frame is torn between two positions
            // (animation) or the content itself changed (navigation).
            return StitchOutcome::Rejected(StitchReject::HighResidual);
        }
        if estimate.dy == 0 {
            return StitchOutcome::NoMotion;
        }
        if estimate.runner_up - estimate.cost < self.options.ambiguity_margin {
            return StitchOutcome::Rejected(StitchReject::Ambiguous);
        }

        let (next, branch, growth) = self.state.transition(estimate.dy);
        if self.viewport_height as u64 + next.max_position as u64 > self.options.max_height as u64 {
            return StitchOutcome::Rejected(StitchReject::HeightLimit);
        }

        let row = self.width as usize * 4;
        let frame_bytes = self.frame_bytes();
        if branch == Branch::Prepend && growth > 0 {
            // Grow at the top: shift the existing rows down (memmove
            // semantics — copy_within handles the overlap), leaving
            // zeroed rows above for the viewport write.
            let shift = growth as usize * row;
            self.canvas.resize(self.canvas.len() + shift, 0);
            let len = self.canvas.len();
            self.canvas.copy_within(0..len - shift, shift);
        } else if branch == Branch::Append && growth > 0 {
            self.canvas
                .resize(self.canvas.len() + growth as usize * row, 0);
        }
        // Newest-wins: rewrite the whole viewport rectangle, not just the
        // new band (see the module docs). `next.position` is the new
        // viewport top in canvas rows for every branch.
        let dst = next.position as usize * row;
        self.canvas[dst..dst + frame_bytes].copy_from_slice(frame);
        self.state = next;
        self.frames_accepted += 1;
        match branch {
            Branch::Append => StitchOutcome::Appended {
                dy: estimate.dy.unsigned_abs(),
                growth,
            },
            Branch::Prepend => StitchOutcome::Prepended {
                dy: estimate.dy.unsigned_abs(),
                growth,
            },
            Branch::Contained => StitchOutcome::Contained { dy: estimate.dy },
        }
    }
}

/// Box-filter downscale of a canvas TAIL for the live preview panel:
/// integer factor `k = ceil(width / target_w)` keeps the aspect, and
/// only the last `max_rows` canvas rows are included (the panel watches
/// the growing edge). Returns `(width, height, rgba)`.
pub fn preview_tail(
    rgba: &[u8],
    width: u32,
    height: u32,
    target_w: u32,
    max_rows: u32,
) -> (u32, u32, Vec<u8>) {
    let k = ((width as f32) / target_w.max(1) as f32).ceil().max(1.0) as u32;
    let pw = width.div_ceil(k);
    let src_rows = (height.min(max_rows.saturating_mul(k))).max(1);
    let y0 = height.saturating_sub(src_rows);
    let ph = src_rows.div_ceil(k);
    let mut out = vec![0u8; (pw * ph * 4) as usize];
    for py in 0..ph {
        for px_ in 0..pw {
            let x1 = ((px_ + 1) * k).min(width);
            let ry0 = y0 + py * k;
            let ry1 = (ry0 + k).min(height);
            let (mut acc, mut n) = ([0u32; 4], 0u32);
            for y in ry0..ry1 {
                let mut i = ((y * width + px_ * k) * 4) as usize;
                for _ in px_ * k..x1 {
                    for c in 0..4 {
                        acc[c] += u32::from(rgba[i + c]);
                    }
                    i += 4;
                    n += 1;
                }
            }
            let o = ((py * pw + px_) * 4) as usize;
            if n > 0
                && let Some(bytes) = out.get_mut(o..o + 4)
            {
                for (c, b) in bytes.iter_mut().enumerate() {
                    *b = (acc[c] / n) as u8;
                }
            }
        }
    }
    (pw, ph, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 64;
    const H: u32 = 96;

    /// Deterministic aperiodic row color: a bit-mix of the row index.
    /// Hash-y rows give every band signal texture with no periodicity,
    /// which is exactly what the matcher needs and what the periodic
    /// test must avoid.
    fn source_row(row: u32) -> [u8; 4] {
        let mut x = row.wrapping_mul(2654435761).wrapping_add(0x9e3779b9);
        x ^= x >> 15;
        x = x.wrapping_mul(2246822519);
        x ^= x >> 13;
        [
            (x & 0xff) as u8,
            ((x >> 8) & 0xff) as u8,
            ((x >> 16) & 0xff) as u8,
            0xff,
        ]
    }

    /// The viewport at `top` over an infinite deterministic source page.
    fn frame_at(top: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((W * H * 4) as usize);
        for row in top..top + H {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    }

    fn stitcher() -> ScrollStitcher {
        ScrollStitcher::new(W, H, &frame_at(0), StitchOptions::default()).unwrap()
    }

    /// The expected canvas after a walk that visited viewport tops
    /// `first..=last` (rows `first..last + H` of the source page).
    fn expected_canvas(first: u32, last: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(((last + H - first) * W * 4) as usize);
        for row in first..last + H {
            v.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        v
    }

    #[test]
    fn preview_tail_downscales_and_tails() {
        // A 8×8 canvas of two solid halves: rows 0..4 red, rows 4..8
        // blue. Downscaled to target 2 → k = 4 → 2 preview columns;
        // tail limited to the last 4 canvas rows (tails are always
        // k-round) → all blue.
        let (w, h) = (8u32, 8u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let color = if y < 4 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                rgba[i..i + 4].copy_from_slice(&color);
            }
        }
        let (pw, ph, out) = preview_tail(&rgba, w, h, 2, 1); // max_rows=1, k=4 → last 4 rows
        assert_eq!((pw, ph), (2, 1));
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| p == &[0, 0, 255, 255]),
            "tail must be the blue half, got {out:?}"
        );

        // Without the tail cap the whole canvas is covered: the first
        // preview row averages the red half, the second the blue half.
        let (pw2, ph2, out2) = preview_tail(&rgba, w, h, 2, 100);
        assert_eq!((pw2, ph2), (2, 2));
        let blocks = out2.as_chunks::<4>().0;
        assert!(blocks.first().is_some_and(|p| p[0] == 255 && p[2] == 0));
        assert!(blocks.last().is_some_and(|p| p[0] == 0 && p[2] == 255));
    }

    #[test]
    fn state_machine_takes_each_normative_branch() {
        let s = ViewportState::default();
        let (s, b, g) = s.transition(10);
        assert_eq!((b, g), (Branch::Append, 10));
        assert_eq!((s.position, s.max_position), (10, 10));

        // Back up inside the captured extent: contained, zero growth.
        let (s2, b2, g2) = s.transition(-4);
        assert_eq!((b2, g2), (Branch::Contained, 0));
        assert_eq!((s2.position, s2.max_position), (6, 10));

        // Above the canvas top: prepend grows without losing height.
        let (s3, b3, g3) = s2.transition(-12);
        assert_eq!((b3, g3), (Branch::Prepend, 6));
        assert_eq!((s3.position, s3.max_position), (0, 16));
    }

    #[test]
    fn first_frame_seeds_the_canvas() {
        let s = stitcher();
        assert_eq!(s.dimensions(), (W, H));
        assert_eq!(s.canvas(), frame_at(0).as_slice());
        assert_eq!(s.captured_extent(), 0);
    }

    #[test]
    fn tiny_viewports_are_rejected_upfront() {
        let frame = vec![0u8; W as usize * 60 * 4];
        assert_eq!(
            ScrollStitcher::new(W, 60, &frame, StitchOptions::default()).err(),
            Some(StitchReject::TooSmall)
        );
    }

    #[test]
    fn scroll_down_appends_and_reconstructs_exactly() {
        let mut s = stitcher();
        for top in [24u32, 24, 61, 95] {
            let out = s.push(&frame_at(top));
            // The dwell at 24 duplicates — skip asserting it here; the
            // duplicate path has its own test.
            if top != 24 {
                assert!(matches!(out, StitchOutcome::Appended { .. }), "{out:?}");
            }
        }
        assert_eq!(s.dimensions(), (W, 95 + H));
        assert_eq!(s.canvas(), expected_canvas(0, 95).as_slice());
    }

    #[test]
    fn duplicate_poll_frames_are_skipped_without_disturbing_state() {
        let mut s = stitcher();
        assert_eq!(s.push(&frame_at(0)), StitchOutcome::Duplicate);
        assert_eq!(s.push(&frame_at(0)), StitchOutcome::Duplicate);
        // The anchor is still the ORIGINAL frame: scrolling after the
        // duplicates must match against it, not a replaced copy.
        s.push(&frame_at(30));
        assert_eq!(s.dimensions(), (W, 30 + H));
    }

    #[test]
    fn scroll_up_prepends_recovering_content_above_the_start() {
        // Start mid-page, scroll down, then back up past the start.
        let mut s = ScrollStitcher::new(W, H, &frame_at(100), StitchOptions::default()).unwrap();
        s.push(&frame_at(130));
        let out = s.push(&frame_at(90));
        assert!(
            matches!(out, StitchOutcome::Prepended { growth: 10, .. }),
            "{out:?}"
        );
        // Canvas covers source rows 90..=130+H.
        assert_eq!(s.canvas(), expected_canvas(90, 130).as_slice());
        assert_eq!(s.dimensions(), (W, 40 + H));
    }

    #[test]
    fn up_down_wiggle_is_contained_without_growth() {
        let mut s = stitcher();
        s.push(&frame_at(40));
        let height_after_append = s.height();
        let out = s.push(&frame_at(15));
        assert!(
            matches!(out, StitchOutcome::Contained { dy: -25 }),
            "{out:?}"
        );
        assert_eq!(s.height(), height_after_append);
        // Rewriting in place keeps the canvas consistent with the source.
        assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
    }

    #[test]
    fn one_pixel_change_reports_no_motion_and_keeps_anchor_fresh() {
        let mut frame = frame_at(0);
        // Column 10 sits inside band 0 (w/8..): a real signal change in
        // the G channel, the caret-blink stand-in.
        frame[10 * 4 + 1] ^= 0xff;
        let mut s = stitcher();
        assert_eq!(s.push(&frame), StitchOutcome::NoMotion);
        assert_eq!(s.dimensions(), (W, H));
        // The changed frame became the anchor: a scroll away from HERE
        // (not from the pre-blink frame) must still match.
        let out = s.push(&frame_at(20));
        assert!(
            matches!(out, StitchOutcome::Appended { growth: 20, .. }),
            "{out:?}"
        );
    }

    #[test]
    fn periodic_texture_is_ambiguous_not_guessy() {
        // Repeated *similar* content: every 48 rows the page repeats the
        // same block, with adjacent copies differing by exactly one luma
        // step (the stand-in for list items that look alike). A pure
        // period with an exact-period offset would make the two frames'
        // signals identical at dy 0 — indistinguishable from no motion —
        // so the luma drift is what keeps d = 0 a *bad* match (cost 1.0)
        // while d = 48 is a perfect one (cost 0). Another offset (d = 0)
        // then fits nearly as well as the best, and the margin test must
        // refuse to choose.
        let similar_page = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H * 4) as usize);
            for row in top..top + H {
                let mut px = source_row(row % 48);
                let parity = ((row / 48) % 2) as u8;
                for c in &mut px[..3] {
                    *c = c.saturating_add(parity);
                }
                v.extend_from_slice(&px.repeat(W as usize));
            }
            v
        };
        let mut s = ScrollStitcher::new(W, H, &similar_page(0), StitchOptions::default()).unwrap();
        assert_eq!(
            s.push(&similar_page(48)),
            StitchOutcome::Rejected(StitchReject::Ambiguous)
        );
        // The canvas is untouched by the rejection.
        assert_eq!(s.dimensions(), (W, H));
    }

    #[test]
    fn flat_content_reports_no_motion() {
        let flat = || vec![0xff; (W * H * 4) as usize];
        let mut changed = flat();
        changed[10 * 4 + 1] = 0x00;
        let mut s = ScrollStitcher::new(W, H, &flat(), StitchOptions::default()).unwrap();
        // A flat frame "matches" every offset at ~zero cost — dy 0 wins
        // by scan order, and zero must not demand an ambiguity margin.
        assert_eq!(s.push(&changed), StitchOutcome::NoMotion);
    }

    #[test]
    fn torn_animation_frames_reject_until_a_clean_pair_recovers() {
        // A capture mid-smooth-scroll: the top half still shows the old
        // position, the bottom half the new one. Nothing aligns.
        let old = frame_at(0);
        let settled = frame_at(20);
        let torn: Vec<u8> = [
            old[..old.len() / 2].to_vec(),
            settled[settled.len() / 2..].to_vec(),
        ]
        .concat();
        let mut s = stitcher();
        assert_eq!(
            s.push(&torn),
            StitchOutcome::Rejected(StitchReject::HighResidual)
        );
        // The settled frame still differs from the torn anchor too much
        // (half of it matched the OTHER position) — also rejected, and
        // becomes the new anchor.
        assert_eq!(
            s.push(&settled),
            StitchOutcome::Rejected(StitchReject::HighResidual)
        );
        // The next clean step now matches against the settled anchor:
        // frame(40) sits 20 rows below frame(20), so the canvas grows by
        // 20 (not 40 — the rejected frames contributed nothing). Known
        // cost of riding through a tear: the 20-row band only the
        // rejected frames showed (page rows 20..40) is lost for good;
        // the frame-40 write covers the seam with newer content.
        let out = s.push(&frame_at(40));
        assert!(
            matches!(out, StitchOutcome::Appended { dy: 20, growth: 20 }),
            "{out:?}"
        );
        assert_eq!(s.dimensions(), (W, H + 20));
        let mut expected = expected_canvas(0, 0)[..20 * W as usize * 4].to_vec();
        expected.extend_from_slice(&expected_canvas(40, 40));
        assert_eq!(s.canvas(), expected.as_slice());
    }

    #[test]
    fn oversized_jump_is_high_residual() {
        let mut s = stitcher();
        // 0.95 × H is beyond the admissible search range, so the best
        // in-range candidate is garbage — surfaced as HighResidual
        // (indistinguishable from a scene change by cost alone, which is
        // fine: the scroll loop reacts the same way).
        let out = s.push(&frame_at((H as f32 * 0.95) as u32));
        assert_eq!(out, StitchOutcome::Rejected(StitchReject::HighResidual));
    }

    #[test]
    fn height_cap_stops_the_stitcher() {
        let options = StitchOptions {
            max_height: H + 60,
            ..StitchOptions::default()
        };
        let mut s = ScrollStitcher::new(W, H, &frame_at(0), options).unwrap();
        s.push(&frame_at(30));
        s.push(&frame_at(60)); // reaches the cap exactly
        assert_eq!(s.dimensions(), (W, H + 60));
        assert_eq!(
            s.push(&frame_at(90)),
            StitchOutcome::Rejected(StitchReject::HeightLimit)
        );
        assert_eq!(s.dimensions(), (W, H + 60));
    }

    #[test]
    fn random_walk_reconstructs_the_visited_page() {
        // The invariant test: a randomized (but deterministic) scroll
        // session — monotone-ish drift with dwells, small up-wiggles and
        // re-visits — must reconstruct exactly the source rows between
        // the lowest and highest viewport tops ever seen.
        let mut seed = 0x1234_5678_u64;
        let mut rand = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };

        let mut top = 500u32;
        let mut s = ScrollStitcher::new(W, H, &frame_at(top), StitchOptions::default()).unwrap();
        let (mut lo, mut hi) = (top, top);
        for _ in 0..200 {
            // Steps stay inside the matcher's admissible window
            // (≤ 40 down / 30 up, overlap ≥ 56 rows) — auto-scroll runs
            // smaller steps than this even at maximum wheel speed.
            let delta = match rand() % 10 {
                0..=1 => 0, // dwell (poll duplicate)
                2..=3 => -i64::from(rand() % 20 + 1),
                _ => i64::from(rand() % 30 + 5),
            }
            .clamp(-30, 40);
            let next = (i64::from(top) + delta).max(0) as u32;
            let out = s.push(&frame_at(next));
            assert!(
                !matches!(out, StitchOutcome::Rejected(_)),
                "walk rejected at {top} → {next}: {out:?}"
            );
            top = next;
            lo = lo.min(top);
            hi = hi.max(top);
        }
        assert_eq!(s.dimensions(), (W, hi - lo + H));
        assert_eq!(s.canvas(), expected_canvas(lo, hi).as_slice());
        let (pushed, accepted) = s.stats();
        assert_eq!(pushed, 201);
        assert!(accepted >= 2);
    }
}
