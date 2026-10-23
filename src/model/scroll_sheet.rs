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
//! - **One position base, and refusals write nothing** — a frame is only
//!   ever placed against the last frame the sheet actually advanced to
//!   (`stable_profile`). Measuring the step against *any* nearer frame
//!   ("why not re-anchor on the frame we just saw?") understates it by
//!   the rows the sheet never advanced through, and the whole-viewport
//!   write then paints displaced pixels over rows that were already
//!   right: one refused frame corrupted 475 rows of a 1 864-row canvas
//!   in the probe that found this. So an unplaceable frame is REFUSED:
//!   no write, no growth, the canvas stays byte-identical, and the base
//!   stays where it was until a frame overlaps it again — a tear or a
//!   fast flick then costs the band nothing could bridge instead of
//!   poisoning a viewport of good content. The previous frame is still
//!   remembered, but only as evidence for the duplicate shortcut and the
//!   fixed-column mask, never as a position.
//! - **Only rows that can testify vote** — a vertical translation is
//!   observable through vertical VARIATION alone. A row identical to its
//!   neighbour could sit anywhere in the other frame and still "match",
//!   so averaging it in does not merely dilute the verdict, it inverts
//!   it: on a mostly-blank page two blank regions match at ANY distance
//!   for cost ≈ 0, and the cheapest offset is decided by leftover
//!   texture. Every cost is therefore a weighted mean over rows whose
//!   weight is how much they differ from their neighbour, and a candidate
//!   with too little weighted evidence is refused instead of ranked.
//!   Without this the sparse-content fixture accepted `dy = -474` for a
//!   272-row scroll and rewrote the sheet in the wrong place.
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

//! The impl is split by concern: [`profile`] (frame profiles + the
//! band mask), [`sticky`] (fixed-chrome detection), [`matcher`] (the
//! offset search and its refusal taxonomy) and [`preview`] (the live
//! panel's geometry and the HUD phase ladder). The tests stay in this
//! file: one fixture set drives the whole pipeline through
//! [`ScrollSheet::push`].

use matcher::{Placement, estimate_offset};
use profile::{MotionMask, RowProfile};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use sticky::{StickyBands, stillness_grid};

pub(crate) use preview::phase_of;
pub use preview::{
    PREVIEW_DISPLAY_W, PREVIEW_SAMPLE_W, Phase, preview_fit_rows, preview_scale, preview_slice,
    preview_window,
};

/// Tuning knobs for [`ScrollSheet`]. Defaults are tuned for text
/// content under manual scrolling (the step is whatever the user's
/// finger did); the thresholds hold across the whole admissible window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetLimits {
    /// Offsets above this fraction of the viewport height are never
    /// considered — a frame that jumped that far either lost all overlap
    /// or matched a wrong periodic copy. It is only a sanity bound: the
    /// real floor is [`SheetLimits::min_overlap`], which still has to
    /// leave `min_overlap` rows to verify a candidate. Manual scrolling
    /// makes the step whatever the user's finger did, so the bound sits
    /// as high as the overlap budget allows; a tighter one refused every
    /// frame of a brisk trackpad flick.
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
    /// Hard cap on canvas height — the capture loop stops (and
    /// reports) when the sheet would exceed it. Guards against
    /// runaway captures on infinite-scroll feeds.
    pub max_height: u32,
}

impl Default for SheetLimits {
    fn default() -> Self {
        Self {
            max_motion_ratio: 0.8,
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
    /// The motion is bigger than the overlap budget: either every
    /// candidate would leave less than [`SheetLimits::min_overlap`] rows
    /// of overlap, or the best candidate sits at the very edge of the
    /// search window with a bad residual — the frame moved further than
    /// one capture interval could bridge. Actionable by the user (scroll
    /// slower) and self-healing (the base is unchanged, so a later frame
    /// that still overlaps it links up with its full displacement).
    NoOverlap,
    /// Multiple offsets fit equally well (periodic texture). Not
    /// force-fit; a later frame usually escapes the pattern.
    Unclear,
    /// The best offset is still a bad match — content changed shape
    /// (mid-animation tear, page navigation, video). Unlike
    /// [`Refusal::NoOverlap`] this says the AREA is unstable; scrolling
    /// slower will not help.
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

/// Decision-log sink for a live capture,
/// `SHOTORI_DEBUG_SHEET_LOG=<path>`: one line per pushed frame and one
/// per band splice. When a stitched output looks wrong, this is the
/// recording of every judgment the sheet made — measured step, costs,
/// refusals, splices — which is the only way to debug a session after
/// the fact (the frames themselves are gone). Best-effort by design:
/// an unwritable path must never touch the capture. Release builds
/// with the variable unset do not read it.
fn sheet_log_path() -> &'static Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::var("SHOTORI_DEBUG_SHEET_LOG")
            .ok()
            .map(PathBuf::from)
    })
}

