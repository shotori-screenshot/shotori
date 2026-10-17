//! Sticky-band detection: fixed chrome inside the selection (site
//! navbars that pin on scroll, floating toolbars) violates the
//! row-translation assumption the matcher stands on. This module owns
//! the evidence (per-row stillness runs on accepted moving frames),
//! the commit surgery (pin the band once at the canvas top) and the
//! band-level flat-content guard.

use super::SheetLimits;
use super::profile::RowProfile;
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
pub(super) struct StickyBands {
    /// Committed top band height in canvas rows. Monotone; 0 until a
    /// band is committed.
    pub(super) top: u32,
    /// Rows the commit surgeries spliced into the canvas — the part of
    /// the physical height that is band, not content (a band pinned
    /// from the first frame reuses the seed frame's rows and never
    /// counts here). `height() = viewport_height + inserted +
    /// max_position`.
    pub(super) inserted: u32,
    /// Committed bottom band height in rows. Unlike the top band this
    /// needs no spliced rows: it is excluded from content writes and
    /// painted at the growing bottom on appends only (see the module
    /// notes).
    pub(super) bot: u32,
    /// Consecutive still frames per sampled row (top tracking), one
    /// entry per `step`-spaced row of the viewport.
    pub(super) run: Vec<u16>,
    /// Same, counted from the bottom edge upward.
    pub(super) bot_run: Vec<u16>,
    /// Content positions of the frames processed since the top prefix
    /// went still — the raw material for the commit surgery's duplicate
    /// segments. Reset whenever the prefix breaks (a row moved again)
    /// or the walk stops being monotonically downward (a Shifted or
    /// Prepend frame interleaves duplicated content in ways the segment
    /// math does not model; detection simply restarts).
    pub(super) pending: Vec<i64>,
    /// The current still top prefix, in sampled rows (`run ≥ 1`).
    /// Bottom-aligned into the band area on every write — the collapsing
    /// toolbar handling above.
    pub(super) prefix: usize,
}

impl StickyBands {
    pub(super) fn new() -> Self {
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
    pub(super) fn update(&mut self, still: &[bool], position: i64) {
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
    pub(super) fn top_commit(&self, cur: &RowProfile, viewport_height: u32) -> Option<u32> {
        let step = cur.step.max(1);
        let skip = (self.top as usize).div_ceil(step);
        let rows = self.commit_height(&self.run, skip, step, viewport_height)?;
        // At least one structured row must attest the band (the
        // flat-content guard — see the struct docs).
        // Strictly INSIDE the band: the row just past it is the first
        // scrolling row, so counting it would let a run of blank rows pin
        // itself off the texture of the content below (measured on the
        // sparse-page fixture: a sticky band of 8 blank rows committed,
        // shifting the viewport highlight for the rest of the session).
        let structured = (skip..)
            .take_while(|&j| ((j * step) as u32) < rows)
            .any(|j| grid_row_structured(cur, j * step));
        (structured && rows > self.top).then_some(rows)
    }

    /// The bottom-band analogue: same evidence bar, mask-only effect.
    /// `bot_run` counts from the BOTTOM edge, so its anchored skip and
    /// structure rows are mirrored.
    pub(super) fn bot_commit(&self, cur: &RowProfile, viewport_height: u32) -> Option<u32> {
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
    pub(super) fn commit_height(
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
pub(super) fn grid_row_structured(profile: &RowProfile, j: usize) -> bool {
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
pub(super) fn stillness_grid(
    base: &RowProfile,
    cur: &RowProfile,
    options: &SheetLimits,
) -> Vec<bool> {
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
