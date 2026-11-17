//! Detection of fixed edge bars from accepted scrolling frames. A row must
//! remain still while failing the document translation before it can establish
//! a band. Flat margins and short repaint stalls cannot establish one.
//!
//! Confirmed bands extend the matcher's exclusion area. Their live prefix is
//! excluded from overlap writes without inserting or shifting document rows.
//! Only a header already present at the original canvas top reserves a header
//! area during prepends. A late sticky title retains its original occurrence
//! in the document, with no additional copy at the image top.
//!
//! Bottom bands are excluded from content writes and painted at the growing
//! canvas bottom. Previously exposed content survives return passes.

use super::SheetLimits;
use super::profile::RowProfile;

/// Consecutive stationary observations needed for a candidate prefix.
const STILL_FRAMES: u16 = 2;
/// Persistence separates fixed bars from short-lived stale repaint regions.
const COMMIT_FRAMES: u16 = 6;
/// A committed band must end in a cluster of at least this many
/// displacement-verified grids — noise (a caret row, a one-row border)
/// must not pin.
const MIN_BAND_GRIDS: usize = 2;
/// Unverified-but-still grids tolerated between verified grids inside
/// one cluster: a band's flat stretches (padding rows, a uniform
/// navbar body) misfit nothing dy away when their translated partner
/// lands back inside the band. Past this many grids a stretch is
/// disconnected background, not interior.
const VERIFIED_GAP_GRIDS: usize = 4;
/// Bands taller than a quarter of the viewport are content, not chrome
/// (the matcher's overlap budget could not survive them anyway).
const MAX_BAND_FRACTION: f32 = 0.25;
/// Luma spread within a sampled row for it to count as "structured" —
/// the band-level flat-content guard (see above).
const STRUCTURED_SPREAD: u8 = 4;

/// Sticky-band state, in viewport (frame) coordinates.
#[derive(Debug, Clone)]
pub(super) struct StickyBands {
    /// Confirmed top exclusion height in viewport rows. Monotone; zero
    /// until a structured band has enough persistent motion evidence.
    pub(super) top: u32,
    /// Header rows already present at the original canvas top. Late overlays
    /// never reserve additional canvas rows or move the content coordinate system.
    pub(super) seed_top: u32,
    /// Confirmed bottom exclusion height. Painted only at the growing bottom
    /// so return passes cannot stamp a footer into previously exposed content.
    pub(super) bot: u32,
    /// Consecutive still frames per sampled row (top tracking), one
    /// entry per `step`-spaced row of the viewport.
    pub(super) run: Vec<u16>,
    /// Same, counted from the bottom edge upward.
    pub(super) bot_run: Vec<u16>,
    /// Same as `run`, counting only displacement-VERIFIED frames — see
    /// [`RowStillness::verified`]. The commit height stops at the last
    /// grid this counter attests: offset-0-still rows whose translated
    /// partner also matched (background strips, slow-tail content,
    /// self-similar band interiors) never grow it.
    pub(super) verified: Vec<u16>,
    /// Bottom-anchored mirror of `verified`.
    pub(super) bot_verified: Vec<u16>,
    /// The current still top prefix, in sampled rows (`run ≥ 1`).
    /// Seed chrome stays anchored to the canvas top; late overlays only
    /// exclude overlap writes.
    pub(super) prefix: usize,
    /// Bottom-anchored mirror of `prefix`: sampled rows from the bottom edge
    /// that remained still on the latest accepted frame (`bot_run ≥ 1`).
    pub(super) bot_prefix: usize,
}

impl StickyBands {
    pub(super) fn new() -> Self {
        Self {
            top: 0,
            seed_top: 0,
            bot: 0,
            run: Vec::new(),
            bot_run: Vec::new(),
            verified: Vec::new(),
            bot_verified: Vec::new(),
            prefix: 0,
            bot_prefix: 0,
        }
    }

