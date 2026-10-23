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
//! - **Separated sidebar composition** — a right-hand column separated by a
//!   uniform gutter keeps its own accepted position and coherent pixel canvas.
//!   It may scroll with the body, pin, resume or reverse without repeating its
//!   text through the body's coordinate system. Column refusals retain the
//!   column's last verified pixels while the body advances. Extra canvas uses
//!   the captured gutter background; uncertain padding retains an incomplete
//!   status. New content moving across the gutter restores document-flow
//!   ownership; previously captured column pixels keep their column position.
//!   Padding never claims to recover unobserved sidebar content.
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
//!   extends the match exclusion. Late overlays never insert canvas rows;
//!   only an existing seed header reserves space at the sheet top. See
//!   [`StickyBands`].
//! - **Visible-content composition** — accepted frames refresh the viewport,
//!   except for localized stationary overlays detected from edges that stay
//!   fixed while the surrounding page moves. These regions retain previously
//!   captured pixels; later unobstructed observations repair seed occlusions.
//!   Bidirectional overlap evidence covers controls clipped by capture edges.
//!   A bounded halo includes the button face, antialiasing and nearby shadow.
//!   Each retained span remembers its viewport row and whether it needs repair.
//!   Edge quarters prefer more interior observations, including before a new
//!   sticky bar is detected. Near floating controls this extends to the edgeward
//!   half to avoid repeated fades and backdrop blur. Interior samples refresh
//!   normally; prepends preserve provenance.
//!   Regions never observed unobstructed retain their captured pixels; the
//!   compositor does not invent missing text or paint a guessed background.
//!   Stationary decorations connected to a confirmed bottom frame use a pixel
//!   mask instead of a control's rectangular halo. The frame is kept once at
//!   the output perimeter, outside both document and independent column planes.
//!   Covered column samples remain repairable when clear observations arrive.
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
//! scrolling can drop/duplicate one seam row per step), broad or animated
//! floating panels and content never exposed by scrolling cannot be recovered,
//! collapsing toolbars keep their tallest extent (rows a shrunken band exposes
//! are lost, not misplaced), single contiguous sheet (no tiling for
//! multi-hundred-MB captures).

//! The impl is split by concern: [`profile`] (frame profiles + the
//! band mask), [`sticky`] (edge chrome), [`floating`] (local overlays and
//! overlap composition), [`matcher`] (the offset search and its refusal
//! taxonomy) and [`preview`] (the live panel's geometry and the HUD phase
//! ladder). In-file fixtures and font-rasterized `scene_tests` exercise the
//! complete pipeline through [`ScrollSheet::push`].

