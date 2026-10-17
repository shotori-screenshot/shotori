//! The live preview panel's geometry: the window of sheet rows the
//! panel shows, the box-filter downscale of a sheet slice, and the
//! [`Phase`] ladder the capture loop's HUD chip reads. Pure functions
//! over sheet state — the engine and the panel share them so there is
//! one geometry source.

use super::Refusal;
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
    /// Frames changed but none could be placed because the motion
    /// outran the overlap budget: the user scrolled faster than one
    /// capture interval could bridge. Transient by nature, and the cure
    /// is in the user's hands — the base never moved, so scrolling
    /// slower re-links as soon as a frame still overlaps it.
    NoOverlap,
    /// The area refuses to sit still — an animation, a video, a canvas
    /// that repaints, or a tear that never settles. Scrolling slower
    /// does not help; this is the honest reading of a region whose
    /// content changes on its own.
    Unstable,
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
pub(super) const STALLED_FRAMES: u32 = 40;

/// The refusal ladder: how many frames in a row have changed without
/// being placeable, and WHY the newest one failed. `refusals` counts
/// CHANGED frames since the last frame folded in — duplicates (nothing
/// moved) are not refusals, so a reader pausing mid-capture never looks
/// broken, and the loop clears the streak on a still frame for the same
/// reason.
///
/// The reason picks the word on the chip: "no overlap" is advice about
/// scroll speed, "unstable" is a statement about the area. Reporting
/// every refusal as "no overlap" was the old bug — a region with a
/// blinking video ad told the user to scroll slower forever.
pub(crate) fn phase_of(refusals: u32, reason: Refusal) -> Phase {
    if refusals >= STALLED_FRAMES {
        Phase::Stalled
    } else if refusals >= NO_OVERLAP_FRAMES {
        match reason {
            Refusal::NoOverlap => Phase::NoOverlap,
            // `TooTall` and `TooSmall` end the session before they can
            // reach the chip; anything else is "the pixels moved in a
            // way a translation cannot explain".
            _ => Phase::Unstable,
        }
    } else {
        Phase::Following
    }
}