    /// Fold one frame's stillness evidence in. `evidence` is per-sampled-row
    /// stillness against the comparison base (offset 0 and translated).
    /// Sized on first use — the sheet is constructed before any evidence
    /// exists.
    pub(super) fn update(&mut self, evidence: &RowStillness) {
        let still = &evidence.still;
        if self.run.len() != still.len() {
            self.run = vec![0; still.len()];
            self.bot_run = vec![0; still.len()];
            self.verified = vec![0; still.len()];
            self.bot_verified = vec![0; still.len()];
        }
        for (j, &s) in still.iter().enumerate() {
            self.run[j] = if s { self.run[j].saturating_add(1) } else { 0 };
            self.verified[j] = if s && evidence.verified[j] {
                self.verified[j].saturating_add(1)
            } else {
                0
            };
        }
        // The bottom masks count from the BOTTOM edge: index k is the
        // k-th sampled row up from the bottom.
        for k in 0..still.len() {
            let j = still.len() - 1 - k;
            self.bot_run[k] = if still[j] {
                self.bot_run[k].saturating_add(1)
            } else {
                0
            };
            self.bot_verified[k] = if still[j] && evidence.verified[j] {
                self.bot_verified[k].saturating_add(1)
            } else {
                0
            };
        }
        // The top prefix: rows [0, prefix) all still THIS frame.
        self.prefix = still.iter().take_while(|&s| *s).count();
        self.bot_prefix = still.iter().rev().take_while(|&s| *s).count();
    }

    /// Exact pixel-row height of the live stationary top prefix, refined
    /// within the trailing `step` grid interval and capped at
    /// [`MAX_BAND_FRACTION`] so fixed-backdrop overlap writes never smear a
    /// sub-grid navbar border before [`Self::top_commit`] fires.
    pub(super) fn live_top_rows(
        &self,
        base: &RowProfile,
        cur: &RowProfile,
        cost_threshold: f32,
    ) -> usize {
        if self.prefix == 0 || base.stride != cur.stride || base.height != cur.height {
            return 0;
        }
        let h = cur.height as usize;
        let step = cur.step.max(1);
        let cap = (cur.height as f32 * MAX_BAND_FRACTION) as usize;
        let probe_start = ((self.prefix - 1) * step + 1).min(h);
        let probe_end = (self.prefix * step).min(h);
        let mut rows = probe_start;
        for y in probe_start..probe_end {
            let c = &cur.data[y * cur.stride..(y + 1) * cur.stride];
            let b = &base.data[y * cur.stride..(y + 1) * cur.stride];
            if row_mad(c, b) <= cost_threshold {
                rows = y + 1;
            } else {
                break;
            }
        }
        rows.min(cap)
    }

    /// Bottom-edge mirror of [`Self::live_top_rows`].
    pub(super) fn live_bot_rows(
        &self,
        base: &RowProfile,
        cur: &RowProfile,
        cost_threshold: f32,
    ) -> usize {
        if self.bot_prefix == 0 || base.stride != cur.stride || base.height != cur.height {
            return 0;
        }
        let h = cur.height as usize;
        let step = cur.step.max(1);
        let cap = (cur.height as f32 * MAX_BAND_FRACTION) as usize;
        let probe_start = ((self.bot_prefix - 1) * step + 1).min(h);
        let probe_end = (self.bot_prefix * step).min(h);
        let mut rows = probe_start;
        for k in probe_start..probe_end {
            let y = h - 1 - k;
            let c = &cur.data[y * cur.stride..(y + 1) * cur.stride];
            let b = &base.data[y * cur.stride..(y + 1) * cur.stride];
            if row_mad(c, b) <= cost_threshold {
                rows = k + 1;
            } else {
                break;
            }
        }
        rows.min(cap)
    }