fn sheet_log(msg: &str) {
    if let Some(path) = sheet_log_path() {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut file) => {
                if let Err(err) = writeln!(file, "{msg}") {
                    eprintln!("sheet log write failed: {err}");
                }
            }
            Err(err) => eprintln!("sheet log open failed: {err}"),
        }
    }
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
    /// The PREVIOUS frame seen (raw bytes — the exact duplicate test
    /// memcmp's it). Purely a "what did the screen just look like"
    /// reference: the duplicate shortcut and the fixed-column mask. It
    /// is NOT a position base — see [`ScrollSheet::stable_profile`].
    prev_raw: Vec<u8>,
    prev_profile: RowProfile,
    /// Sticky-band detection state (see [`StickyBands`]).
    sticky: StickyBands,
    /// The profile of the last frame ACCEPTED into the canvas: the one
    /// and only comparison base for placement. A frame that cannot be
    /// measured against it is refused rather than folded against a
    /// remembered substitute, because a guessed position writes correct
    /// pixels over rows that were already right (see `place`).
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
            prev_raw: seed,
            prev_profile: profile.clone(),
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
        self.prev_raw = seed.clone();
        self.prev_profile = RowProfile::from_rgba(&seed, self.width, self.viewport_height);
        self.stable_profile = self.prev_profile.clone();
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
        if frame == self.prev_raw.as_slice() {
            // Poll outran the repaint: not even a Still — the anchor
            // must stay so the next real frame diffs against fresh bytes.
            return Fold::Unchanged;
        }

        let profile = RowProfile::from_rgba(frame, self.width, self.viewport_height);
        let outcome = self.place(frame, &profile);
        // Every non-duplicate frame becomes the previous-frame reference,
        // refused ones included: the fixed-column mask needs two ADJACENT
        // frames to tell "this band never moves" from "this band moved a
        // lot", and a duplicate must never become that reference. The
        // POSITION base (`stable_profile`) is not touched here — only
        // `place` advances it, and only on an accept.
        self.prev_raw.clear();
        self.prev_raw.extend_from_slice(frame);
        self.prev_profile = profile;
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
            sheet_log(&format!(
                "#{} NoOverlap (no admissible candidate)",
                self.frames_pushed
            ));
            return Placement::Reject(Refusal::NoOverlap);
        };
        if estimate.cost > self.options.cost_threshold {
            // A bad residual at the very EDGE of the search window means
            // the frame moved further than the window could bridge: the
            // honest reading is "no overlap left", not "the content
            // changed on its own". The distinction is the HUD's whole
            // message — "slow down" versus "this area will not sit still".
            if estimate.dy.abs() >= estimate.window - 2 {
                sheet_log(&format!(
                    "#{} Refused(NoOverlap) dy={} cost={:.2} at window edge {}",
                    self.frames_pushed, estimate.dy, estimate.cost, estimate.window
                ));
                return Placement::Reject(Refusal::NoOverlap);
            }
            sheet_log(&format!(
                "#{} Refused(Mismatch) dy={} cost={:.2}",
                self.frames_pushed, estimate.dy, estimate.cost
            ));
            return Placement::Reject(Refusal::Mismatch);
        }
        if estimate.dy == 0 {
            return Placement::Still;
        }
        if estimate.runner_up - estimate.cost < self.options.ambiguity_margin {
            sheet_log(&format!(
                "#{} Refused(Unclear) dy={} cost={:.2} runner={:.2} margin={:.2}",
                self.frames_pushed,
                estimate.dy,
                estimate.cost,
                estimate.runner_up,
                estimate.runner_up - estimate.cost
            ));
            return Placement::Reject(Refusal::Unclear);
        }
        Placement::Match(estimate)
    }

    fn place(&mut self, frame: &[u8], profile: &RowProfile) -> Fold {
        // Low-information guard: a near-uniform anchor cannot be aligned
        // by any translation — the
        // conservative answer is Still; a later textured frame brings
        // something to match against. Flat PDF margins hit this.
        if self.stable_profile.spread <= 1 {
            return Fold::Still;
        }
        // Sidebar evidence: compare this frame against the previous one
        // at the same position and down-weight bands that did not move.
        self.motion.observe(&self.prev_profile, profile);
        // Position comes from ONE base: the last frame actually folded
        // into the sheet. There used to be a second, "riding" anchor
        // (every seen frame) that took over when the step outgrew the
        // matcher's window. It was a trap: a dy measured from a frame
        // the sheet never advanced to understates the displacement by
        // the lost band, so the frame was written a band too HIGH and
        // the newest-wins write painted displaced pixels over a whole
        // viewport of already-correct rows (measured: one refused frame
        // corrupted 475 rows of the canvas). A frame whose distance from
        // the stable base cannot be measured is now simply refused —
        // nothing is written, the canvas stays exactly as it was, and
        // the next frame that overlaps the (unchanged) base links up
        // again with its full displacement.
        let estimate = match self.classify(&self.stable_profile, profile) {
            Placement::Match(e) => e,
            Placement::Still => return Fold::Still,
            Placement::Reject(reason) => return Fold::Refused(reason),
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
        // is only meaningful against the base that produced it, and the
        // displacement probe needs the step that was accepted.
        {
            let evidence =
                stillness_grid(&self.stable_profile, profile, &self.options, estimate.dy);
            if !evidence.still.is_empty() {
                self.sticky.update(&evidence, next.position);
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
        sheet_log(&format!(
            "#{} Match dy={} cost={:.2} margin={:.2} branch={branch:?} growth={growth} pos={} top={} ins={} prefix={}",
            self.frames_pushed,
            estimate.dy,
            estimate.cost,
            estimate.runner_up - estimate.cost,
            self.state.position,
            self.sticky.top,
            self.sticky.inserted,
            self.sticky.prefix,
        ));
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
            sheet_log(&format!(
                "SPLICE band rows {m0}→{t} (+{g}, inserted={}, pos={})",
                self.sticky.inserted, self.state.position
            ));
        }
        self.sticky.top = t;
        self.sticky.pending.clear();
    }
}

mod matcher;
mod preview;
mod profile;
mod sticky;
#[cfg(test)]
mod tests;
