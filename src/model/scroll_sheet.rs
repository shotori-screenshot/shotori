//! # Scroll sheet: assembling one long image from scrolled frames
//!
//! The pure half of long screenshots. The platform half (the capture
//! engine) hands this module equal-sized RGBA frames in the order the
//! user scrolled them; every question about *where* the next frame
//! belongs is answered here, with no compositor, clock or socket in
//! sight — which is what makes the whole thing testable.
//!
//! The vocabulary is ours on purpose. We call the growing image a
//! **sheet**: frames are *folded* into it, a frame that cannot be placed
//! is *refused* with a reason instead of being forced in. Two ideas in
//! here were learned from other projects and are credited where they
//! live; everything else is written against our own capture loop.
//!
//! Design notes (why it is shaped this way):
//!
//! - **Viewport state machine** — the sheet is
//!   `viewport_height + max_position` rows, monotonic by construction:
//!   scrolling down grows the bottom, scrolling back up grows the TOP
//!   (the sheet never shrinks — an up-scroll after a down-scroll must not
//!   delete content the user already passed). Overshoot, rubber-banding
//!   and up/down wiggling all fall out of that one transition instead of
//!   needing special cases.
//! - **Two-stage matching** — offset *ranking* uses eight narrow luma
//!   bands per row (≤ 9 samples per band, so ~72 floats per row no
//!   matter how wide the selection is), searched around the last
//!   accepted step, then a step-3 sweep of the whole admissible range and
//!   a ±3 refinement around the three best basins. Only the twelve best
//!   offsets that are more than 2 rows apart get verified with the
//!   full-row luma MAD that
//!   decides the match (row means alone collapse on text — see
//!   [`RowProfile`] — so the band means rank, they never decide). An
//!   exhaustive scan cost one full-row pass per candidate; this is ~1/20
//!   of that per frame, which is what lets the capture loop keep up with
//!   a human scroll.
//! - **Fixed-column mask** — consecutive band means also identify column
//!   strips that do not move while the page does (PDF thumbnail panes,
//!   documentation outlines, a media player's controls). 3–7 moving
//!   bands means the rest is fixed chrome: those bands leave both the
//!   ranking score and the full-row verification, so an independently
//!   painted sidebar can no longer refuse every frame.
//! - **Refusal taxonomy** — a frame that cannot be placed is refused
//!   *with a reason*, never force-fit: mid-animation frames (torn between
//!   two scroll positions), periodic textures (code listings, striped
//!   tables — several offsets fit equally well) and flat regions (pure
//!   white PDF margins carry no signal) each get their own verdict, so
//!   the capture loop can say "no overlap, keep going" instead of dying.
//!   The loop treats a refusal as a state, not an error; see
//!   [`Refusal`].
//! - **Fixed-band detection** — fixed chrome inside the selection (site
//!   navbars that pin on scroll) breaks the row-translation assumption;
//!   since the static edge trim only survives small bands, each accepted
//!   moving frame also votes per sampled row ("still at the same viewport
//!   row while the page moved"), and a persistent structured prefix
//!   commits as a band: masked out of matching, and written ONCE at the
//!   sheet top — pinned chrome must not repeat down the long image. See
//!   [`StickyBands`].
//! - **Newest-wins sheet** — on every accepted frame the whole viewport
//!   rectangle is rewritten into the sheet (not just the new band). Where
//!   old and new overlap the fresh pixels win, so blinking carets and
//!   lazy-loading images leave the *latest* state in the output.
//! - **Anchor refresh on every non-duplicate frame** — a refused frame
//!   still becomes the comparison anchor. Differencing a mid-animation
//!   tear against the last *stable* frame forever would never recover;
//!   riding through the change re-anchors on the settled content.
//! - **Dual anchor** — the riding anchor above is only the FALLBACK base.
//!   Matching happens against the last frame ACCEPTED into the sheet
//!   first: a riding anchor sits at a position the sheet never advanced
//!   to, so a dy measured from it silently drops the band between the
//!   last accept and the anchor (the "lost band" that left the long image
//!   shorter than the real page). The stable base recovers the full
//!   displacement whenever the step fits inside the matcher's window —
//!   which every real scroll step does.
//!
//! Fixed bottom chrome (a site footer/status bar) follows the same
//! evidence as the top band, but with a simpler sheet rule: once
//! committed it leaves the content write entirely and is painted ONLY
//! while growing at the bottom. Scrolling back up therefore cannot stamp
//! the footer into the middle of the long image, and the footer appears
//! exactly once in the output. (The newest-wins write overwrites the
//! previous bottom copy with fresh content on the way down, so no extra
//! surgery is needed.)
//!
//! Known limitations (deliberate): integer offsets only (sub-pixel smooth
//! scrolling can drop/duplicate one seam row per step), fixed elements
//! in the middle of the frame (an in-table sticky header) are not
//! detected (only edge-anchored bands are), collapsing toolbars keep
//! their tallest extent (rows a shrunken band exposes are lost, not
//! misplaced), single contiguous sheet (no tiling for multi-hundred-MB
//! captures).

/// Tuning knobs for [`ScrollSheet`]. Defaults are tuned for text
/// content under *our* auto-scroll (small, roughly known steps); manual
/// scrolling only widens the offset distribution, the thresholds hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetLimits {
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
    /// reports) when the sheet would exceed it. Guards against
    /// runaway captures on infinite-scroll feeds.
    pub max_height: u32,
}

impl Default for SheetLimits {
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

/// The direction the long image grows in. Folding itself always runs
/// on row translation: horizontal sessions transpose every incoming
/// frame on the way in and transpose the finished canvas on the way out.
/// This keeps one matcher and one set of tests for both axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

/// Flip an RGBA buffer across its diagonal: `(w, h)` → `(h, w)`.
/// Pixel-perfect, one 4-byte copy per pixel — the seam is never resampled.
fn transpose_rgba(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    debug_assert_eq!(rgba.len(), w * h * 4);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            // Original (x, y) lands at (y, x) in the transposed buffer;
            // its row width is the original height.
            let dst = (x * h + y) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    out
}

/// Why a frame could not be folded. These reach the scroll loop (and
/// its logs) verbatim — the taxonomy is the observability of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Viewport smaller than the minimum workable size.
    TooSmall,
    /// No admissible offset exists at all: every candidate would leave
    /// less than [`SheetLimits::min_overlap`] rows of overlap.
    /// Unreachable with the constructor's size gate and default options —
    /// kept as the matcher's defensive floor for custom options.
    NoOverlap,
    /// Multiple offsets fit equally well (periodic texture). Not
    /// force-fit; a later frame usually escapes the pattern.
    Unclear,
    /// The best offset is still a bad match — content changed shape
    /// (mid-animation tear, page navigation, video).
    Mismatch,
    /// Accepting the offset would grow the canvas past
    /// [`SheetLimits::max_height`].
    TooTall,
}

/// The verdict on one pushed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fold {
    /// Viewport moved down past the captured extent; canvas grew at the
    /// bottom by `growth` rows (`dy` = detected offset magnitude).
    GrewBottom { dy: u32, growth: u32 },
    /// Viewport moved up above the canvas top; canvas grew at the top.
    GrewTop { dy: u32, growth: u32 },
    /// Viewport moved within the already-captured extent; canvas
    /// rewritten in place, no growth.
    Shifted { dy: i32 },
    /// Byte-identical to the previous frame — the capture poll ran faster
    /// than the repaint. The anchor frame is kept (a duplicate must not
    /// become the new comparison reference).
    Unchanged,
    /// Pixels changed but nothing moved reliably (caret blink, hover
    /// effects). The frame becomes the new anchor; state is untouched.
    Still,
    /// Frame could not be placed — see [`Refusal`]. The frame still
    /// becomes the new anchor so the next comparison rides through the
    /// change instead of differencing against a stale frame.
    Refused(Refusal),
}