    /// Whether the top band should commit now, and at what height (in
    /// canvas rows). Requires the still prefix to hold at least
    /// [`MIN_BAND_GRIDS`] displacement-verified grids, structure inside
    /// the band, and a height between the current band and the fraction
    /// cap.
    pub(super) fn top_commit(&self, cur: &RowProfile, viewport_height: u32) -> Option<u32> {
        let step = cur.step.max(1);
        let skip = (self.top as usize).div_ceil(step);
        let rows = self.commit_height(&self.run, &self.verified, skip, step, viewport_height)?;
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
        if cur.height == 0 {
            return None;
        }
        let step = cur.step.max(1);
        let skip = (self.bot as usize).div_ceil(step);
        let rows = self.commit_height(
            &self.bot_run,
            &self.bot_verified,
            skip,
            step,
            viewport_height,
        )?;
        // Mirror `top_commit`: only sample rows strictly inside the candidate
        // bottom band `[cur.height - rows, cur.height)`. When `j * step == rows`,
        // `last - rows` lands on the first scrolling content row just above the
        // band, which would let a blank bottom margin pin itself as a sticky
        // footer off the texture of scrolling text above it.
        let last = cur.height as usize - 1;
        let structured = (skip..)
            .take_while(|&j| ((j * step) as u32) < rows)
            .filter_map(|j| last.checked_sub(j * step))
            .any(|y| grid_row_structured(cur, y));
        (structured && rows > self.bot).then_some(rows)
    }

    /// Shared commit-height test for one edge's run tables: how far the
    /// still prefix extends from the anchored edge, TRIMMED to the last
    /// contiguous cluster of displacement-verified grids. The offset-0
    /// run may run far past it (flat background strips are still too —
    /// that is exactly what the translation probe is for); rows past
    /// the cluster stay content rather than expanding the exclusion into
    /// ordinary page margins. Three shapes matter:
    ///
    /// - A self-similar band interior collides with its own rows dy
    ///   away (chrome row vs chrome row) and never verifies — grids
    ///   before the FIRST verified one are a free lead, so uniform
    ///   chrome still commits at its full extent.
    /// - A flat strip below the chrome is still but verifies only at
    ///   its bottom edge, where its translated partner crosses into
    ///   real content — a disconnected cluster. The gap limit drops
    ///   it, and the height stops at the chrome.
    /// - Grids after a non-still grid (page content moving through)
    ///   are unreachable — bands stay edge-anchored.
    ///
    /// `skip` is how many anchored grids the already-committed band
    /// covers; grid `j` attests to rows `[j·step, (j+1)·step)` from
    /// the edge.
    pub(super) fn commit_height(
        &self,
        run: &[u16],
        verified: &[u16],
        skip: usize,
        step: usize,
        viewport_height: u32,
    ) -> Option<u32> {
        let mut far = skip;
        let mut cluster = 0usize;
        let mut cluster_end = skip;
        let mut gap = 0usize;
        for (i, &r) in run.iter().enumerate().skip(skip) {
            if r < STILL_FRAMES {
                break;
            }
            if verified[i] >= COMMIT_FRAMES {
                cluster += 1;
                cluster_end = i + 1;
                gap = 0;
            } else if cluster > 0 {
                gap += 1;
                if gap > VERIFIED_GAP_GRIDS {
                    break;
                }
            }
            // else: still the pre-first-verified lead — self-similar
            // band interior colliding with its own rows dy away. Free:
            // the verified grids below attest the rows above them.
        }
        if cluster >= MIN_BAND_GRIDS {
            far = cluster_end;
        }
        if far == skip {
            return None;
        }
        let rows = (far * step) as u32;
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

/// Per-sampled-row stillness evidence of `cur` against `base`, probed
/// at the same viewport position (offset 0) AND at the accepted
/// translation (offset `dy`). The grid matches `cur.step` so the band
/// geometry and the matcher's scan share one coordinate system.
pub(super) struct RowStillness {
    /// Offset-0 stillness: the row is near-identical to the base at the
    /// same viewport position. On an accepted moving frame this means
    /// "pinned or featureless" — never "nothing scrolled". Keeps the
    /// run (and the live prefix) contiguous.
    pub(super) still: Vec<bool>,
    /// Displacement-verified stillness: `still` AND a misfit against
    /// the base at offset dy. The three row kinds separate only under
    /// the second probe — page content matches TRANSLATED (it IS the
    /// translation), featureless background matches BOTH, pinned
    /// chrome matches ONLY offset 0. A row whose translated partner
    /// falls outside the frame inherits `still`: the edge opposite the
    /// scroll cannot testify (a top band on an up-scroll, a footer on
    /// a down-scroll), and those rows are chrome in practice.
    pub(super) verified: Vec<bool>,
}

pub(super) fn stillness_grid(
    base: &RowProfile,
    cur: &RowProfile,
    options: &SheetLimits,
    dy: i32,
    fixed_backdrop: bool,
) -> RowStillness {
    if base.stride != cur.stride || base.height != cur.height {
        return RowStillness {
            still: Vec::new(),
            verified: Vec::new(),
        };
    }
    let h = cur.height as usize;
    let step_abs = dy.unsigned_abs() as usize;
    let edge_cap = (cur.height as f32 * MAX_BAND_FRACTION) as usize;
    let mut still = Vec::with_capacity(h.div_ceil(cur.step.max(1)));
    let mut verified = Vec::with_capacity(still.capacity());
    let mut j = 0usize;
    while j < h {
        let c = &cur.data[j * cur.stride..(j + 1) * cur.stride];
        let b = &base.data[j * cur.stride..(j + 1) * cur.stride];
        let s = row_mad(c, b) <= options.cost_threshold;
        let v = if !s {
            false
        } else if fixed_backdrop {
            // Over a textured screen-fixed backdrop, empty backdrop gaps are
            // both offset-0-still AND different from `base[j + dy]`. Require
            // occlusion evidence instead: at least one row within `|dy|`
            // rows inward of `j` changed (foreground crossed the boundary),
            // while the outer prefix/suffix through `j` stayed still.
            if step_abs == 0 {
                false
            } else if j <= edge_cap {
                let end = (j + step_abs).min(h.saturating_sub(1));
                (j + 1..=end).step_by(cur.step.max(1)).any(|inward| {
                    row_mad(
                        &cur.data[inward * cur.stride..(inward + 1) * cur.stride],
                        &base.data[inward * cur.stride..(inward + 1) * cur.stride],
                    ) > options.cost_threshold
                })
            } else if h - 1 - j <= edge_cap {
                let start = j.saturating_sub(step_abs);
                (start..j).step_by(cur.step.max(1)).any(|inward| {
                    row_mad(
                        &cur.data[inward * cur.stride..(inward + 1) * cur.stride],
                        &base.data[inward * cur.stride..(inward + 1) * cur.stride],
                    ) > options.cost_threshold
                })
            } else {
                false
            }
        } else {
            // Displacement verification: under the accepted step, the
            // content of viewport row j pairs with base row j + dy. The
            // matcher's own "these rows align" bar doubles as the "this
            // row moved with the page" bar.
            let t = j as i64 + i64::from(dy);
            if t < 0 || t >= i64::from(cur.height) {
                // Off the frame: the edge opposite the scroll cannot
                // testify; inherit the offset-0 verdict.
                true
            } else {
                let t = t as usize;
                row_mad(c, &base.data[t * cur.stride..(t + 1) * cur.stride])
                    >= options.cost_threshold
            }
        };
        still.push(s);
        verified.push(v);
        j += cur.step;
    }
    RowStillness { still, verified }
}

/// Mean absolute difference of two luma rows.
fn row_mad(a: &[u8], b: &[u8]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| u32::from(x.abs_diff(*y)))
        .sum::<u32>() as f32
        / a.len() as f32
}

#[cfg(test)]
mod tests {
    use super::{COMMIT_FRAMES, RowProfile, SheetLimits, StickyBands, stillness_grid};
    use crate::model::scroll_sheet::profile::BAND_COUNT;

