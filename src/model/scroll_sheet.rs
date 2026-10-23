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

//! The impl is split by concern: [`profile`] (frame profiles and the band
//! mask), [`matcher`] (offset search and refusal taxonomy), [`sticky`] (edge
//! chrome), [`floating`] (local overlays and overlap composition), its
//! `frame_edge` submodule (captured window decorations), [`sidebar`]
//! (independent column composition), and [`preview`] (live panel geometry and
//! the HUD phase ladder). The `tests` module and font-rasterized `scene_tests`
//! exercise the complete pipeline through [`ScrollSheet::push`].

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
mod tests;