/// Which way the viewport left the already-captured extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    Below,
    Above,
    Within,
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
                Branch::Above,
                growth,
            )
        } else if candidate > self.max_position {
            let growth = (candidate - self.max_position) as u32;
            (
                Self {
                    position: candidate,
                    max_position: candidate,
                },
                Branch::Below,
                growth,
            )
        } else {
            (
                Self {
                    position: candidate,
                    max_position: self.max_position,
                },
                Branch::Within,
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
/// and the matcher reports Still at d = 0). Pixel rows keep the
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
    /// Eight narrow sample bands inside `stride`, as `[start, end)` column
    /// ranges. Used by the cheap candidate search and the sidebar mask.
    bands: [(usize, usize); BAND_COUNT],
    /// Per-row band means: the cheap signal the offset search ranks
    /// candidates with. Full-row pixel MAD still decides the winner.
    /// Cell `[row][band]` is 0–255 luma.
    band_means: Vec<[f32; BAND_COUNT]>,
    /// Largest per-row interior mean spread, precomputed once. A frame
    /// whose rows are (near-)uniform carries no alignment information:
    /// every translation window looks identical — the conservative
    /// answer is Still; a later textured frame brings something to
    /// match against. Flat PDF margins hit this.
    spread: u8,
}

/// Samples compared per candidate offset, topside for the cost scan.
const SCAN_BUDGET: usize = 120_000;
/// Narrow bands sampled across the interior: cheap per-row signal for
/// candidate ranking and the fixed-column motion mask. Eight thin strips
/// retain per-glyph detail that full-row means lose on text, at ~1/stride
/// the cost.
const BAND_COUNT: usize = 8;
/// A band whose relative frame-to-frame change stays below this is fixed
/// chrome, not scrolling content.
const BAND_MOTION_FLOOR: f32 = 0.10;
/// 3–7 moving bands means "some bands are fixed chrome". Fewer than 3
/// is too little signal to trust the mask.
const MIN_MASKED_BANDS: usize = 3;

type BandWeights = [f32; BAND_COUNT];
const ALL_BANDS: BandWeights = [1.0; BAND_COUNT];

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
    fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
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
        Self {
            height,
            stride,
            step,
            data,
            bands,
            band_means,
            spread,
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
struct MotionMask {
    /// Exponentially smoothed relative motion per band.
    motion: [f32; BAND_COUNT],
    sampled: bool,
    /// 0.0 masks a band out of both the candidate score and the
    /// full-row verification; 1.0 keeps it.
    weights: BandWeights,
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
    fn observe(&mut self, prev: &RowProfile, cur: &RowProfile) {
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

/// Sticky-band detection: fixed chrome inside the selection (site
/// navbars that pin on scroll, floating toolbars) violates the
/// row-translation assumption the matcher stands on. The static edge
/// trim (~6 %) only survives small bands and only for MATCHING — the
/// band pixels still land in the canvas at every viewport position
/// ("a fixed navbar is duplicated along the seam"). This adds the
/// visual detection layer:
///
/// - **Evidence** — on every accepted frame with dy ≠ 0 (a MOVING page
///   is what makes "still at the same viewport row" mean "fixed
///   chrome" instead of "nothing scrolled"), each sampled row that is
///   near-identical to the comparison base at offset 0 accumulates a
///   stillness run. Content rows cannot stay identical while the page
///   moves under them.
/// - **Commit** — a run of [`STILL_FRAMES`] over a top-anchored prefix
///   of rows, backed by at least one structured row (a band that is
///   uniformly flat is indistinguishable from flat CONTENT — pure-white
///   margins must not get pinned), commits the band.
/// - **Canvas semantics** — a committed top band is written ONCE, from
///   the committing frame, and pinned at the canvas top: the band covers
///   only the content that does NOT track with scrolling, so a second
///   copy down the image could only be a duplicate of chrome. During the
///   detection delay the band rows were still being written as content at each
///   viewport position; committing surgically removes those duplicate
///   segments (tracked in `pending`) so the content stream stays exact.
/// - **Bottom band** — same evidence, a simpler canvas rule: once
///   committed the trailing rows leave BOTH the match scan and the
///   normal content write. The footer is painted only on appends, at
///   the growing bottom; an up-scroll writes content over the old
///   bottom copy and never stamps the footer mid-image. No splice is
///   needed because the newest-wins write already cleans the previous
///   bottom copy on the way down.
///
/// Collapsing toolbars (the band SHRINKS after pinning) are handled by
/// never shrinking the committed height: the band area keeps its
/// tallest extent and each frame's live prefix is bottom-aligned into
/// it, so the pin line — the only row where band meets content — stays
/// pixel-continuous.
const STILL_FRAMES: u16 = 2;
/// A committed band must span at least this many sampled rows — noise
/// (a caret row, a one-row border) must not pin.
const MIN_BAND_GRIDS: usize = 2;
/// Bands taller than a quarter of the viewport are content, not chrome
/// (the matcher's overlap budget could not survive them anyway).
const MAX_BAND_FRACTION: f32 = 0.25;
/// Luma spread within a sampled row for it to count as "structured" —
/// the band-level flat-content guard (see above).
const STRUCTURED_SPREAD: u8 = 4;

/// Sticky-band state, in viewport (frame) coordinates.
#[derive(Debug, Clone)]
struct StickyBands {
    /// Committed top band height in canvas rows. Monotone; 0 until a
    /// band is committed.
    top: u32,
    /// Rows the commit surgeries spliced into the canvas — the part of
    /// the physical height that is band, not content (a band pinned
    /// from the first frame reuses the seed frame's rows and never
    /// counts here). `height() = viewport_height + inserted +
    /// max_position`.
    inserted: u32,
    /// Committed bottom band height in rows. Unlike the top band this
    /// needs no spliced rows: it is excluded from content writes and
    /// painted at the growing bottom on appends only (see the module
    /// notes).
    bot: u32,
    /// Consecutive still frames per sampled row (top tracking), one
    /// entry per `step`-spaced row of the viewport.
    run: Vec<u16>,
    /// Same, counted from the bottom edge upward.
    bot_run: Vec<u16>,
    /// Content positions of the frames processed since the top prefix
    /// went still — the raw material for the commit surgery's duplicate
    /// segments. Reset whenever the prefix breaks (a row moved again)
    /// or the walk stops being monotonically downward (a Shifted or
    /// Prepend frame interleaves duplicated content in ways the segment
    /// math does not model; detection simply restarts).
    pending: Vec<i64>,
    /// The current still top prefix, in sampled rows (`run ≥ 1`).
    /// Bottom-aligned into the band area on every write — the collapsing
    /// toolbar handling above.
    prefix: usize,
}

impl StickyBands {
    fn new() -> Self {
        Self {
            top: 0,
            inserted: 0,
            bot: 0,
            run: Vec::new(),
            bot_run: Vec::new(),
            pending: Vec::new(),
            prefix: 0,
        }
    }

    /// Fold one frame's stillness evidence in. `still` is per-sampled-row
    /// stillness against the comparison base at offset 0; `position` is
    /// the frame's content position (post-transition). Sized on first
    /// use — the sheet is constructed before any evidence exists.
    fn update(&mut self, still: &[bool], position: i64) {
        if self.run.len() != still.len() {
            self.run = vec![0; still.len()];
            self.bot_run = vec![0; still.len()];
        }
        let monotone = self.pending.last().is_none_or(|&p| position > p);
        for (j, &s) in still.iter().enumerate() {
            self.run[j] = if s { self.run[j].saturating_add(1) } else { 0 };
        }
        // bot_run counts from the BOTTOM edge: index k is the k-th
        // sampled row up from the bottom.
        for k in 0..still.len() {
            let still_from_bottom = still[still.len() - 1 - k];
            self.bot_run[k] = if still_from_bottom {
                self.bot_run[k].saturating_add(1)
            } else {
                0
            };
        }
        // The top prefix: rows [0, prefix) all still THIS frame.
        self.prefix = still.iter().take_while(|&s| *s).count();
        if self.prefix >= MIN_BAND_GRIDS && monotone {
            self.pending.push(position);
        } else {
            // Prefix broke or the walk reversed — everything recorded
            // since the band went still is no longer trustworthy as
            // duplicate-segment geometry.
            self.pending.clear();
        }
    }

    /// Whether the top band should commit now, and at what height (in
    /// canvas rows). Requires the full prefix to have been still for
    /// [`STILL_FRAMES`] frames, structure inside the band, and a height
    /// between the current band and the fraction cap.
    fn top_commit(&self, cur: &RowProfile, viewport_height: u32) -> Option<u32> {
        let step = cur.step.max(1);
        let skip = (self.top as usize).div_ceil(step);
        let rows = self.commit_height(&self.run, skip, step, viewport_height)?;
        // At least one structured row must attest the band (the
        // flat-content guard — see the struct docs).
        let structured = (skip..)
            .take_while(|&j| (j * step) as u32 <= rows)
            .any(|j| grid_row_structured(cur, j * step));
        (structured && rows > self.top).then_some(rows)
    }

    /// The bottom-band analogue: same evidence bar, mask-only effect.
    /// `bot_run` counts from the BOTTOM edge, so its anchored skip and
    /// structure rows are mirrored.
    fn bot_commit(&self, cur: &RowProfile, viewport_height: u32) -> Option<u32> {
        let step = cur.step.max(1);
        let skip = (self.bot as usize).div_ceil(step);
        let rows = self.commit_height(&self.bot_run, skip, step, viewport_height)?;
        let last = cur.height as usize - 1;
        let structured = (skip..)
            .take_while(|&j| (j * step) as u32 <= rows)
            .any(|j| grid_row_structured(cur, last - j * step));
        (structured && rows > self.bot).then_some(rows)
    }

    /// Shared commit-height test for one edge's run table: how many
    /// rows from the anchored edge have been still for
    /// [`STILL_FRAMES`] frames, capped by the fraction limit. `skip`
    /// is how many anchored grids the already-committed band covers;
    /// grid `j` attests to rows `[j·step, (j+1)·step)` from the edge,
    /// so the committed height is the last attested grid's far edge —
    /// at most `step − 1` rows coarser than the true band, which only
    /// ever pins a sliver of content beyond the chrome.
    fn commit_height(
        &self,
        run: &[u16],
        skip: usize,
        step: usize,
        viewport_height: u32,
    ) -> Option<u32> {
        let mut grids = 0usize;
        for &r in run.iter().skip(skip) {
            if r < STILL_FRAMES {
                break;
            }
            grids += 1;
        }
        if grids < MIN_BAND_GRIDS {
            return None;
        }
        let rows = ((skip + grids) * step) as u32;
        let cap = (viewport_height as f32 * MAX_BAND_FRACTION) as u32;
        (rows <= cap).then_some(rows)
    }
}

/// Whether sampled row `j` of `profile` carries alignment-grade texture
/// (luma spread across the interior columns). Band-level flat-content
/// guard: a "still" band with no structured row at all is a flat
/// content region, not chrome.
fn grid_row_structured(profile: &RowProfile, j: usize) -> bool {
    profile
        .data
        .get(j * profile.stride..(j + 1) * profile.stride)
        .is_some_and(|l| {
            let (mut lo, mut hi) = (u8::MAX, u8::MIN);
            for &v in l {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            hi.saturating_sub(lo) > STRUCTURED_SPREAD
        })
}

/// Per-sampled-row stillness of `cur` against `base` at offset 0 (same
/// viewport position). Only meaningful on frames accepted with dy ≠ 0 —
/// see [`StickyBands`]. The grid matches `cur.step` so the band
/// geometry and the matcher's scan share one coordinate system.
fn stillness_grid(base: &RowProfile, cur: &RowProfile, options: &SheetLimits) -> Vec<bool> {
    if base.stride != cur.stride || base.height != cur.height {
        return Vec::new();
    }
    let h = cur.height as usize;
    let mut still = Vec::with_capacity(h.div_ceil(cur.step.max(1)));
    let mut j = 0usize;
    while j < h {
        let c = &cur.data[j * cur.stride..(j + 1) * cur.stride];
        let b = &base.data[j * cur.stride..(j + 1) * cur.stride];
        let mad = c
            .iter()
            .zip(b.iter())
            .map(|(a, b)| u32::from(a.abs_diff(*b)))
            .sum::<u32>() as f32
            / cur.stride as f32;
        still.push(mad <= options.cost_threshold);
        j += cur.step;
    }
    still
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

/// One matcher verdict against a specific comparison base (see
/// `ScrollSheet::place` for the dual-anchor selection).
enum Placement {
    /// A usable offset (cost and margin pass, dy ≠ 0).
    Match(OffsetEstimate),
    /// Cost passes at dy 0 — content changed without moving.
    Still,
    /// Unusable, with the taxonomy reason.
    Reject(Refusal),
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
/// rows (see [`RowProfile`] for why means are not enough). `sticky_trim`
/// extends the static edge trim by the committed sticky bands (see
/// [`StickyBands`]) so detected chrome leaves the scan too. Returns
/// [`None`] when no candidate is admissible at all.
/// Current-frame row range compared for viewport movement `d`:
/// `[first, end)`. Positive `d` means the page scrolled down, so the
/// current frame's row `j` matches the anchor's row `j + d`.
fn overlap_window(h: i32, d: i32, trim_top: i32, trim_bot: i32) -> (usize, usize) {
    let end = (h - trim_bot - d.max(0)).max(0) as usize;
    let first = ((-d).max(0).max(trim_top)) as usize;
    (first, end)
}

/// Weighted band-mean cost: the cheap signal that ranks candidate
/// offsets before any full-row work. Masked (fixed-sidebar) bands are
/// skipped and the worst active band drops out (a trimmed mean), so one
/// animated band cannot dominate the ranking.
fn band_cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
    min_overlap: i32,
) -> Option<f32> {
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim_top, trim_bot);
    let rows = end.saturating_sub(first);
    if rows < min_overlap as usize {
        return None;
    }
    let mut sums = [0.0f32; BAND_COUNT];
    for j in first..end {
        let pj = (j as i32 + d) as usize;
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            sums[band] += (cur.band_means[j][band] - prev.band_means[pj][band]).abs();
        }
    }
    let mut active: Vec<f32> = sums
        .iter()
        .zip(weights.iter())
        .filter(|&(_, &w)| w > 0.0)
        .map(|(sum, _)| *sum / rows as f32)
        .collect();
    if active.is_empty() {
        return None;
    }
    if active.len() >= 4 {
        let worst = active.iter().copied().fold(0.0f32, f32::max);
        if let Some(pos) = active.iter().position(|&v| v == worst) {
            active.swap_remove(pos);
        }
    }
    Some(active.iter().sum::<f32>() / active.len() as f32)
}

/// Weighted full-row pixel MAD on the step grid. This is the exact
/// signal that decides a match: masked bands are skipped but the
/// per-row trimmed mean from v1 is kept (fixed edge chrome misaligns
/// under every translation, and dropping the worst rows absorbs it).
fn row_cost(
    prev: &RowProfile,
    cur: &RowProfile,
    d: i32,
    weights: &BandWeights,
    trim_top: i32,
    trim_bot: i32,
) -> f32 {
    let h = prev.height.min(cur.height) as i32;
    let (first, end) = overlap_window(h, d, trim_top, trim_bot);
    let first = first.div_ceil(cur.step) * cur.step;
    let stride = cur.stride;
    let mut row_sums: Vec<f32> = Vec::with_capacity(16);
    let mut j = first;
    while j < end {
        let mut sum = 0u32;
        let mut width = 0usize;
        for (band, &weight) in weights.iter().enumerate() {
            if weight <= 0.0 {
                continue;
            }
            let (start, stop) = cur.bands[band];
            let c = &cur.data[j * stride + start..j * stride + stop];
            let p = &prev.data
                [(j as i32 + d) as usize * stride + start..(j as i32 + d) as usize * stride + stop];
            sum += c
                .iter()
                .zip(p.iter())
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum::<u32>();
            width += stop - start;
        }
        if width == 0 {
            // Defensive: a mask that zeroed every band is meaningless;
            // fall back to the full interior row.
            let c = &cur.data[j * stride..(j + 1) * stride];
            let p =
                &prev.data[(j as i32 + d) as usize * stride..(j as i32 + d + 1) as usize * stride];
            sum = c
                .iter()
                .zip(p.iter())
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum();
            width = stride;
        }
        row_sums.push(sum as f32 / width as f32);
        j += cur.step;
    }
    let n = row_sums.len();
    if n == 0 {
        return f32::INFINITY;
    }
    let keep = n - n / 7;
    row_sums.select_nth_unstable_by(keep - 1, f32::total_cmp);
    row_sums.iter().take(keep).sum::<f32>() / keep as f32
}

/// Candidate offsets for the first search pass: the prediction
/// neighbourhood (the ±128-row window around the last accepted step,
/// where a smooth scroll almost always lands) plus a step-3 coarse
/// sweep of the whole range.
/// Small ranges are enumerated fully so tiny test/toolbar-sized frames
/// keep exact behaviour.
fn candidate_offsets(max_admissible: i32, prediction: i32) -> Vec<i32> {
    let mut seen = vec![false; (max_admissible as usize) * 2 + 1];
    let mut candidates = Vec::new();
    let mut push = |d: i32, out: &mut Vec<i32>| {
        if d.abs() <= max_admissible {
            let index = (d + max_admissible) as usize;
            if !seen[index] {
                seen[index] = true;
                out.push(d);
            }
        }
    };
    let prediction = prediction.clamp(-max_admissible, max_admissible);
    for delta in 0..=128 {
        push(prediction - delta, &mut candidates);
        push(prediction + delta, &mut candidates);
    }
    if max_admissible <= 96 {
        for d in -max_admissible..=max_admissible {
            push(d, &mut candidates);
        }
    } else {
        let mut d = -max_admissible;
        while d <= max_admissible {
            push(d, &mut candidates);
            d += 3;
        }
        push(max_admissible, &mut candidates);
    }
    candidates
}

/// Find the offset with a two-stage search: rank every candidate with
/// the cheap 8-band signature, refine the three best basins at ±3 rows,
/// then verify the twelve best (pairwise >2 rows apart) offsets with the
/// full-row cost. Output semantics match the old brute-force scan — only
/// the work per frame drops by an order of magnitude, so the capture
/// loop stops being matcher-bound.
///
/// `weights` masks fixed sidebars (see [`MotionMask`]). `prediction` is
/// the last accepted step; it only seeds the search order.
fn estimate_offset(
    prev: &RowProfile,
    cur: &RowProfile,
    options: &SheetLimits,
    sticky_trim: (u32, u32),
    weights: &BandWeights,
    prediction: i32,
) -> Option<OffsetEstimate> {
    let h = prev.height.min(cur.height) as i32;
    if h <= 0 || prev.stride != cur.stride {
        return None;
    }
    // Sticky-edge trim: fixed chrome inside the selection (site navbars,
    // toolbar strips) breaks the row-translation assumption — a ~4%
    // fixed band alone pushes the mean cost past the threshold and
    // EVERY frame rejects (the "scrolled up past the site header and
    // the preview stopped syncing" report: log showed one Prepend then
    // an unbroken run of Mismatch). Edges leave the SCAN only — dy
    // semantics are unchanged (translation is uniform below the band)
    // and the canvas still writes whole frames. Small frames (tests,
    // tiny selections) keep every row: there is no budget to spare.
    // Detected sticky bands (adaptive, any height up to the fraction
    // cap) extend the static budget — the static trim survives only as
    // the pre-detection fallback.
    let (trim_top, trim_bot) = if h >= 200 {
        (
            (h / 16).min(96).max(sticky_trim.0 as i32),
            (h / 48).min(32).max(sticky_trim.1 as i32),
        )
    } else {
        (sticky_trim.0 as i32, sticky_trim.1 as i32)
    };
    let max_motion = ((h as f32) * options.max_motion_ratio) as i32;
    let max_admissible = max_motion.min(h - trim_top - trim_bot - options.min_overlap as i32);
    if max_admissible < 0 {
        return None;
    }
    let min_overlap = options.min_overlap as i32;

    let mut candidates = candidate_offsets(max_admissible, prediction);
    let sig = |d: i32, prev: &RowProfile, cur: &RowProfile| {
        band_cost(prev, cur, d, weights, trim_top, trim_bot, min_overlap)
    };

    // Stage 1: rank with the band signature and pick the three best
    // basins (separated by 6 rows so one valley is not counted thrice).
    let mut ranked: Vec<(i32, f32)> = candidates
        .iter()
        .filter_map(|&d| sig(d, prev, cur).map(|cost| (d, cost)))
        .collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));
    let mut basins: Vec<i32> = Vec::with_capacity(3);
    for &(d, _) in &ranked {
        if basins.len() == 3 {
            break;
        }
        if basins.iter().all(|other| (other - d).abs() > 6) {
            basins.push(d);
        }
    }
    // Stage 2: refine each basin at ±3 rows (the coarse sweep is step 3).
    for &basin in &basins {
        for delta in -3..=3 {
            let d = basin + delta;
            if d.abs() <= max_admissible && !candidates.contains(&d) {
                candidates.push(d);
            }
        }
    }
    let mut ranked: Vec<(i32, f32)> = candidates
        .iter()
        .filter_map(|&d| sig(d, prev, cur).map(|cost| (d, cost)))
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())));

    // Stage 3: verify up to 12 offsets at least 3 rows apart with the
    // full-row cost. d = 0 stays in the pool so flat-content ties still
    // resolve to Still rather than a phantom offset.
    let mut shortlist: Vec<i32> = Vec::with_capacity(12);
    let mut include_zero = false;
    for &(d, _) in &ranked {
        if shortlist.len() == 12 {
            break;
        }
        include_zero |= d == 0;
        if shortlist.iter().all(|other| (other - d).abs() > 2) {
            shortlist.push(d);
        }
    }
    if !include_zero {
        shortlist.push(0);
    }
    let mut best: Option<(i32, f32)> = None;
    let mut evaluated: Vec<(i32, f32)> = Vec::with_capacity(shortlist.len());
    for d in shortlist {
        let cost = row_cost(prev, cur, d, weights, trim_top, trim_bot);
        evaluated.push((d, cost));
        if best.is_none_or(|(_, bc)| cost < bc) {
            best = Some((d, cost));
        }
    }
    let (best_dy, best_cost) = best?;
    let runner_up = evaluated
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
/// rectangle; read the folded image from [`ScrollSheet::canvas`].
pub(crate) struct ScrollSheet {
    /// Sheet-space width: the captured region's width for vertical
    /// sessions, its height for horizontal ones (frames are transposed).
    width: u32,
    /// Sheet-space viewport height: the captured region's height for
    /// vertical sessions, its width for horizontal ones.
    viewport_height: u32,
    /// The original captured region's size. `output_dimensions()` swaps
    /// these back at delivery.
    frame_size: (u32, u32),
    axis: Axis,
    /// Once a non-seed frame is accepted the canvas is no longer a
    /// single frame, so switching axes can no longer be lossless.
    axis_locked: bool,
    options: SheetLimits,
    /// The sheet so far: RGBA, `height()` rows of `width` px.
    canvas: Vec<u8>,
    state: ViewportState,
    /// The last frame used as comparison anchor (raw bytes — the exact
    /// duplicate test memcmp's it).
    anchor_raw: Vec<u8>,
    anchor_profile: RowProfile,
    /// Sticky-band detection state (see [`StickyBands`]).
    sticky: StickyBands,
    /// The profile of the last frame ACCEPTED into the canvas. The
    /// FIRST comparison base in [`ScrollSheet::place`] — see the
    /// module docs ("dual anchor"): a clean frame after a tear matches
    /// it directly and the full displacement lands, where the riding
    /// anchor would swallow the in-between band.
    stable_profile: RowProfile,
    /// Fixed-sidebar mask learned from consecutive frames (see
    /// [`MotionMask`]): bands that never move are chrome, not content.
    motion: MotionMask,
    /// The last accepted step, positive = down. Seeds the candidate
    /// search order; it never gates correctness (the coarse sweep still
    /// covers the full range when the user reverses).
    last_dy: i32,
    /// Diagnostics for the scroll loop's logs / progress UI.
    frames_pushed: usize,
    frames_accepted: usize,
    /// How many times rows were spliced in ABOVE the sheet content — a
    /// prepend, a fixed-band commit or an axis switch. The live preview
    /// caches the pixels it holds; this is its staleness signal when the
    /// window itself did not move.
    top_shifts: u32,
}