    fn render_viewport(
        width: u32,
        height: u32,
        scroll_top: u32,
        structured_footer: bool,
    ) -> Vec<u8> {
        const FOOTER_ROWS: u32 = 8;
        let mut rgba = vec![255u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                let px = if y >= height - FOOTER_ROWS {
                    if structured_footer {
                        // Stationary structured status bar inside the bottom 8 rows.
                        let glyph = (x / 6 + y).is_multiple_of(5) && (16..width - 16).contains(&x);
                        let v = if glyph { 36 } else { 228 };
                        [v, v, v, 255]
                    } else {
                        // Completely flat blank bottom margin.
                        [248, 248, 250, 255]
                    }
                } else {
                    // Non-periodic scrolling content right down to `height - FOOTER_ROWS - 1`
                    // (the exact boundary row `last - rows` just above the margin).
                    let doc_y = y + scroll_top;
                    let hash = doc_y
                        .wrapping_mul(73)
                        .wrapping_add(x.wrapping_mul(37))
                        .wrapping_add((doc_y / 3) ^ (x / 5));
                    let ink = (16..width - 16).contains(&x) && (hash % 7 < 3);
                    let v = if ink { 24 } else { 248 };
                    [v, v, v, 255]
                };
                rgba[idx..idx + 4].copy_from_slice(&px);
            }
        }
        rgba
    }