use floating::FloatingOverlays;
use matcher::{Placement, estimate_offset};
use profile::{MotionMask, RowProfile};
use sidebar::Sidebar;
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
/// per accepted placement. When a stitched output looks wrong, this is the
/// recording of every judgment the sheet made — measured step, costs,
/// refusals and edge bands — which helps debug a session after
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
    floating: FloatingOverlays,
    sidebar: Option<Box<Sidebar>>,
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
    /// Preview invalidation revision for prepends, axis switches and
    /// floating-overlay repair within an existing viewport.
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
        Self::new_internal(width, viewport_height, first_frame, options, axis, true)
    }

    fn new_internal(
        width: u32,
        viewport_height: u32,
        first_frame: &[u8],
        options: SheetLimits,
        axis: Axis,
        detect_sidebar: bool,
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
            sidebar: if detect_sidebar && axis == Axis::Vertical {
                Sidebar::new(sheet_w, sheet_h, &seed, options)
            } else {
                None
            },
            floating: FloatingOverlays::new(&seed, sheet_w),
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
        if self.axis_locked || self.frames_accepted > 1 || self.state.position != 0 {
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
        self.floating = FloatingOverlays::new(&seed, self.width);
        self.sidebar = if axis == Axis::Vertical {
            Sidebar::new(self.width, self.viewport_height, &seed, self.options)
        } else {
            None
        };
        self.sticky = StickyBands::new();
        self.motion = MotionMask::default();
        self.last_dy = 0;
        self.state = ViewportState::default();
        self.axis = axis;
        self.top_shifts = self.top_shifts.saturating_add(1);
        true
    }

    fn height(&self) -> u32 {
        (i64::from(self.viewport_height) + self.state.max_position) as u32
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

    /// A separated column could not be placed or its uncovered canvas has no
    /// observed plain background. The body can still advance independently.
    pub(crate) fn has_incomplete_column(&self) -> bool {
        self.sidebar
            .as_ref()
            .is_some_and(|column| column.is_incomplete())
    }

    /// Content revision for prepends and occlusion repair. The live preview
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
        self.push_with_document_motion(frame, None)
    }

    fn push_with_document_motion(&mut self, frame: &[u8], document_motion: Option<i32>) -> Fold {
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
        let outcome = self.place(frame, &profile, document_motion);
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
    fn classify(
        &self,
        base: &RowProfile,
        cur: &RowProfile,
        document_motion: Option<i32>,
    ) -> Placement {
        // Committed sticky bands extend the edge trim of the scan.
        let trim = (self.sticky.top, self.sticky.bot);
        let mut weights = self.motion.weights;
        if let Some(sidebar) = &self.sidebar {
            for (weight, &(_, end)) in weights.iter_mut().zip(&cur.bands) {
                if self.width as usize / 8 + end > sidebar.left() {
                    *weight = 0.;
                }
            }
        }
        let Some(mut estimate) = estimate_offset(
            base,
            cur,
            &self.options,
            trim,
            &weights,
            document_motion.unwrap_or(self.last_dy),
        ) else {
            sheet_log(&format!(
                "#{} NoOverlap (no admissible candidate)",
                self.frames_pushed
            ));
            return Placement::Reject(Refusal::NoOverlap);
        };
        // A whole-frame candidate must also match the body independently.
        // Its search neighbourhood covers abrupt reversals missed by sparse
        // body signatures; a stationary sidebar cannot dictate the offset.
        if self.sidebar.is_some()
            && (estimate.cost > self.options.cost_threshold
                || estimate.runner_up - estimate.cost < self.options.ambiguity_margin)
            && let Some(full) = estimate_offset(
                base,
                cur,
                &self.options,
                trim,
                &self.motion.weights,
                document_motion.unwrap_or(self.last_dy),
            )
            && full.cost <= self.options.cost_threshold
            && full.runner_up - full.cost >= self.options.ambiguity_margin
            && let Some(body) = estimate_offset(base, cur, &self.options, trim, &weights, full.dy)
            && body.dy == full.dy
            && body.cost <= self.options.cost_threshold
        {
            estimate = full;
        }
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
        // A synchronized column can use independently verified document
        // motion to resolve a weak but unique pixel-perfect match. Exact
        // periodic ties and columns already moving independently cannot.
        let coherent_column = document_motion == Some(estimate.dy)
            && estimate.cost <= f32::EPSILON
            && estimate.runner_up > estimate.cost;
        if !coherent_column && estimate.runner_up - estimate.cost < self.options.ambiguity_margin {
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

    fn place(&mut self, frame: &[u8], profile: &RowProfile, document_motion: Option<i32>) -> Fold {
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
        let estimate = match self.classify(&self.stable_profile, profile, document_motion) {
            Placement::Match(e) => e,
            Placement::Still => {
                // A late column repaint can recover while the body is idle.
                // Refresh that column alone and invalidate the preview cache.
                let height = self.height();
                if let Some(sidebar) = &mut self.sidebar
                    && sidebar.is_incomplete()
                {
                    sidebar.observe(
                        frame,
                        0,
                        0,
                        height,
                        self.sticky.top as usize..(self.viewport_height - self.sticky.bot) as usize,
                        &self.floating.frame_edge,
                    );
                    sidebar.composite(
                        &mut self.canvas,
                        self.width as usize,
                        self.sticky.bot as usize,
                    );
                    self.floating.frame_edge.composite(
                        &mut self.canvas,
                        frame,
                        self.state.position as usize,
                    );
                    self.top_shifts = self.top_shifts.saturating_add(1);
                    return Fold::Shifted { dy: 0 };
                }
                return Fold::Still;
            }
            Placement::Reject(reason) => return Fold::Refused(reason),
        };

        let (next, branch, growth) = self.state.transition(estimate.dy);
        if u64::from(self.viewport_height) + next.max_position as u64
            > self.options.max_height as u64
        {
            return Fold::Refused(Refusal::TooTall);
        }

        let had_floating = !self.floating.is_empty();
        self.floating.observe_frame_edge(frame, estimate.dy);
        self.sticky.seed_top = self
            .sticky
            .seed_top
            .min(self.floating.frame_edge.seed_prefix_rows() as u32);
        if let Some(sidebar) = &mut self.sidebar {
            let prepend = if branch == Branch::Above { growth } else { 0 };
            sidebar.observe(
                frame,
                estimate.dy,
                prepend,
                self.viewport_height + next.max_position as u32,
                self.sticky.top as usize..(self.viewport_height - self.sticky.bot) as usize,
                &self.floating.frame_edge,
            );
        }
        self.floating.observe(
            frame,
            (self.width, self.viewport_height),
            self.sidebar
                .as_ref()
                .and_then(|sidebar| sidebar.overlay_bounds()),
        );
        if had_floating || !self.floating.is_empty() {
            // Occlusion repair changes pixels even inside a stationary preview.
            self.top_shifts = self.top_shifts.saturating_add(1);
        }

        // Sticky evidence rides on the same accepted match — stillness
        // is only meaningful against the base that produced it, and the
        // displacement probe needs the step that was accepted.
        {
            let evidence =
                stillness_grid(&self.stable_profile, profile, &self.options, estimate.dy);
            if !evidence.still.is_empty() {
                self.sticky.update(&evidence);
                if let Some(t) = self.sticky.top_commit(profile, self.viewport_height) {
                    self.commit_top_band(t, frame);
                }
                if let Some(b) = self.sticky.bot_commit(profile, self.viewport_height) {
                    self.sticky.bot = b;
                }
            }
        }

        let row = self.width as usize * 4;
        let top = self.sticky.seed_top as usize;
        let prepend = if branch == Branch::Above {
            growth as usize
        } else {
            0
        };
        let retained = top + prepend..self.canvas.len() / row - self.sticky.bot as usize + prepend;
        if branch == Branch::Above && growth > 0 {
            // Grow at the top of the CONTENT area: shift the content
            // rows down (memmove semantics — copy_within handles the
            // overlap), leaving zeroed rows above for the viewport
            // write. The band area at the physical top stays put.
            let shift = growth as usize * row;
            self.floating.insert_rows(top, growth as usize);
            self.canvas.resize(self.canvas.len() + shift, 0);
            let len = self.canvas.len();
            self.canvas
                .copy_within(top * row..len - shift, top * row + shift);
            // A tentative still prefix may contain ordinary page rules. Newly
            // exposed rows have no earlier observation to retain, so seed them
            // from the capture instead of leaving transparent holes. Their
            // provenance stays repairable until a clear observation replaces it.
            let exposed_end = (top + growth as usize).min(self.viewport_height as usize);
            self.canvas[top * row..exposed_end * row]
                .copy_from_slice(&frame[top * row..exposed_end * row]);
            self.top_shifts = self.top_shifts.saturating_add(1);
        } else if branch == Branch::Below && growth > 0 {
            self.canvas
                .resize(self.canvas.len() + growth as usize * row, 0);
        }
        // The live still prefix protects earlier content from a fixed bar.
        // Only seed chrome owns a reserved header; a late bar does not shift
        // the document or get copied above it. A zero prefix must remain zero,
        // and sampling may only exclude rows covered by the observed grid.
        let live_prefix = if self.sticky.prefix == 0 {
            0
        } else {
            ((self.sticky.prefix - 1) * profile.step.max(1) + 1).min(self.viewport_height as usize)
        };
        let prefix_rows = live_prefix.min(top);
        if prefix_rows > 0 {
            let band_bytes = prefix_rows * row;
            self.canvas[..band_bytes].copy_from_slice(&frame[..band_bytes]);
        }
        // Keep any reserved seed header separate from the document plane.
        // Late overlays may disappear and expose their entire former area.
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
        self.floating.composite(
            &mut self.canvas,
            frame,
            self.width as usize,
            next.position as usize,
            start..content_end,
            retained,
        );
        if bot > 0 && branch == Branch::Below {
            // One footer, at the growing bottom. Appends are the only
            // branch that grows the canvas downward, so this is the
            // only place the footer's pixels are genuine content for
            // the final image; Shifted/Prepend leave the old footer
            // at the canvas bottom untouched.
            let footer_dst = (next.position as usize + content_end) * row;
            let footer_src = content_end * row;
            self.canvas[footer_dst..footer_dst + bot * row]
                .copy_from_slice(&frame[footer_src..self.viewport_height as usize * row]);
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.composite(
                &mut self.canvas,
                self.width as usize,
                self.sticky.bot as usize,
            );
            if sidebar.is_independent() {
                self.top_shifts = self.top_shifts.saturating_add(1);
            }
        }
        self.floating
            .frame_edge
            .composite(&mut self.canvas, frame, next.position as usize);
        self.state = next;
        self.frames_accepted += 1;
        // A non-seed frame has entered the canvas; the axis can no
        // longer be swapped losslessly.
        self.axis_locked = true;
        self.last_dy = estimate.dy;
        // The accepted frame is the new stable base (see `place`).
        self.stable_profile = profile.clone();
        sheet_log(&format!(
            "#{} Match dy={} cost={:.2} margin={:.2} branch={branch:?} growth={growth} pos={} top={} seed_top={} prefix={}",
            self.frames_pushed,
            estimate.dy,
            estimate.cost,
            estimate.runner_up - estimate.cost,
            self.state.position,
            self.sticky.top,
            self.sticky.seed_top,
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

    /// Exclude a verified top overlay without changing document coordinates.
    /// Only chrome already present in the seed owns a reserved canvas header.
    fn commit_top_band(&mut self, t: u32, frame: &[u8]) {
        // Prepends may already have moved copies of unconfirmed chrome into
        // the canvas. Only the prefix unchanged since the seed owns a header;
        // a once-blank document margin must stay eligible for content writes.
        self.sticky.seed_top = t.min(self.floating.frame_edge.seed_prefix_rows() as u32);
        let bytes = self.sticky.seed_top as usize * self.width as usize * 4;
        self.canvas[..bytes].copy_from_slice(&frame[..bytes]);
        self.sticky.top = t;
    }
}

mod floating;
mod matcher;
mod preview;
mod profile;
#[cfg(test)]
mod scene_tests;
mod sidebar;
mod sticky;
#[cfg(test)]
mod tests {
    use super::preview::STALLED_FRAMES;
    use super::profile::ALL_BANDS;
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

    /// A text-like ink row: dark glyphs on a light ground, in-row
    /// structure, aperiodic — the per-row shape of real content.
    fn ink_row(y: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(W as usize * 4);
        for x in 0..W {
            let h = ink_hash(x, y);
            let l = if h % 100 < 42 {
                30 + (h % 60) as u8
            } else {
                238 + (h % 8) as u8
            };
            v.extend_from_slice(&[l, l, l, 255]);
        }
        v
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
        assert_eq!(phase_of(1, Refusal::NoOverlap), Phase::Following);
        assert_eq!(phase_of(2, Refusal::NoOverlap), Phase::NoOverlap);
        assert_eq!(
            phase_of(STALLED_FRAMES - 1, Refusal::NoOverlap),
            Phase::NoOverlap
        );
        assert_eq!(phase_of(STALLED_FRAMES, Refusal::NoOverlap), Phase::Stalled);
        assert_eq!(phase_of(u32::MAX, Refusal::Mismatch), Phase::Stalled);
    }

    /// The chip's word comes from the matcher's verdict, not from the
    /// count alone: "no overlap" is advice about scroll speed, while
    /// unstable pixels are a property of the area.
    #[test]
    fn the_chip_word_follows_the_refusal_reason() {
        assert_eq!(phase_of(3, Refusal::NoOverlap), Phase::NoOverlap);
        assert_eq!(phase_of(3, Refusal::Mismatch), Phase::Unstable);
        assert_eq!(phase_of(3, Refusal::Unclear), Phase::Unstable);
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
            // (≤ 40 down / 30 up, overlap ≥ 56 rows).
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
        // The verified-persistence bar (COMMIT_FRAMES) needs six moving
        // frames before the band commits.
        for top in [30u32, 60, 90, 120, 150, 180] {
            s.push(&frame(top));
        }
        // Band written ONCE at the top (the seed frame already had it
        // there — no splice), content stream exact beneath it.
        assert_eq!(s.dimensions(), (W, HDR + (180 + H2 - HDR)));
        assert_eq!(s.canvas(), expected(180).as_slice());
        // The highlight reports the CONTENT segment, below the band.
        assert_eq!(s.viewport_span(), (HDR + 180, H2 - HDR));
    }

    /// Flat page margins match both stationary and translated coordinates;
    /// they must not enlarge the fixed-header exclusion.
    #[test]
    fn flat_background_below_the_chrome_never_pins() {
        const H2: u32 = 800;
        const HDR: u32 = 48;
        const FLAT: u32 = 200;
        let band = chrome_band(HDR);
        let flat_row: Vec<u8> = [24, 24, 28, 255].repeat(W as usize);
        let frame = |top: u32| -> Vec<u8> {
            let mut v = band.clone();
            for _ in 0..FLAT {
                v.extend_from_slice(&flat_row);
            }
            for row in top..top + H2 - HDR - FLAT {
                v.extend_from_slice(&source_row(row).repeat(W as usize));
            }
            v
        };

        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [30u32, 60, 90, 120, 150, 180] {
            let out = s.push(&frame(top));
            assert!(matches!(out, Fold::GrewBottom { dy: 30, .. }), "{out:?}");
        }
        // The band is exactly the chrome (step = 1 at this size, no
        // grid coarseness); the flat strip below never joined the run.
        assert_eq!(s.sticky.top, HDR);
    }

    /// Short repaint stalls can look stationary beneath a fixed header.
    /// The persistence requirement prevents them from enlarging its exclusion.
    #[test]
    fn stalled_damage_blocks_under_the_chrome_never_grow_the_band() {
        const H2: u32 = 800;
        const HDR: u32 = 48;
        const BLOCK: u32 = 16; // stale rows hugging the band's bottom edge
        let band = chrome_band(HDR);
        // Per-pixel ink rows: like real text, each row carries in-row
        // structure, so a stale block of them passes every per-frame
        // signal chrome can — stillness, displacement misfit, structure.
        let frame = |top: u32| -> Vec<u8> {
            let mut v = band.clone();
            for row in top..top + H2 - HDR {
                v.extend_from_slice(&ink_row(row));
            }
            v
        };
        // `lag` rows of the block still show the older position.
        let damaged = |top: u32, lag: u32| -> Vec<u8> {
            let mut v = band.clone();
            for row in top - lag..top - lag + BLOCK {
                v.extend_from_slice(&ink_row(row));
            }
            for row in top + BLOCK..top + H2 - HDR {
                v.extend_from_slice(&ink_row(row));
            }
            v
        };

        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [40u32, 80, 120, 160, 200, 240] {
            let out = s.push(&frame(top));
            assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        }
        assert_eq!(s.sticky.top, HDR);
        // Three accepted frames with the same stale block hugging the
        // band: offset-0 still, misfits the translation.
        for (top, lag) in [(280u32, 40u32), (320u32, 80u32), (360u32, 120u32)] {
            let out = s.push(&damaged(top, lag));
            assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        }
        assert_eq!(s.sticky.top, HDR, "a repaint stall must not grow the band");
        // Capture recovers once the block repaints.
        let out = s.push(&frame(400));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        assert_eq!(s.sticky.top, HDR);
    }

    /// A compositor tear the trim absorbs: the top strip of the frame
    /// still shows the previous position (painted first, scrolled
    /// later) while the rest aligns at the true step. The trimmed row
    /// mean accepts the frame — by design, one sticky navbar must not
    /// be able to refuse the session — and the wholesale write then
    /// stamped the stale strip over canvas rows that already held the
    /// right page content: a sheared sliver nothing ever revisits (the
    /// third GitHub capture). Matching may absorb misaligned rows; the
    /// write may not. A skipped row keeps the canvas it already had,
    /// and the row↔page mapping makes that the correct pixels.
    #[test]
    fn absorbed_tear_frames_do_not_stamp_their_stale_strip() {
        const H2: u32 = 800;
        const TEAR: u32 = 16; // well under the 1/7 trim
        let frame = |top: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            for row in top..top + H2 {
                v.extend_from_slice(&ink_row(row));
            }
            v
        };
        // A TEAR-row block mid-frame still shows `top − lag`: the
        // compositor repainted around it but not it. Rows above the
        // block moved, so the offset-0 still prefix is zero and the
        // wholesale write reaches straight over it — this is the case
        // the prefix skip cannot see.
        let torn_mid = |top: u32, lag: u32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * H2 * 4) as usize);
            let at = 300usize;
            for row in 0..at {
                v.extend_from_slice(&ink_row(top + row as u32));
            }
            for row in top - lag..top - lag + TEAR {
                v.extend_from_slice(&ink_row(row));
            }
            for row in top + at as u32 + TEAR..top + H2 {
                v.extend_from_slice(&ink_row(row));
            }
            v
        };

        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for top in [40u32, 80] {
            let out = s.push(&frame(top));
            assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        }
        let out = s.push(&torn_mid(120, 40));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        let out = s.push(&frame(160));
        assert!(matches!(out, Fold::GrewBottom { dy: 40, .. }), "{out:?}");
        // The canvas must be an exact page prefix: canvas row c is page
        // row c.
        let (_, h) = s.dimensions();
        assert_eq!(h, 160 + H2);
        let canvas = s.canvas();
        for c in 0..h as usize {
            let want = ink_row(c as u32);
            let got = &canvas[c * W as usize * 4..(c + 1) * W as usize * 4];
            assert_eq!(got, want.as_slice(), "canvas row {c} diverged");
        }
    }

    /// A mid-scroll overlay leaves its original document occurrence intact
    /// and never inserts another copy above the captured content.
    #[test]
    fn mid_scroll_pin_keeps_document_coordinates_and_a_single_original_heading() {
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
        let expected = |last: u32| -> Vec<u8> { (0..last + H2).flat_map(row_of).collect() };

        // c = 40 is the first pinned frame; the still run reaches 2 two
        // frames later, so the commit happens while scrolling through
        // pinned territory.
        let mut s = ScrollSheet::new(W, H2, &frame(0), SheetLimits::default()).unwrap();
        for c in [20u32, 40, 60, 90, 120, 150, 180, 210] {
            let out = s.push(&frame(c));
            assert!(matches!(out, Fold::GrewBottom { .. }), "frame {c}: {out:?}");
        }
        assert_eq!(s.dimensions(), (W, 210 + H2));
        assert_eq!(s.canvas(), expected(210).as_slice());
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
        for top in [40u32, 80, 120, 160, 200, 240] {
            let out = s.push(&frame(top));
            assert!(matches!(out, Fold::GrewBottom { .. }), "{out:?}");
        } // sixth verified frame → bot mask commits
        // No top band ever: the header scrolls.
        assert_eq!(s.viewport_span().0, 240);
        assert_eq!(s.dimensions(), (W, H2 + 240));
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
        for top in [40u32, 80, 120, 160, 200, 240] {
            s.push(&frame(top));
        }
        let (_, h) = s.dimensions();
        let canvas = s.canvas();
        let row = W as usize * 4;
        let mut ideal = Vec::new();
        for r in 0..240 + H2 - FTR {
            ideal.extend_from_slice(&source_row(r).repeat(W as usize));
        }
        ideal.extend_from_slice(&band);
        assert_eq!(h, 240 + H2);
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
        for top in [40u32, 80, 120, 160, 200, 240] {
            s.push(&frame(top));
        }
        s.push(&frame(160)); // scroll back up mid-list
        let (_, h) = s.dimensions();
        let canvas = s.canvas();
        let row = W as usize * 4;
        let mut ideal = Vec::new();
        for r in 0..240 + H2 - FTR {
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
        assert_eq!(h, 240 + H2);
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
        for top in [40u32, 80, 120, 160, 200, 240] {
            s.push(&frame(top));
        }
        s.push(&frame(120)); // scroll back up mid-list
        let mut ideal = Vec::new();
        ideal.extend_from_slice(&band);
        for row in 0..240 + H2 - HDR - FTR {
            ideal.extend_from_slice(&source_row(row).repeat(W as usize));
        }
        ideal.extend_from_slice(&band);
        assert_eq!(s.dimensions(), (W, H2 + 240));
        assert_eq!(s.canvas(), ideal.as_slice());
    }

    // === Realistic manual-scroll probe =================================

    /// An independently rasterized floating button: opaque face, dark glyph,
    /// antialiased rim and translucent shadow over generated page content.
    fn floating_button(frame: &mut [u8], center: (i32, i32)) {
        let height = frame.len() / (TW as usize * 4);
        for y in 0..height as i32 {
            for x in 0..TW as i32 {
                let dx = x - center.0;
                let dy = y - center.1;
                let radius = ((dx * dx + dy * dy) as f32).sqrt();
                let (value, alpha) =
                    if dx.abs() <= 1 && dy.abs() < 7 || (dy - 4).abs() < 2 && dx.abs() < 6 {
                        (35., 1.)
                    } else if radius < 12. {
                        (250., 1.)
                    } else if radius < 13. {
                        (180., 13. - radius)
                    } else if radius < 17. {
                        (80., (17. - radius) * 0.06)
                    } else {
                        continue;
                    };
                let i = (y as usize * TW as usize + x as usize) * 4;
                for channel in &mut frame[i..i + 3] {
                    *channel = (f32::from(*channel) * (1. - alpha) + value * alpha).round() as u8;
                }
            }
        }
    }

    fn assert_page_pixels(actual: &[u8], expected: &[u8]) {
        assert_eq!(actual.len(), expected.len());
        let differing = actual
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| (i % TW as usize, i / TW as usize))
            .collect::<Vec<_>>();
        assert!(
            differing.is_empty(),
            "{} incorrect pixels; first {:?}",
            differing.len(),
            differing.first()
        );
    }

    #[test]
    fn floating_button_recovers_seed_and_preserves_clean_overlap_in_both_directions() {
        let page = text_page();
        let viewport = 480;
        let frame = |top| {
            let mut frame = sample(&page, top as f32, viewport);
            floating_button(&mut frame, (TW as i32 / 2, 240));
            frame
        };
        let mut sheet =
            ScrollSheet::new(TW, viewport, &frame(200), SheetLimits::default()).unwrap();
        for top in (208..=360).step_by(8) {
            let result = sheet.push(&frame(top));
            assert!(
                matches!(result, Fold::GrewBottom { dy: 8, .. }),
                "{result:?}"
            );
        }
        assert_page_pixels(sheet.canvas(), &sample(&page, 200., viewport + 160));
        let revision = sheet.top_revision();
        for top in (120..360).step_by(8).rev() {
            let result = sheet.push(&frame(top));
            assert!(
                matches!(result, Fold::GrewTop { .. } | Fold::Shifted { .. }),
                "{result:?}"
            );
        }
        assert!(sheet.top_revision() > revision);
        assert_page_pixels(sheet.canvas(), &sample(&page, 120., viewport + 240));
    }

    #[test]
    fn floating_buttons_survive_small_steps_and_different_viewport_positions() {
        let page = text_page();
        let viewport = 640;
        for step in [1, 2, 4, 16, 40] {
            let frame = |top| {
                let mut rgba = sample(&page, top as f32, viewport);
                floating_button(&mut rgba, (85, 193));
                floating_button(&mut rgba, (182, 507));
                rgba
            };
            let mut sheet =
                ScrollSheet::new(TW, viewport, &frame(0), SheetLimits::default()).unwrap();
            for top in (step..=160).step_by(step as usize) {
                let result = sheet.push(&frame(top));
                assert!(
                    matches!(result, Fold::GrewBottom { .. }),
                    "step {step}: {result:?}"
                );
                if top >= 80 {
                    assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + top));
                }
            }
            assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + 160));
        }
    }

    #[test]
    fn floating_button_and_pinned_header_survive_prepending() {
        let page = text_page();
        let viewport = 640;
        let header = sample(&page, 2_000., 32);
        let frame = |top| {
            let mut rgba = sample(&page, top as f32, viewport);
            rgba[..header.len()].copy_from_slice(&header);
            floating_button(&mut rgba, (120, 360));
            rgba
        };
        let mut sheet =
            ScrollSheet::new(TW, viewport, &frame(200), SheetLimits::default()).unwrap();
        for top in (224..=440).step_by(24) {
            assert!(matches!(sheet.push(&frame(top)), Fold::GrewBottom { .. }));
        }
        assert_eq!(sheet.sticky.top, 32);
        for top in (128..440).step_by(24).rev() {
            assert!(matches!(
                sheet.push(&frame(top)),
                Fold::GrewTop { .. } | Fold::Shifted { .. }
            ));
        }
        let mut expected = header;
        expected.extend_from_slice(&sample(&page, 160., 440 + viewport - 160));
        assert_page_pixels(sheet.canvas(), &expected);
    }

    #[test]
    fn floating_button_never_fabricates_pixels_without_an_unobstructed_observation() {
        let page = text_page();
        let viewport = 640;
        let frame = |top| {
            let mut rgba = sample(&page, top as f32, viewport);
            floating_button(&mut rgba, (120, 550));
            rgba
        };
        let mut sheet = ScrollSheet::new(TW, viewport, &frame(0), SheetLimits::default()).unwrap();
        for top in [96, 192] {
            let capture = frame(top);
            let result = sheet.push(&capture);
            assert!(matches!(result, Fold::GrewBottom { .. }), "{result:?}");
            let src = (550 * TW as usize + 120) * 4;
            let dst = ((top as usize + 550) * TW as usize + 120) * 4;
            assert_eq!(&sheet.canvas()[dst..dst + 4], &capture[src..src + 4]);
        }
        // Earlier occlusions are now exposed; only the latest, never-visible
        // patch may retain its captured foreground.
        let known_rows = 700;
        assert_page_pixels(
            &sheet.canvas()[..known_rows * TW as usize * 4],
            &sample(&page, 0., known_rows as u32),
        );
        assert!(
            sheet
                .canvas()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
    }

    #[test]
    fn floating_button_appearance_disappearance_and_refused_frames_keep_the_page() {
        let page = text_page();
        let viewport = 480;
        let frame = |top, button| {
            let mut frame = sample(&page, top as f32, viewport);
            if button {
                floating_button(&mut frame, (TW as i32 / 2, 240));
            }
            frame
        };
        let mut sheet =
            ScrollSheet::new(TW, viewport, &frame(0, false), SheetLimits::default()).unwrap();
        for top in (24..=240).step_by(24) {
            let result = sheet.push(&frame(top, true));
            assert!(matches!(result, Fold::GrewBottom { .. }), "{result:?}");
        }
        let before = sheet.canvas().to_vec();
        let bad = frame(3_000, true);
        assert!(matches!(sheet.push(&bad), Fold::Refused(_)));
        assert_eq!(sheet.canvas(), before);
        for top in (264..=384).step_by(24) {
            assert!(matches!(
                sheet.push(&frame(top, true)),
                Fold::GrewBottom { .. }
            ));
        }
        for top in (240..384).step_by(24).rev() {
            assert!(matches!(
                sheet.push(&frame(top, false)),
                Fold::Shifted { .. }
            ));
        }
        assert!(sheet.floating.is_empty());
        assert_page_pixels(sheet.canvas(), &sample(&page, 0., viewport + 384));
    }

    #[test]
    fn page_local_button_is_preserved_and_horizontal_floating_button_is_removed() {
        let mut page_frame = sample(&text_page(), 0., 1_200);
        floating_button(&mut page_frame, (TW as i32 / 2, 280));
        let frame =
            |top: usize| page_frame[top * TW as usize * 4..(top + 480) * TW as usize * 4].to_vec();
        let mut sheet = ScrollSheet::new(TW, 480, &frame(0), SheetLimits::default()).unwrap();
        for top in (24..=240).step_by(24) {
            assert!(matches!(sheet.push(&frame(top)), Fold::GrewBottom { .. }));
            assert!(sheet.floating.is_empty());
        }
        assert_page_pixels(sheet.canvas(), &page_frame[..720 * TW as usize * 4]);

        let page = text_page();
        let horizontal = |top| {
            let mut rgba = sample(&page, top as f32, 480);
            floating_button(&mut rgba, (120, 240));
            transpose_rgba(&rgba, TW, 480)
        };
        let mut sheet = ScrollSheet::new_with_axis(
            480,
            TW,
            &horizontal(0),
            SheetLimits::default(),
            Axis::Horizontal,
        )
        .unwrap();
        for top in (24..=240).step_by(24) {
            assert!(matches!(
                sheet.push(&horizontal(top)),
                Fold::GrewBottom { .. }
            ));
        }
        assert_page_pixels(sheet.canvas(), &sample(&page, 0., 720));
    }

    const TW: u32 = 240;
    const TH: u32 = 6_000;

    fn ink_hash(x: u32, y: u32) -> u32 {
        x.wrapping_mul(0x9E37_79B1)
            .wrapping_add(y.wrapping_mul(0x85EB_CA77))
            .wrapping_mul(2654435761)
            >> 11
    }

    /// A text-like page: lines of per-pixel ink on a light ground. Unlike
    /// the `source_row` fixtures, a row is NOT one constant colour — that
    /// is what makes this page a fair stand-in for a real document.
    fn text_page() -> Vec<u8> {
        let mut page = vec![246u8; (TW * TH) as usize];
        let mut y = 0u32;
        let mut line = 0u32;
        while y < TH {
            let line_h = 13 + line * 7 % 12; // 13..=24 rows
            let ink_rows = (y + line_h * 3 / 4).min(TH);
            for gy in y..ink_rows {
                for x in 0..TW {
                    let h = ink_hash(x, gy);
                    let px = if h % 100 < 42 {
                        30 + (h % 60) as u8
                    } else {
                        238 + (h % 8) as u8
                    };
                    page[(gy * TW + x) as usize] = px;
                }
            }
            y += line_h;
            line += 1;
        }
        page
    }

    /// The viewport at a FRACTIONAL scroll offset: a real compositor
    /// resamples a sub-pixel scroll, so the frame is a bilinear blend of
    /// two page rows.
    fn sample(page: &[u8], top: f32, vh: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((TW * vh * 4) as usize);
        for j in 0..vh {
            let y = top + j as f32;
            let y0 = y.floor().max(0.0) as u32;
            let f = y - y0 as f32;
            let a = y0.min(TH - 1);
            let b = (y0 + 1).min(TH - 1);
            for x in 0..TW {
                let va = f32::from(page[(a * TW + x) as usize]);
                let vb = f32::from(page[(b * TW + x) as usize]);
                let v = (va + (vb - va) * f).round() as u8;
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        out
    }

    /// A sparse-text page: a paragraph every 44 rows, the rest blank. This
    /// is what most documentation/markdown pages look like inside a tall
    /// selection, and it is the hard case for a translation matcher: most
    /// rows carry no signal, so many offsets cost ~nothing.
    fn sparse_page() -> Vec<u8> {
        let mut page = vec![250u8; (TW * TH) as usize];
        let mut y = 20u32;
        while y + 15 < TH {
            for gy in y..y + 15 {
                for x in 24..TW - 24 {
                    let h = ink_hash(x, gy);
                    if h % 100 < 40 {
                        page[(gy * TW + x) as usize] = 40 + (h % 50) as u8;
                    }
                }
            }
            y += 44;
        }
        page
    }

    /// Drive a manual session over `page` — one frame per poll — and check
    /// the invariant that makes a long capture worth anything:
    ///
    /// **the sheet is always an exact PREFIX of the page.**
    ///
    /// Not "looks about right": byte-for-byte. Rows that were folded in
    /// early can never be rewritten by a later frame that was placed at a
    /// guessed position (that is what used to shift a whole viewport of
    /// correct content), and the sheet can never grow by more than the
    /// page was actually scrolled. Returns the number of refused frames so
    /// a caller can assert which path it exercised.
    fn scroll_and_check_prefix(page: &[u8], steps: &[f32], rounds: usize) -> (ScrollSheet, usize) {
        const VH: u32 = 700;
        let mut s = ScrollSheet::new(TW, VH, &sample(page, 0., VH), SheetLimits::default())
            .expect("sheet from a full-width frame");
        let mut top = 0f32;
        let mut refused = 0;
        for _ in 0..rounds {
            for &step in steps {
                top += step;
                if matches!(s.push(&sample(page, top, VH)), Fold::Refused(_)) {
                    refused += 1;
                }
            }
        }
        let rows = s.dimensions().1;
        assert!(
            rows <= top as u32 + VH,
            "the sheet claims more rows than were scrolled: {rows} > {}+{VH}",
            top as u32
        );
        let want = sample(page, 0., rows);
        assert!(
            s.canvas() == want.as_slice(),
            "the sheet stopped being an exact prefix of the page \
             ({rows} rows after scrolled-to {top})"
        );
        (s, refused)
    }

    /// The manual-scroll reality check: a real page (per-pixel ink, not
    /// row-constant test stripes), a fractional eased-out flick, one frame
    /// per poll. Every changed frame must fold, and the result must be the
    /// page itself.
    #[test]
    fn a_smooth_flick_of_a_real_page_folds_every_frame() {
        let page = text_page();
        let steps = [96f32, 92., 84., 66., 44., 24., 10., 4.];
        let (s, refused) = scroll_and_check_prefix(&page, &steps, 3);
        let (pushed, accepted) = s.stats();
        assert_eq!(refused, 0, "no frame of a smooth flick may be refused");
        assert_eq!(accepted, pushed, "every changed frame must fold in");
        // 3 rounds of 420 rows over a 700-row viewport.
        assert_eq!(s.dimensions(), (TW, 1_260 + 700));
    }

    /// The same gesture over sparse content (a paragraph every 44 rows,
    /// blank in between): the matcher's cost surface is nearly flat there,
    /// which is exactly where a guessed position used to win. Some frames
    /// may be refused as ambiguous; the sheet must still be exact.
    #[test]
    fn a_sparse_page_stays_exact_whatever_the_matcher_refuses() {
        let page = sparse_page();
        let steps = [96f32, 92., 84., 66., 44., 24., 10., 4.];
        let (s, _) = scroll_and_check_prefix(&page, &steps, 3);
        let (pushed, accepted) = s.stats();
        assert!(accepted >= pushed / 2, "{accepted}/{pushed} folded in");
    }

    /// A fast trackpad flick outruns the overlap budget: steps beyond
    /// 0.8 × 700 rows. Those frames are refused — and the refusal must
    /// cost nothing but the lost band. The old code "recovered" by
    /// placing them against the frame it had just seen, which shifted
    /// every accepted row after the first refusal.
    #[test]
    fn a_fast_flick_beyond_the_window_refuses_instead_of_shifting() {
        let page = text_page();
        let steps = [300f32, 380., 440., 520., 640.];
        let (s, refused) = scroll_and_check_prefix(&page, &steps, 2);
        assert!(refused > 0, "640 rows cannot be bridged by a 700-row frame");
        let (pushed, accepted) = s.stats();
        assert!(accepted > 0, "the slow part of the flick must still fold");
        assert!(
            accepted < pushed,
            "{accepted}/{pushed} — did the jump fold?"
        );
    }

    /// A short region (the matcher's window is only ±48 rows here): a
    /// jump beyond it writes NOT ONE BYTE, and a frame that still overlaps
    /// the untouched base then lands with its FULL displacement — the
    /// self-healing half of "refusals are a state, not an error".
    #[test]
    fn a_jump_beyond_the_window_refuses_and_keeps_the_canvas_byte_identical() {
        let mut s = sheet();
        let seed = s.canvas().to_vec();
        for top in [70u32, 100, 130] {
            let out = s.push(&frame_at(top));
            assert!(matches!(out, Fold::Refused(_)), "jump to {top}: {out:?}");
            assert_eq!(s.canvas(), seed.as_slice(), "a refusal must not write");
            assert_eq!(s.dimensions(), (W, H));
        }
        let out = s.push(&frame_at(40));
        assert!(
            matches!(out, Fold::GrewBottom { dy: 40, growth: 40 }),
            "{out:?}"
        );
        assert_eq!(s.dimensions(), (W, H + 40));
        assert_eq!(s.canvas(), expected_canvas(0, 40).as_slice());
    }

    /// The stall is not the end of the session: while nothing links to
    /// the base the sheet simply stops growing, and a single nudge back
    /// inside the window resumes it with the page rows that were skipped
    /// still on the right coordinates — no gap, no duplicate band.
    #[test]
    fn a_stalled_session_resumes_exactly_after_one_nudge_back() {
        let mut s = sheet();
        let seed = s.canvas().to_vec();
        for top in [70u32, 140, 210] {
            assert!(matches!(s.push(&frame_at(top)), Fold::Refused(_)));
            assert_eq!(s.canvas(), seed.as_slice(), "a stall must not write");
        }
        // One nudge back inside the window, then keep going down.
        let mut extent = 0u32;
        for top in [40u32, 80, 120] {
            let out = s.push(&frame_at(top));
            assert!(
                matches!(out, Fold::GrewBottom { dy, .. } if dy == top - extent),
                "{out:?}"
            );
            extent = top;
        }
        assert_eq!(s.dimensions(), (W, H + 120));
        assert_eq!(s.canvas(), expected_canvas(0, 120).as_slice());
    }

    /// A flat TOP region of moving content must NOT be pinned: the
    /// band-level structure guard is what separates "flat because
    /// chrome" from "flat because blank". Runs long enough for the
    /// detection ladder to fire — a blank prefix is "still" in every
    /// frame, which is exactly what a pinned navbar looks like, and only
    /// the in-band structure test tells them apart.
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
        let mut extent = 0u32;
        for top in [40u32, 80, 120, 160, 200] {
            let out = s.push(&frame(top));
            assert!(
                matches!(out, Fold::GrewBottom { dy, .. } if dy == top - extent),
                "{out:?}"
            );
            extent = top;
            // Still all-flat rows accumulated a run, but no structure →
            // no commit: the span stays whole-viewport.
            assert_eq!(s.viewport_span(), (top, H2), "band pinned at {top}");
        }
    }
}