impl ScrollSheet {
    /// Start a sheet from the viewport's current content. Fails with
    /// [`Refusal::TooSmall`] when the selection is too small to
    /// ever match (the height must clear the overlap budget with room
    /// for motion).
    pub(crate) fn new(
        width: u32,
        viewport_height: u32,
        first_frame: &[u8],
        options: SheetLimits,
    ) -> Result<Self, Refusal> {
        Self::new_with_axis(width, viewport_height, first_frame, options, Axis::Vertical)
    }

    /// Start a sheet whose long image grows along `axis`. Horizontal
    /// sessions transpose the seed and every later frame, so the matcher
    /// and canvas stay row-oriented.
    pub(crate) fn new_with_axis(
        width: u32,
        viewport_height: u32,
        first_frame: &[u8],
        options: SheetLimits,
        axis: Axis,
    ) -> Result<Self, Refusal> {
        let (sheet_w, sheet_h) = match axis {
            Axis::Vertical => (width, viewport_height),
            Axis::Horizontal => (viewport_height, width),
        };
        if sheet_w < 16 || sheet_h <= options.min_overlap + 32 {
            return Err(Refusal::TooSmall);
        }
        debug_assert_eq!(
            first_frame.len(),
            width as usize * viewport_height as usize * 4,
            "frame size must match the declared viewport"
        );
        let seed = match axis {
            Axis::Vertical => first_frame.to_vec(),
            Axis::Horizontal => transpose_rgba(first_frame, width, viewport_height),
        };
        let profile = RowProfile::from_rgba(&seed, sheet_w, sheet_h);
        Ok(Self {
            width: sheet_w,
            viewport_height: sheet_h,
            frame_size: (width, viewport_height),
            axis,
            axis_locked: false,
            options,
            canvas: seed.clone(),
            state: ViewportState {
                position: 0,
                max_position: 0,
            },
            anchor_raw: seed,
            anchor_profile: profile.clone(),
            sticky: StickyBands::new(),
            stable_profile: profile,
            motion: MotionMask::default(),
            last_dy: 0,
            frames_pushed: 1,
            frames_accepted: 1,
            top_shifts: 0,
        })
    }