    #[test]
    fn blank_bottom_margin_below_scrolling_text_never_commits_as_footer() {
        const W: u32 = 160;
        const H: u32 = 200;
        const STEP_DY: i32 = 16;

        let limits = SheetLimits::default();
        let mut blank_bands = StickyBands::new();
        let mut prev_blank = RowProfile::from_rgba(&render_viewport(W, H, 0, false), W, H);

        let mut footer_bands = StickyBands::new();
        let mut prev_footer = RowProfile::from_rgba(&render_viewport(W, H, 0, true), W, H);

        for frame_idx in 1..=u32::from(COMMIT_FRAMES) + 2 {
            let scroll_top = frame_idx * STEP_DY as u32;

            let cur_blank = RowProfile::from_rgba(&render_viewport(W, H, scroll_top, false), W, H);
            let blank_ev = stillness_grid(&prev_blank, &cur_blank, &limits, STEP_DY, false);
            blank_bands.update(&blank_ev);
            assert_eq!(
                blank_bands.bot_commit(&cur_blank, H),
                None,
                "blank bottom margin must not commit off scrolling text at frame {frame_idx}"
            );
            prev_blank = cur_blank;

            let cur_footer = RowProfile::from_rgba(&render_viewport(W, H, scroll_top, true), W, H);
            let footer_ev = stillness_grid(&prev_footer, &cur_footer, &limits, STEP_DY, false);
            footer_bands.update(&footer_ev);
            if frame_idx >= u32::from(COMMIT_FRAMES) && footer_bands.bot == 0 {
                let committed = footer_bands.bot_commit(&cur_footer, H);
                assert_eq!(committed, Some(8));
                footer_bands.bot = 8;
            }
            prev_footer = cur_footer;
        }

        assert_eq!(footer_bands.bot, 8);
    }

    #[test]
    fn bot_commit_handles_zero_and_tiny_heights_without_underflow() {
        let mut bands = StickyBands::new();
        bands.bot_run = vec![COMMIT_FRAMES; 4];
        bands.bot_verified = vec![COMMIT_FRAMES; 4];

        let empty_profile = RowProfile {
            height: 0,
            stride: 16,
            step: 1,
            data: Vec::new(),
            bands: [(0, 0); BAND_COUNT],
            band_means: Vec::new(),
            spread: 0,
            row_weight: Vec::new(),
        };
        assert_eq!(bands.bot_commit(&empty_profile, 64), None);

        // `cur.height == 2` while `commit_height` proposes `rows == 2` (`rows == height`)
        // and `rows == 4` (`rows > cur.height` when `viewport_height` is larger): neither
        // may underflow `last - j * step`.
        let mut structured_row = vec![20u8; 32];
        structured_row[0] = 220;
        structured_row[16] = 220;
        let tiny_structured = RowProfile {
            height: 2,
            stride: 16,
            step: 1,
            data: structured_row,
            bands: [(0, 16); BAND_COUNT],
            band_means: vec![[120.0; BAND_COUNT]; 2],
            spread: 0,
            row_weight: vec![1.0; 2],
        };
        bands.bot_run = vec![COMMIT_FRAMES; 2];
        bands.bot_verified = vec![COMMIT_FRAMES; 2];
        assert_eq!(bands.bot_commit(&tiny_structured, 8), Some(2));

        bands.bot_run = vec![COMMIT_FRAMES; 4];
        bands.bot_verified = vec![COMMIT_FRAMES; 4];
        assert_eq!(bands.bot_commit(&tiny_structured, 32), Some(4));

        let tiny_flat = RowProfile {
            data: vec![200u8; 32],
            ..tiny_structured
        };
        assert_eq!(bands.bot_commit(&tiny_flat, 8), None);
        assert_eq!(bands.bot_commit(&tiny_flat, 32), None);
    }
}