    /// The long image's growth direction.
    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    /// Axis switching is lossless only while the canvas is still the
    /// seed frame; after the first accepted frame it is locked.
    pub(crate) fn axis_locked(&self) -> bool {
        self.axis_locked
    }

    /// Switch growth direction before the first accepted frame. Returns
    /// false (leaving everything untouched) once locked.
    pub(crate) fn set_axis(&mut self, axis: Axis) -> bool {
        if self.axis == axis {
            return true;
        }
        if self.axis_locked
            || self.frames_accepted > 1
            || self.sticky.inserted > 0
            || self.state.position != 0
        {
            self.axis_locked = true;
            return false;
        }
        let (old_w, old_h) = (self.width, self.height());
        // The transposed viewport must still clear the matcher's size
        // gate (the original width becomes the sheet height).
        let (new_w, new_h) = (old_h, old_w);
        if new_w < 16 || new_h <= self.options.min_overlap + 32 {
            return false;
        }
        let seed = transpose_rgba(&self.canvas, old_w, old_h);
        self.canvas = seed.clone();
        self.width = old_h;
        self.viewport_height = old_w;
        self.anchor_raw = seed.clone();
        self.anchor_profile = RowProfile::from_rgba(&seed, self.width, self.viewport_height);
        self.stable_profile = self.anchor_profile.clone();
        self.sticky = StickyBands::new();
        self.motion = MotionMask::default();
        self.last_dy = 0;
        self.state = ViewportState::default();
        self.axis = axis;
        self.top_shifts = self.top_shifts.saturating_add(1);
        true
    }

    fn height(&self) -> u32 {
        // Physical canvas rows: the seed frame's viewport rows, every
        // spliced band row, and the grown content below.
        (i64::from(self.viewport_height)
            + i64::from(self.sticky.inserted)
            + self.state.max_position) as u32
    }

    /// The sheet so far (RGBA, `width` × `height()`).
    pub(crate) fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    /// Sheet-space dimensions — what the live preview consumes (for a
    /// horizontal session this is the transposed, tall strip).
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height())
    }

    /// The finished image's dimensions (horizontal swaps them back).
    pub(crate) fn output_dimensions(&self) -> (u32, u32) {
        match self.axis {
            Axis::Vertical => (self.width, self.height()),
            Axis::Horizontal => (self.height(), self.width),
        }
    }

    /// The finished image: RGBA in output orientation. Vertical sessions
    /// copy the canvas as-is; horizontal ones transpose it once, at
    /// delivery (never per preview frame).
    pub(crate) fn output(&mut self) -> (u32, u32, Vec<u8>) {
        let (w, h) = self.output_dimensions();
        let rgba = match self.axis {
            Axis::Vertical => self.canvas.clone(),
            Axis::Horizontal => transpose_rgba(&self.canvas, self.width, self.height()),
        };
        (w, h, rgba)
    }

    /// How many rows the viewport has advanced beyond the initial view —
    /// the number a progress UI reports as "captured".
    pub(crate) fn captured_extent(&self) -> u32 {
        self.state.max_position as u32
    }

    /// Where the capture viewport currently sits on the canvas:
    /// (top row, row count). Feeds the preview's viewport highlight —
    /// moving the region frame or scrolling both travel through here.
    /// With a committed top band the reported span is the CONTENT
    /// segment (the band area at the canvas top is every frame's, not a
    /// position — the highlight belongs below it).
    pub(crate) fn viewport_span(&self) -> (u32, u32) {
        (
            self.sticky.top + self.state.position.max(0) as u32,
            self.viewport_height - self.sticky.top,
        )
    }

    /// Content-splice counter (see the field docs): the live preview
    /// compares it to decide whether its cached window still matches.
    pub(crate) fn top_revision(&self) -> u32 {
        self.top_shifts
    }

    /// (frames pushed, frames folded into the canvas) — health signals
    /// for the scroll loop's logs.
    pub(crate) fn stats(&self) -> (usize, usize) {
        (self.frames_pushed, self.frames_accepted)
    }

    /// Judge one capture of the selection rectangle (same size as the
    /// first frame) and fold it into the canvas.
    pub(crate) fn push(&mut self, frame: &[u8]) -> Fold {
        self.frames_pushed += 1;
        debug_assert_eq!(
            frame.len(),
            self.frame_size.0 as usize * self.frame_size.1 as usize * 4,
            "frame size must match the declared viewport"
        );
        // Horizontal sessions are vertical sessions over transposed
        // frames; the transpose is the only axis-specific code in the
        // whole sheet.
        let transposed;
        let frame = match self.axis {
            Axis::Vertical => frame,
            Axis::Horizontal => {
                transposed = transpose_rgba(frame, self.frame_size.0, self.frame_size.1);
                transposed.as_slice()
            }
        };
        if frame == self.anchor_raw.as_slice() {
            // Poll outran the repaint: not even a Still — the anchor
            // must stay so the next real frame diffs against fresh bytes.
            return Fold::Unchanged;
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

    /// One matcher verdict against a specific comparison base.
    fn classify(&self, base: &RowProfile, cur: &RowProfile) -> Placement {
        // Committed sticky bands extend the edge trim of the scan.
        let trim = (self.sticky.top, self.sticky.bot);
        let Some(estimate) = estimate_offset(
            base,
            cur,
            &self.options,
            trim,
            &self.motion.weights,
            self.last_dy,
        ) else {
            return Placement::Reject(Refusal::NoOverlap);
        };
        if estimate.cost > self.options.cost_threshold {
            return Placement::Reject(Refusal::Mismatch);
        }
        if estimate.dy == 0 {
            return Placement::Still;
        }
        if estimate.runner_up - estimate.cost < self.options.ambiguity_margin {
            return Placement::Reject(Refusal::Unclear);
        }
        Placement::Match(estimate)
    }

    fn place(&mut self, frame: &[u8], profile: &RowProfile) -> Fold {
        // Low-information guard: a near-uniform anchor cannot be aligned
        // by any translation — the
        // conservative answer is Still; a later textured frame brings
        // something to match against. Flat PDF margins hit this.
        if self.anchor_profile.spread <= 1 {
            return Fold::Still;
        }
        // Sidebar evidence: compare this frame against the riding anchor
        // at the same position and down-weight bands that did not move.
        self.motion.observe(&self.anchor_profile, profile);
        // Dual-anchor selection. The riding anchor (every non-duplicate
        // frame, INCLUDING rejects) sits at a position the canvas never
        // advanced to — accepting a dy measured from it silently drops
        // the band between the last accept and the anchor (the "lost
        // band" that left the long image shorter than the real one,
        // user-visible as the preview not matching the frame). The last
        // ACCEPTED frame is therefore tried FIRST: a clean frame after
        // a tear matches it directly and the full displacement lands.
        // The riding anchor remains the fallback for motion beyond the
        // stable window — a jump no single bridge could cover — where
        // the band loss is the documented, unavoidable cost.
        let (estimate, stable_used) = match self.classify(&self.stable_profile, profile) {
            Placement::Match(e) => (e, true),
            Placement::Still => return Fold::Still,
            Placement::Reject(_) => match self.classify(&self.anchor_profile, profile) {
                Placement::Match(e) => (e, false),
                Placement::Still => return Fold::Still,
                Placement::Reject(reason) => return Fold::Refused(reason),
            },
        };

        let (next, branch, growth) = self.state.transition(estimate.dy);
        if u64::from(self.viewport_height)
            + u64::from(self.sticky.inserted)
            + next.max_position as u64
            > self.options.max_height as u64
        {
            return Fold::Refused(Refusal::TooTall);
        }

        // Sticky evidence rides on the same accepted match — stillness
        // is only meaningful against the base that produced it.
        {
            let base = if stable_used {
                &self.stable_profile
            } else {
                &self.anchor_profile
            };
            let still = stillness_grid(base, profile, &self.options);
            if !still.is_empty() {
                self.sticky.update(&still, next.position);
                if let Some(t) = self.sticky.top_commit(profile, self.viewport_height) {
                    self.commit_top_band(t, frame);
                }
                if let Some(b) = self.sticky.bot_commit(profile, self.viewport_height) {
                    self.sticky.bot = b;
                }
            }
        }

        let row = self.width as usize * 4;
        let top = self.sticky.top as usize;
        if branch == Branch::Above && growth > 0 {
            // Grow at the top of the CONTENT area: shift the content
            // rows down (memmove semantics — copy_within handles the
            // overlap), leaving zeroed rows above for the viewport
            // write. The band area at the physical top stays put.
            let shift = growth as usize * row;
            self.canvas.resize(self.canvas.len() + shift, 0);
            let len = self.canvas.len();
            self.canvas.copy_within(top..len - shift, top + shift);
            self.top_shifts = self.top_shifts.saturating_add(1);
        } else if branch == Branch::Below && growth > 0 {
            self.canvas
                .resize(self.canvas.len() + growth as usize * row, 0);
        }
        // Newest-wins: rewrite the whole viewport rectangle, not just the
        // new band (see the module docs). With a committed top band the
        // frame splits: rows [0, prefix) bottom-align into the band area
        // (rewritten every frame — a collapsing toolbar shrinks the live
        // prefix while the band keeps its tallest extent), rows from
        // `start = max(top, prefix)` land in the content area at
        // `position + start`. During the detection delay (prefix still
        // tentative, top not yet committed) the still prefix rows are
        // SKIPPED: writing them would stamp chrome pixels into the
        // content plane (placeholder-type sticky would happen to match;
        // browser-chrome type would corrupt whatever the earlier frames
        // recorded there). Skipping shifts nothing — the remaining rows
        // keep the `position + row` mapping either way.
        // Grid j attests rows [j·step, (j+1)·step); the prefix height is
        // the last attested row + 1 (one grid coarser than the truth,
        // never writing a row only its grid's FIRST row vouches for).
        // Zero prefix must stay zero — a phantom one-row skip would
        // punch a hole in every ordinary scroll (newly prepended rows
        // have no earlier frame to backfill them).
        let live_prefix = if self.sticky.prefix == 0 {
            0
        } else {
            ((self.sticky.prefix - 1) * profile.step.max(1) + 1).min(self.viewport_height as usize)
        };
        let prefix_rows = live_prefix.min(top);
        if prefix_rows > 0 {
            let band_bytes = prefix_rows * row;
            let band_dst = (top - prefix_rows) * row;
            self.canvas[band_dst..band_dst + band_bytes].copy_from_slice(&frame[..band_bytes]);
        }
        // Collapsing toolbars (live prefix below the committed height)
        // write from the committed height: the exposed rows are lost
        // rather than misplaced (the documented v1 simplification).
        let start = top.max(live_prefix);
        // A committed BOTTOM band is chrome, not content. Excluding it
        // from the normal viewport write is what keeps a fixed footer
        // from being stamped into the middle of the long image when
        // the user scrolls back up: the footer is only written while
        // GROWING at the bottom (where it belongs), and the old footer
        // at the previous bottom is overwritten by the new content
        // write on the way down. Without this, every up-scroll left a
        // footer copy at that viewport's bottom (the old
        // "footer repeats down the canvas" limitation).
        let bot = self.sticky.bot as usize;
        let content_end = self.viewport_height as usize - bot;
        let start = start.min(content_end);
        // `inserted` is the physical shift the commit surgeries imposed
        // on the content stream (mid-scroll pins splice the band in and
        // everything below moves down by `top`; a band pinned from the
        // first frame reuses the seed rows and shifts nothing). The
        // write destination must follow whichever happened.
        let dst = (next.position as usize + start + self.sticky.inserted as usize) * row;
        let src = start * row;
        let content_bytes = (content_end - start) * row;
        if content_bytes > 0 {
            self.canvas[dst..dst + content_bytes].copy_from_slice(&frame[src..content_end * row]);
        }
        if bot > 0 && branch == Branch::Below {
            // One footer, at the growing bottom. Appends are the only
            // branch that grows the canvas downward, so this is the
            // only place the footer's pixels are genuine content for
            // the final image; Shifted/Prepend leave the old footer
            // at the canvas bottom untouched.
            let footer_dst =
                (next.position as usize + content_end + self.sticky.inserted as usize) * row;
            let footer_src = content_end * row;
            self.canvas[footer_dst..footer_dst + bot * row]
                .copy_from_slice(&frame[footer_src..self.viewport_height as usize * row]);
        }
        self.state = next;
        self.frames_accepted += 1;
        // A non-seed frame has entered the canvas; the axis can no
        // longer be swapped losslessly.
        self.axis_locked = true;
        self.last_dy = estimate.dy;
        // The accepted frame is the new stable base (see `place`).
        self.stable_profile = profile.clone();
        match branch {
            Branch::Below => Fold::GrewBottom {
                dy: estimate.dy.unsigned_abs(),
                growth,
            },
            Branch::Above => Fold::GrewTop {
                dy: estimate.dy.unsigned_abs(),
                growth,
            },
            Branch::Within => Fold::Shifted { dy: estimate.dy },
        }
    }

    /// Commit surgery for a newly detected top band of `t` rows (see
    /// [`StickyBands`]). The insight that keeps this cheap: the "band
    /// copies" the detection delay wrote at each viewport top are NOT
    /// wrong — a sticky element's placeholder in the content plane
    /// renders the element itself, so those pixels are the correct
    /// content for exactly where they landed (newest-wins keeps the
    /// last copy, and later frames wash the placeholder once the
    /// element scrolls out of its container). The surgery therefore
    /// only has to make room: splice `t − top` fresh rows in at the
    /// band line, seed them from this frame's band rows, and leave
    /// `state` untouched — the content mapping (`canvas row =
    /// top + position + (row − top)`) is unchanged for rows below the
    /// band, so position/max_position stay valid as-is. The one
    /// exception is a band that was pinned from the very FIRST frame:
    /// the seed frame already wrote it at the canvas top, so the
    /// splice would duplicate it — detected by the canvas top already
    /// matching the frame's band rows (memcmp; also covers band
    /// growth when the old band rows at [0, top) still match).
    fn commit_top_band(&mut self, t: u32, frame: &[u8]) {
        let m0 = self.sticky.top;
        let g = t - m0; // band growth in rows
        let row = self.width as usize * 4;
        let t_bytes = t as usize * row;

        let already_pinned =
            self.canvas.len() >= t_bytes && self.canvas[..t_bytes] == frame[..t_bytes];
        if !already_pinned {
            // Splice g fresh rows in at the band line (below any
            // previously committed band), shifting the content down.
            let at = m0 as usize * row;
            let add = g as usize * row;
            let len = self.canvas.len();
            self.canvas.resize(len + add, 0);
            self.canvas.copy_within(at..len, at + add);
            // Seed the new band rows so the pin line (band bottom meets
            // content top) is continuous even before the write path
            // refreshes it.
            self.canvas[at..t_bytes].copy_from_slice(&frame[at..t_bytes]);
            self.sticky.inserted += g;
            self.top_shifts = self.top_shifts.saturating_add(1);
        }
        self.sticky.top = t;
        self.sticky.pending.clear();
    }
}

/// Logical width the preview panel paints a slice at. Part of the
/// preview CONTRACT rather than a UI detail: the engine sizes the slice
/// from it, so one sheet row is always the same number of screen pixels
/// and the strip cannot rescale under the user.
pub const PREVIEW_DISPLAY_W: f32 = 254.;

/// Slice sample width before the box filter: the preview keeps ~2× the
/// panel's logical column, which stays legible on HiDPI and keeps the
/// filter cheap. Shared by the engine (which cuts the slice) and the
/// panel (which sizes the hole it goes in).
pub const PREVIEW_SAMPLE_W: u32 = 480;

/// Downscale factor and slice width (preview px) for a sheet width.
/// One function so the engine's row math and the slicer can never
/// disagree about `k`.
fn preview_scale_for(width: u32, target_w: u32) -> (u32, u32) {
    let k = ((width as f32) / target_w.max(1) as f32).ceil().max(1.0) as u32;
    (k, width.div_ceil(k))
}

/// [`preview_scale_for`] at the production sample width.
pub fn preview_scale(width: u32) -> (u32, u32) {
    preview_scale_for(width, PREVIEW_SAMPLE_W)
}

/// Preview rows that exactly fill a panel well `well_h` logical px tall
/// at [`PREVIEW_DISPLAY_W`] wide — the strip is painted 1:1, never
/// scaled to the well.
pub fn preview_fit_rows(well_h: f32, pw: u32) -> u32 {
    ((well_h * pw.max(1) as f32 / PREVIEW_DISPLAY_W) as u32).max(16)
}

/// Where the preview panel's window sits on the sheet: `(top, rows)` in
/// sheet rows.
///
/// The panel is a viewfinder, not a second copy of the sheet: it shows a
/// window whose height is what fits the panel at a fixed scale, and the
/// camera keeps the viewport's BOTTOM edge on the window's bottom edge.
/// That is what makes the strip move at exactly the scroll rate — the
/// live frame enters at the bottom and captured rows leave at the top,
/// pixel-for-pixel like the page itself. While the viewport is at the
/// growing edge (the normal case) this is simply "the tail"; scrolling
/// back up re-aims the camera instead of sliding the whole sheet around.
pub fn preview_window(
    sheet_h: u32,
    viewport_top: u32,
    viewport_h: u32,
    window_rows: u32,
) -> (u32, u32) {
    // The window may never be shorter than the viewport: the highlight
    // must fit inside the strip, or the "what you are looking at" band
    // would be clipped by a panel-height accident.
    let rows = window_rows.max(viewport_h.min(sheet_h)).min(sheet_h).max(1);
    let bottom = viewport_top.saturating_add(viewport_h).min(sheet_h);
    let top = bottom.saturating_sub(rows).min(sheet_h - rows);
    (top, rows)
}

/// Box-filter downscale of a horizontal slice of the sheet — what the
/// preview panel paints. Integer factor `k = ceil(width / target_w)`
/// keeps the aspect; the slice is `[top, top + rows)`.
///
/// Returns `(width, height, rgba)` in PREVIEW pixels (the `k`
/// downscale). The caller already knows `top`/`rows`, so it can map the
/// live viewport into the same space without a second geometry source.
pub fn preview_slice(
    rgba: &[u8],
    width: u32,
    height: u32,
    top: u32,
    rows: u32,
    target_w: u32,
) -> (u32, u32, Vec<u8>) {
    let (k, pw) = preview_scale_for(width, target_w);
    let rows = rows.min(height.saturating_sub(top)).max(1);
    let ph = rows.div_ceil(k);
    let y0 = top.min(height.saturating_sub(1));
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

/// The capture loop's reading of the session — what the HUD's status chip
/// says. This is a STATE, not an error: a frame that cannot be placed is
/// ordinary (a torn animation, a hard flick, fixed chrome), and the
/// session keeps sampling through it. The old loop treated the same
/// evidence as fatal and ended the capture, which from the user's side
/// looked exactly like a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing has moved for a moment: the user is reading, or the page
    /// stopped. (The loop owns that clock; the ladder below never
    /// produces it.)
    Waiting,
    /// Frames are folding in. A healthy capture says nothing on screen.
    Following,
    /// Frames changed but none could be placed — no overlap with what we
    /// have. Transient by nature; the next settled frame usually folds.
    NoOverlap,
    /// No overlap for long enough that the region cannot advance on its
    /// own (fixed chrome taller than the matcher's trim, a video pane, a
    /// region that does not scroll). Still not fatal: the user can keep
    /// scrolling, move the frame, or finish with what is already there.
    Stalled,
}

/// Changed-but-unplaceable frames before the HUD stops calling it a
/// one-off tear: a smooth-scroll tear clears within a poll or two, and a
/// warning that flashes on every flick is noise.
const NO_OVERLAP_FRAMES: u32 = 2;
/// ... before the loop says the region cannot advance at all. At the
/// capture cadence this is ~2 s of scrolling.
const STALLED_FRAMES: u32 = 40;

/// The refusal ladder: how many frames in a row have changed without
/// being placeable. `refusals` counts CHANGED frames since the last
/// frame folded in — duplicates (nothing moved) are not refusals, so a
/// reader pausing mid-capture never looks broken.
pub(crate) fn phase_of(refusals: u32) -> Phase {
    if refusals >= STALLED_FRAMES {
        Phase::Stalled
    } else if refusals >= NO_OVERLAP_FRAMES {
        Phase::NoOverlap
    } else {
        Phase::Following
    }
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

    fn sheet() -> ScrollSheet {
        ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap()
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

    /// A striped canvas: row `y` is a solid colour so a downscaled block
    /// reveals exactly which sheet rows it came from.
    fn striped(w: u32, h: u32, color: impl Fn(u32) -> [u8; 4]) -> Vec<u8> {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&color(y));
            }
        }
        rgba
    }

    #[test]
    fn preview_slice_downscales_the_requested_window() {
        // 8×8 canvas: rows 0..4 red, rows 4..8 blue. target 2 → k = 4 →
        // 2 preview columns; the slice [4, 8) is all blue, [0, 8) mixes.
        let (w, h) = (8u32, 8u32);
        let rgba = striped(w, h, |y| {
            if y < 4 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        });
        let (pw, ph, out) = preview_slice(&rgba, w, h, 4, 4, 2);
        assert_eq!((pw, ph), (2, 1));
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| p == &[0, 0, 255, 255]),
            "slice [4, 8) must be the blue half, got {out:?}"
        );

        let (pw2, ph2, out2) = preview_slice(&rgba, w, h, 0, 8, 2);
        assert_eq!((pw2, ph2), (2, 2));
        let blocks = out2.as_chunks::<4>().0;
        assert!(blocks.first().is_some_and(|p| p[0] == 255 && p[2] == 0));
        assert!(blocks.last().is_some_and(|p| p[0] == 0 && p[2] == 255));

        // A window past the end is clamped, never a panic or a blank.
        let (_, ph3, out3) = preview_slice(&rgba, w, h, 7, 4, 2);
        assert_eq!(ph3, 1);
        assert_eq!(out3.len(), 2 * 4);
    }

    #[test]
    fn preview_window_tracks_the_viewport_bottom_and_clamps() {
        // A short sheet: the whole thing, top-anchored, even though the
        // requested window is taller.
        assert_eq!(preview_window(300, 0, 300, 800), (0, 300));
        assert_eq!(preview_window(300, 100, 200, 800), (0, 300));

        // Growing sheet, viewport at the bottom edge: the window is the
        // tail, so new rows enter at its bottom.
        assert_eq!(preview_window(10_000, 9_200, 800, 1_600), (8_400, 1_600));
        // Reading mid-sheet: the camera follows the viewport bottom.
        assert_eq!(preview_window(10_000, 3_000, 800, 1_600), (2_200, 1_600));
        // Near the top the camera clamps at row 0 instead of going
        // negative.
        assert_eq!(preview_window(10_000, 100, 800, 1_600), (0, 1_600));
        // Requested window shorter than the viewport: grown to fit, so
        // the highlight is never clipped.
        assert_eq!(preview_window(10_000, 5_000, 900, 400), (5_000, 900));
    }

    #[test]
    fn preview_window_rate_is_the_scroll_rate() {
        // The sync contract: for equal scroll steps the window top moves
        // by the same amount, so the strip advances exactly as fast as
        // the page (no easing, no easing-out at the edges) until the
        // sheet top clamps.
        let window = 1_600;
        let mut previous = None;
        for viewport_top in [2_000, 2_400, 2_800, 3_200] {
            let (top, _) = preview_window(20_000, viewport_top, 800, window);
            if let Some(previous) = previous {
                assert_eq!(top - previous, 400, "camera must move 1:1");
            }
            previous = Some(top);
        }
    }

    #[test]
    fn phase_ladder_separates_a_tear_from_a_dead_region() {
        assert_eq!(phase_of(0), Phase::Following);
        assert_eq!(phase_of(1), Phase::Following);
        assert_eq!(phase_of(2), Phase::NoOverlap);
        assert_eq!(phase_of(STALLED_FRAMES - 1), Phase::NoOverlap);
        assert_eq!(phase_of(STALLED_FRAMES), Phase::Stalled);
        assert_eq!(phase_of(u32::MAX), Phase::Stalled);
    }

    #[test]
    fn state_machine_takes_each_normative_branch() {
        let s = ViewportState::default();
        let (s, b, g) = s.transition(10);
        assert_eq!((b, g), (Branch::Below, 10));
        assert_eq!((s.position, s.max_position), (10, 10));

        // Back up inside the captured extent: contained, zero growth.
        let (s2, b2, g2) = s.transition(-4);
        assert_eq!((b2, g2), (Branch::Within, 0));
        assert_eq!((s2.position, s2.max_position), (6, 10));

        // Above the canvas top: prepend grows without losing height.
        let (s3, b3, g3) = s2.transition(-12);
        assert_eq!((b3, g3), (Branch::Above, 6));
        assert_eq!((s3.position, s3.max_position), (0, 16));
    }

    #[test]
    fn horizontal_sessions_transpose_in_and_out() {
        // One vertical reference walk…
        let mut vertical = ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap();
        vertical.push(&frame_at(30));
        let (vw, vh, vout) = vertical.output();

        // …and the same walk over transposed frames.
        let horiz = |top: u32| transpose_rgba(&frame_at(top), W, H);
        let mut horizontal =
            ScrollSheet::new_with_axis(H, W, &horiz(0), SheetLimits::default(), Axis::Horizontal)
                .unwrap();
        let out = horizontal.push(&horiz(30));
        assert!(matches!(out, Fold::GrewBottom { dy: 30, .. }), "{out:?}");
        let (hw, hh, hout) = horizontal.output();
        // The output is the vertical canvas laid on its side — exact
        // pixels, no resampling.
        assert_eq!((hw, hh), (vh, vw));
        assert_eq!(hout, transpose_rgba(&vout, vw, vh));
        // The preview keeps consuming the sheet-space (tall) canvas.
        assert_eq!(horizontal.dimensions(), (W, H + 30));
    }

    #[test]
    fn axis_switches_only_before_the_first_accepted_frame() {
        // A horizontally scrolling source: every pixel samples a 1-D
        // page indexed by column, so shifting the frame left moves each
        // row left too. The sheet sees the transposed (row) motion.
        let hframe = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H * 4) as usize);
            for y in 0..H {
                for x in 0..W {
                    let px = source_row(top.wrapping_add(x).wrapping_add(y.wrapping_mul(W)));
                    v.extend_from_slice(&px);
                }
            }
            v
        };
        let options = SheetLimits {
            min_overlap: 16,
            ..SheetLimits::default()
        };
        let mut s = ScrollSheet::new(W, H, &hframe(0), options).unwrap();
        assert_eq!(s.axis(), Axis::Vertical);
        assert!(!s.axis_locked());
        assert!(s.set_axis(Axis::Horizontal));
        assert_eq!(s.axis(), Axis::Horizontal);
        // Sheet space is the transposed seed: (H, W) → (H, W)? The
        // seed was 64×96; transposed it is 96×64.
        assert_eq!(s.dimensions(), (H, W));
        assert_eq!(s.output_dimensions(), (W, H));
        let out = s.push(&hframe(20));
        assert!(matches!(out, Fold::GrewBottom { dy: 20, .. }), "{out:?}");
        assert_eq!(s.output_dimensions(), (W + 20, H));
        assert!(s.axis_locked());
        // A grown canvas cannot be swapped losslessly.
        assert!(!s.set_axis(Axis::Vertical));
        assert_eq!(s.axis(), Axis::Horizontal);
    }

    #[test]
    fn first_frame_seeds_the_canvas() {
        let s = sheet();
        assert_eq!(s.dimensions(), (W, H));
        assert_eq!(s.canvas(), frame_at(0).as_slice());
        assert_eq!(s.captured_extent(), 0);
    }

    #[test]
    fn tiny_viewports_are_rejected_upfront() {
        let frame = vec![0u8; W as usize * 60 * 4];
        assert_eq!(
            ScrollSheet::new(W, 60, &frame, SheetLimits::default()).err(),
            Some(Refusal::TooSmall)
        );
    }

    #[test]
    fn scroll_down_appends_and_reconstructs_exactly() {
        let mut s = sheet();
        for top in [24u32, 24, 61, 95] {
            let out = s.push(&frame_at(top));
            // The dwell at 24 duplicates — skip asserting it here; the
            // duplicate path has its own test.
            if top != 24 {
                assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
            }
        }
        assert_eq!(s.dimensions(), (W, 95 + H));
        assert_eq!(s.canvas(), expected_canvas(0, 95).as_slice());
    }

    #[test]
    fn duplicate_poll_frames_are_skipped_without_disturbing_state() {
        let mut s = sheet();
        assert_eq!(s.push(&frame_at(0)), Fold::Unchanged);
        assert_eq!(s.push(&frame_at(0)), Fold::Unchanged);
        // The anchor is still the ORIGINAL frame: scrolling after the
        // duplicates must match against it, not a replaced copy.
        s.push(&frame_at(30));
        assert_eq!(s.dimensions(), (W, 30 + H));
    }

    #[test]
    fn scroll_up_prepends_recovering_content_above_the_start() {
        // Start mid-page, scroll down, then back up past the start.
        let mut s = ScrollSheet::new(W, H, &frame_at(100), SheetLimits::default()).unwrap();
        s.push(&frame_at(130));
        let out = s.push(&frame_at(90));
        assert!(matches!(out, Fold::GrewTop { growth: 10, .. }), "{out:?}");
        // Canvas covers source rows 90..=130+H.
        assert_eq!(s.canvas(), expected_canvas(90, 130).as_slice());
        assert_eq!(s.dimensions(), (W, 40 + H));
    }

    #[test]
    fn up_down_wiggle_is_contained_without_growth() {
        let mut s = sheet();
        s.push(&frame_at(40));
        let height_after_append = s.height();
        let out = s.push(&frame_at(15));
        assert!(matches!(out, Fold::Shifted { dy: -25 }), "{out:?}");
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
        let mut s = sheet();
        assert_eq!(s.push(&frame), Fold::Still);
        assert_eq!(s.dimensions(), (W, H));
        // The changed frame became the anchor: a scroll away from HERE
        // (not from the pre-blink frame) must still match.
        let out = s.push(&frame_at(20));
        assert!(
            matches!(out, Fold::GrewBottom { growth: 20, .. }),
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
        let mut s = ScrollSheet::new(W, H, &similar_page(0), SheetLimits::default()).unwrap();
        assert_eq!(s.push(&similar_page(48)), Fold::Refused(Refusal::Unclear));
        // The canvas is untouched by the rejection.
        assert_eq!(s.dimensions(), (W, H));
    }

    #[test]
    fn flat_content_reports_no_motion() {
        let flat = || vec![0xff; (W * H * 4) as usize];
        let mut changed = flat();
        changed[10 * 4 + 1] = 0x00;
        let mut s = ScrollSheet::new(W, H, &flat(), SheetLimits::default()).unwrap();
        // A flat frame "matches" every offset at ~zero cost — dy 0 wins
        // by scan order, and zero must not demand an ambiguity margin.
        assert_eq!(s.push(&changed), Fold::Still);
    }

    #[test]
    fn torn_frames_reject_but_the_settled_frame_recovers_the_full_step() {
        // A capture mid-smooth-scroll: the top half still shows the old
        // position, the bottom half the new one. Nothing aligns — and
        // the frame becomes the riding anchor.
        let old = frame_at(0);
        let settled = frame_at(20);
        let torn: Vec<u8> = [
            old[..old.len() / 2].to_vec(),
            settled[settled.len() / 2..].to_vec(),
        ]
        .concat();
        let mut s = sheet();
        assert_eq!(s.push(&torn), Fold::Refused(Refusal::Mismatch));
        // The settled frame matches the last ACCEPTED frame directly
        // (dual anchor), so the FULL 20-row step lands — no band lost.
        // (The single-anchor design swallowed the 20 rows between the
        // accept and the riding anchor — the long image came out
        // shorter than the real one.)
        let out = s.push(&settled);
        assert!(
            matches!(out, Fold::GrewBottom { dy: 20, growth: 20 }),
            "{out:?}"
        );
        let out = s.push(&frame_at(40));
        assert!(
            matches!(out, Fold::GrewBottom { dy: 20, growth: 20 }),
            "{out:?}"
        );
        assert_eq!(s.dimensions(), (W, 40 + H));
        assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
    }

    #[test]
    fn sticky_top_chrome_still_matches_through_the_content() {
        // A fixed site header occupies the top 48 rows of every frame;
        // the content scrolls beneath it. Under translation the band is
        // misaligned somewhere in the comparison window for EVERY
        // candidate — the untrimmed mean rejected every frame (the
        // "scrolled up to the navbar and sync died" report). The
        // trimmed row mean must absorb it in all three directions.
        // H2 clears the edge-trim gate; the header is ~6% of the rows,
        // well under the 1/7 trim.
        const H2: u32 = 800;
        let hdr = 48u32;
        let header = source_row(999_999).repeat(W as usize);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for _ in 0..hdr {
                v.extend_from_slice(&header);
            }
            for row in top..top + H2 - hdr {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v
        };

        // Down, then back up (Shifted) — the exact gesture from the
        // bug report.
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        let out = s.push(&frame(60));
        assert!(
            matches!(out, Fold::GrewBottom { dy: 60, growth: 60 }),
            "{out:?}"
        );
        let out = s.push(&frame(0));
        assert!(matches!(out, Fold::Shifted { dy: -60 }), "{out:?}");

        // Up past the start of the canvas (Prepend) through the band.
        let mut s = ScrollSheet::new(W, H2, &frame(100), SheetLimits::default()).unwrap();
        s.push(&frame(160));
        let out = s.push(&frame(40));
        assert!(
            matches!(
                out,
                Fold::GrewTop {
                    dy: 120,
                    growth: 60
                }
            ),
            "{out:?}"
        );
        // The canvas covers source rows 40..160+H2-hdr; the viewport
        // highlight must be back at the very top.
        assert_eq!(s.viewport_span().0, 0);
    }

    #[test]
    fn jumps_beyond_the_stable_window_fall_back_to_the_riding_anchor() {
        // A CLEAN frame can be rejected too — here by jumping beyond the
        // matcher's window from the last accept (0 → 70 > 0.6 × H). It
        // still becomes the riding anchor. The next frame (100) is also
        // beyond the stable window but 30 rows from that anchor: the
        // fallback accepts it with the anchor-relative dy — losing the
        // 70-row band nothing could bridge (the documented cost).
        let mut s = sheet();
        assert_eq!(s.push(&frame_at(70)), Fold::Refused(Refusal::Mismatch));
        let out = s.push(&frame_at(100));
        assert!(
            matches!(out, Fold::GrewBottom { dy: 30, growth: 30 }),
            "{out:?}"
        );
        assert_eq!(s.dimensions(), (W, H + 30));
    }

    #[test]
    fn preview_window_and_viewport_span_agree_on_the_last_accepted_frame() {
        // The user-facing contract behind the preview highlight: the
        // preview rows under the viewport span are EXACTLY the
        // downscaled rows of the last accepted frame — what the user's
        // frame shows. Teardown of a tear in between must not shift it.
        let old = frame_at(0);
        let settled = frame_at(20);
        let torn: Vec<u8> = [
            old[..old.len() / 2].to_vec(),
            settled[settled.len() / 2..].to_vec(),
        ]
        .concat();
        let mut s = sheet();
        s.push(&torn);
        let out = s.push(&settled);
        assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");

        let (w, h) = s.dimensions();
        let (vt, vh) = s.viewport_span();
        let (top, rows) = preview_window(h, vt, vh, 10_000);
        let (pw, ph, rgba) = preview_slice(s.canvas(), w, h, top, rows, 32);
        // The engine's mapping, verbatim (viewport_in_preview).
        let k = w.div_ceil(pw.max(1));
        let v_h = (vh / k).clamp(1, ph.max(1));
        let v_top = (vt.saturating_sub(top) / k).min(ph.saturating_sub(v_h));
        // The frame's own preview, same filter and factor: in frame
        // space the accepted frame IS rows [0, vh).
        let (fw, fh, frame_prev) = preview_slice(&settled, W, H, 0, vh, 32);
        assert_eq!((fw, fh), (pw, v_h));
        let start = (v_top * pw * 4) as usize;
        let end = ((v_top + v_h) * pw * 4) as usize;
        assert_eq!(
            &rgba[start..end],
            &frame_prev[..v_h as usize * pw as usize * 4],
            "the highlighted preview rows must show the frame's rows"
        );
    }

    #[test]
    fn coarse_sweep_finds_steps_beyond_the_prediction_window() {
        // A 300-row jump at 0 prediction is outside the ±128
        // neighbourhood, so only the step-3 coarse sweep can find it.
        const H2: u32 = 800;
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in top..top + H2 {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        let out = s.push(&frame(300));
        assert!(
            matches!(
                out,
                Fold::GrewBottom {
                    dy: 300,
                    growth: 300
                }
            ),
            "{out:?}"
        );
        let mut expected = Vec::new();
        for row in 0..300 + H2 {
            expected.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        assert_eq!(s.canvas(), expected.as_slice());
    }

    #[test]
    fn oversized_jump_is_high_residual() {
        let mut s = sheet();
        // 0.95 × H is beyond the admissible search range, so the best
        // in-range candidate is garbage — surfaced as Mismatch
        // (indistinguishable from a scene change by cost alone, which is
        // fine: the scroll loop reacts the same way).
        let out = s.push(&frame_at((H as f32 * 0.95) as u32));
        assert_eq!(out, Fold::Refused(Refusal::Mismatch));
    }

    #[test]
    fn height_cap_stops_the_sheet() {
        let options = SheetLimits {
            max_height: H + 60,
            ..SheetLimits::default()
        };
        let mut s = ScrollSheet::new(W, H, &frame_at(0), options).unwrap();
        s.push(&frame_at(30));
        s.push(&frame_at(60)); // reaches the cap exactly
        assert_eq!(s.dimensions(), (W, H + 60));
        assert_eq!(s.push(&frame_at(90)), Fold::Refused(Refusal::TooTall));
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
        let mut s = ScrollSheet::new(W, H, &frame_at(top), SheetLimits::default()).unwrap();
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
                !matches!(out, Fold::Refused(_)),
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

    // === Sticky-band detection ==========================================

    /// A structured (non-flat) fixed chrome row: dark left half, bright
    /// right half — constant across frames (what makes it sticky) but
    /// with in-row luma spread (what lets the band-level flat-content
    /// guard distinguish it from flat content).
    fn chrome_band(rows: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((W * rows * 4) as usize);
        for _ in 0..rows {
            for x in 0..W {
                let px: [u8; 4] = if x < W / 2 {
                    [30, 30, 40, 255]
                } else {
                    [220, 220, 230, 255]
                };
                v.extend_from_slice(&px);
            }
        }
        v
    }

    /// A page with a structured header pinned from the FIRST frame; the
    /// content scrolls beneath it (the classic fixed site navbar).
    #[test]
    fn structured_header_from_the_first_frame_pins_once() {
        const H2: u32 = 800;
        const HDR: u32 = 48;
        let band = chrome_band(HDR);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = band.clone();
            for row in top..top + H2 - HDR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v
        };
        let expected = |last: u32| -> Vec<u8> {
            let mut v = band.clone();
            for row in 0..last + H2 - HDR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v
        };

        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        // Run 1 on the second frame, run 2 → committed on the third.
        s.push(&frame(30));
        s.push(&frame(60));
        s.push(&frame(95));
        // Band written ONCE at the top (the seed frame already had it
        // there — no splice), content stream exact beneath it.
        assert_eq!(s.dimensions(), (W, HDR + (95 + H2 - HDR)));
        assert_eq!(s.canvas(), expected(95).as_slice());
        // The highlight reports the CONTENT segment, below the band.
        assert_eq!(s.viewport_span(), (HDR + 95, H2 - HDR));
    }

    /// The mid-scroll pin: the navbar travels WITH the content until it
    /// reaches the viewport top (its placeholder stays in the content
    /// plane — rendered once there), then freezes. Detection commits a
    /// few frames later; the surgery splices the band in and the
    /// content stream stays exact, navbar appearing once in the band
    /// and once at its pre-pin content position.
    #[test]
    fn mid_scroll_pin_splices_the_band_and_keeps_the_content_exact() {
        const H2: u32 = 800;
        const NAV: u32 = 12;
        const NAV_AT: u32 = 40; // the navbar's content-plane rows
        let band = chrome_band(NAV);
        // The source page carries the navbar content at its placeholder.
        let row_of = |row: u32| -> Vec<u8> {
            if (NAV_AT..NAV_AT + NAV).contains(&row) {
                let mut v = Vec::with_capacity(W as usize * 4);
                for x in 0..W {
                    let px: [u8; 4] = if x < W / 2 {
                        [30, 30, 40, 255]
                    } else {
                        [220, 220, 230, 255]
                    };
                    v.extend_from_slice(&px);
                }
                v
            } else {
                source_row(row).repeat(W as usize)
            }
        };
        // Pre-pin: the whole viewport is content. Post-pin: the band
        // sits at [0, NAV) and viewport row r >= NAV shows source row
        // c + r (the placeholder scrolls on under the frozen band).
        let frame = |c: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            if c >= NAV_AT {
                v.extend_from_slice(&band);
                for row in c + NAV..c + H2 {
                    v.extend_from_slice(&row_of(row));
                }
            } else {
                for row in c..c + H2 {
                    v.extend_from_slice(&row_of(row));
                }
            }
            v
        };
        let expected = |last: u32| -> Vec<u8> {
            let mut v = band.clone();
            for row in 0..last + H2 {
                v.extend_from_slice(&row_of(row));
            }
            v
        };

        // c = 40 is the first pinned frame; the still run reaches 2 two
        // frames later, so the commit happens while scrolling through
        // pinned territory.
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for c in [20u32, 40, 60, 90] {
            let out = s.push(&frame(c));
            assert!(matches!(out, Fold::GrewBottom { .. }), "frame {c}: {out:?}");
        }
        // Band spliced once at the top (+NAV rows). The content plane
        // ends at last + H (pinned frames contribute H − NAV viewport
        // rows, but the first frame's full H rows seed the plane): the
        // navbar's own placeholder rows [40, 52) appear exactly once.
        assert_eq!(s.dimensions(), (W, NAV + 90 + H2));
        assert_eq!(s.canvas(), expected(90).as_slice());
    }

    /// A tall fixed footer (taller than the static bottom trim) is
    /// detected and committed; matching continues across it.
    #[test]
    fn fixed_footer_commits_and_folding_continues() {
        const H2: u32 = 800;
        const FTR: u32 = 40; // > the static trim (min(800/48, 32))
        let band = chrome_band(FTR);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in top..top + H2 - FTR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v.extend_from_slice(&band);
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        let out = s.push(&frame(40));
        assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
        let out = s.push(&frame(80)); // second still frame → bot mask commits
        assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
        // No top band ever: the header scrolls.
        assert_eq!(s.viewport_span().0, 80);
        assert_eq!(s.dimensions(), (W, H2 + 80));
    }

    // === Fixed-sidebar motion mask =====================================

    /// A frame with a fixed left pane (PDF thumbnails, docs outline) that
    /// keeps its own content while the page scrolls beneath the right
    /// side. The pane's rows are static across frames but do NOT align
    /// under the page's row translation — exactly the case the old
    /// interior-only matcher rejected.
    fn sidebar_frame(top: u32) -> Vec<u8> {
        const W2: u32 = 320;
        const H2: u32 = 400;
        const PANE: u32 = 160;
        let mut v = Vec::with_capacity((W2 * H2 * 4) as usize);
        for row in 0..H2 {
            let pane = source_row(50_000 + row);
            let content = source_row(top + row);
            for x in 0..W2 {
                let px = if x < PANE { pane } else { content };
                v.extend_from_slice(&px);
            }
        }
        v
    }

    #[test]
    fn fixed_sidebar_bands_are_masked_and_folding_continues() {
        const W2: u32 = 320;
        const H2: u32 = 400;
        let mut s = ScrollSheet::new(W2, H2, &sidebar_frame(0), SheetLimits::default()).unwrap();
        let out = s.push(&sidebar_frame(40));
        assert!(
            matches!(out, Fold::GrewBottom { dy: 40, growth: 40 }),
            "the static pane must not poison the offset score: {out:?}"
        );
        // The pane occupies the four left signature bands of the
        // interior; the scrolling content the right four.
        assert_eq!(s.motion.weights[..4], [0.0, 0.0, 0.0, 0.0]);
        assert!(s.motion.weights[4..].iter().all(|&w| w == 1.0));
        assert_eq!(s.dimensions(), (W2, H2 + 40));
    }

    #[test]
    fn all_moving_bands_keep_the_full_frame_in_the_score() {
        // Without a stationary pane the mask must not drop anything:
        // page content moves across the whole width.
        let mut s = ScrollSheet::new(W, H, &frame_at(0), SheetLimits::default()).unwrap();
        s.push(&frame_at(20));
        assert_eq!(s.motion.weights, ALL_BANDS);
    }

    /// The committed footer is content-plane-excluded: the output is
    /// the page's content rows followed by exactly one footer band.
    #[test]
    fn fixed_footer_is_written_once_at_the_canvas_bottom() {
        const H2: u32 = 800;
        const FTR: u32 = 40;
        let band = chrome_band(FTR);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in top..top + H2 - FTR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v.extend_from_slice(&band);
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [40u32, 80, 120, 160] {
            s.push(&frame(top));
        }
        let (_, h) = s.dimensions();
        let canvas = s.canvas();
        let row = W as usize * 4;
        let mut ideal = Vec::new();
        for r in 0..160 + H2 - FTR {
            ideal.extend_from_slice(&source_row(r).repeat(W as usize));
        }
        ideal.extend_from_slice(&band);
        assert_eq!(h, 160 + H2);
        let mut first = None;
        for y in 0..h as usize {
            let got = &canvas[y * row..(y + 1) * row];
            let want = &ideal[y * row..(y + 1) * row];
            if got != want {
                first = Some(y);
                break;
            }
        }
        assert!(first.is_none(), "first footer mismatch at row {first:?}");
    }

    /// Scrolling back up after the footer was committed must not stamp
    /// a second footer at that viewport's bottom (the old limitation).
    #[test]
    fn fixed_footer_survives_an_up_scroll_without_duplicating() {
        const H2: u32 = 800;
        const FTR: u32 = 40;
        let band = chrome_band(FTR);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in top..top + H2 - FTR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v.extend_from_slice(&band);
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [40u32, 80, 120, 40] {
            s.push(&frame(top));
        }
        let (_, h) = s.dimensions();
        let canvas = s.canvas();
        let row = W as usize * 4;
        let mut ideal = Vec::new();
        for r in 0..120 + H2 - FTR {
            ideal.extend_from_slice(&source_row(r).repeat(W as usize));
        }
        ideal.extend_from_slice(&band);
        let mut first = None;
        for y in 0..h.min((ideal.len() / row) as u32) as usize {
            let got = &canvas[y * row..(y + 1) * row];
            let want = &ideal[y * row..(y + 1) * row];
            if got != want {
                first = Some(y);
                break;
            }
        }
        assert_eq!(h, 120 + H2);
        assert!(
            first.is_none(),
            "first up-scroll footer mismatch at row {first:?}"
        );
    }

    /// Header and footer committed together: the output is one header,
    /// the content stream, one footer — including after an up-scroll.
    #[test]
    fn header_and_footer_both_pin_once() {
        const H2: u32 = 800;
        const HDR: u32 = 40;
        const FTR: u32 = 40;
        let band = chrome_band(40);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            v.extend_from_slice(&band);
            for row in top..top + H2 - HDR - FTR {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v.extend_from_slice(&band);
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [40u32, 80, 120, 40] {
            s.push(&frame(top));
        }
        let mut ideal = Vec::new();
        ideal.extend_from_slice(&band);
        for row in 0..120 + H2 - HDR - FTR {
            ideal.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        ideal.extend_from_slice(&band);
        assert_eq!(s.dimensions(), (W, H2 + 120));
        assert_eq!(s.canvas(), ideal.as_slice());
    }

    /// A flat TOP region of moving content must NOT be pinned: the
    /// band-level structure guard is what separates "flat because
    /// chrome" from "flat because blank".
    #[test]
    fn flat_top_content_is_not_pinned() {
        const H2: u32 = 800;
        const FLAT: u32 = 100; // > any committed-band minimum
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in 0..H2 {
                let px = if row < FLAT {
                    [0xff, 0xff, 0xff, 0xff]
                } else {
                    source_row(top + row)
                };
                v.extend_from_slice(&px.repeat(W as usize));
            }
            v
        };
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        let out = s.push(&frame(40));
        assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
        // Still all-flat rows accumulated a run, but no structure → no
        // commit: the span stays whole-viewport.
        assert_eq!(s.viewport_span(), (40, H2));
    }
}
