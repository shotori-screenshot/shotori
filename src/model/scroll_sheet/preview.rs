//! The live preview panel's geometry: the window of sheet rows the
//! panel shows, the box-filter downscale of a sheet slice, and the
//! [`Phase`] ladder the capture loop's HUD chip reads. Pure functions
//! over sheet state — the engine and the panel share them so there is
//! one geometry source.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Axis, Refusal, transpose_rgba};

/// Default logical width the preview panel paints a slice at on a
/// full-width (`264px`) deck. Compact side-strip decks publish their own
/// logical cross-axis width via [`PreviewGeometry::set_box`].
pub const PREVIEW_DISPLAY_W: f32 = 254.;

/// Default physical sample width (~2× [`PREVIEW_DISPLAY_W`]) when no
/// display scale has been published yet. Production sessions publish
/// [`PreviewGeometry::sample_width_for`] from the panel's output scale
/// and cross-axis width so both sharpness and scroll-rate mapping stay
/// scale-invariant.
pub const PREVIEW_SAMPLE_W: u32 = 508;

/// Shared `(well_along, well_cross, sample_cross)` channel from the
/// preview panel to the capture engine. Packed into a single `AtomicU64`
/// (`sample_cross: u32` in high 32 bits, `well_cross: u16` in bits
/// 16..32, `well_along: u16` in bits 0..16) so a panel update publishes
/// logical well dimensions and physical sample width atomically with zero
/// lock or torn-read hazard; `0` marks a chrome-free session where no
/// preview is rendered.
#[derive(Clone, Debug, Default)]
pub struct PreviewGeometry(Arc<AtomicU64>);

impl PreviewGeometry {
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }

    /// Physical sample width matching `display_w` logical px on an output
    /// with physical-to-logical `scale`.
    pub fn sample_width_for(display_w: f32, scale: f32) -> u32 {
        (display_w.max(1.0) * scale.clamp(0.5, 8.0))
            .round()
            .max(1.0) as u32
    }

    /// Physical sample width matching [`PREVIEW_DISPLAY_W`] on an output
    /// with physical-to-logical `scale`.
    pub fn sample_width_for_scale(scale: f32) -> u32 {
        Self::sample_width_for(PREVIEW_DISPLAY_W, scale)
    }

    /// Publish `(well_along_logical, well_cross_logical, sample_cross_physical)`.
    pub fn set_box(&self, well_along: u32, well_cross: u32, sample_cross: u32) {
        let along = well_along.min(u32::from(u16::MAX));
        let cross = well_cross.min(u32::from(u16::MAX));
        let packed = if along == 0 || cross == 0 || sample_cross == 0 {
            0
        } else {
            (u64::from(sample_cross) << 32) | (u64::from(cross) << 16) | u64::from(along)
        };
        self.0.store(packed, Ordering::Relaxed);
    }

    pub fn set(&self, well_rows: u32, sample_width: u32) {
        self.set_box(well_rows, PREVIEW_DISPLAY_W.round() as u32, sample_width);
    }

    /// Returns `(well_along_logical, well_cross_logical, sample_cross_physical)`
    /// when a preview panel has published its metrics, or `None` for
    /// chrome-free / uninitialized sessions.
    pub fn get_box(&self) -> Option<(u32, u32, u32)> {
        let packed = self.0.load(Ordering::Relaxed);
        let along = (packed & 0xFFFF) as u32;
        let cross = ((packed >> 16) & 0xFFFF) as u32;
        let sample_w = (packed >> 32) as u32;
        (along > 0 && cross > 0 && sample_w > 0).then_some((along, cross, sample_w))
    }

    /// Returns `(well_rows_logical, sample_width_physical)` when a
    /// preview panel has published its metrics, or `None` for
    /// chrome-free / uninitialized sessions.
    pub fn get(&self) -> Option<(u32, u32)> {
        self.get_box()
            .map(|(along, _cross, sample_w)| (along, sample_w))
    }

    /// Sheet rows that fill `well_along` logical px when `sheet_w` physical
    /// px span `well_cross` logical px — exact rational aspect mapping so
    /// the logical window height is invariant across 1×..3× HiDPI scales
    /// and adaptive panel widths.
    pub fn window_rows_for(sheet_w: u32, well_along: u32, well_cross: u32) -> u32 {
        if sheet_w == 0 || well_along == 0 {
            return 16;
        }
        let cross = u64::from(well_cross.max(1));
        let numer = u64::from(well_along) * u64::from(sheet_w) + cross / 2;
        (numer / cross).clamp(16, u64::from(u32::MAX)) as u32
    }

    /// Area-box downscaled slice oriented in screen coordinates: vertical
    /// sessions keep `(pw, ph)` directly; horizontal sessions transpose
    /// the downscaled `(pw, ph)` buffer back to `(ph, pw)` so the live
    /// preview renders right-side up instead of sideways/mirrored.
    pub fn slice_oriented(
        rgba: &[u8],
        width: u32,
        height: u32,
        top: u32,
        rows: u32,
        target_w: u32,
        axis: Axis,
    ) -> (u32, u32, Vec<u8>) {
        let (pw, ph, out) = preview_slice(rgba, width, height, top, rows, target_w);
        match axis {
            Axis::Vertical => (pw, ph, out),
            Axis::Horizontal => (ph, pw, transpose_rgba(&out, pw, ph)),
        }
    }
}

/// Nominal step ratio and exact preview cross-axis width (preview px) for
/// a sheet width and target physical sample width. `pw` clamps directly
/// to `target_w.min(width)` so narrow-over-threshold widths (e.g. 255 px
/// over a 254 px well) never drop 2× in resolution before GPU upscaling.
pub fn preview_scale_for(width: u32, target_w: u32) -> (u32, u32) {
    let pw = width.min(target_w.max(1)).max(1);
    let k = width.div_ceil(pw).max(1);
    (k, pw)
}

/// [`preview_scale_for`] at the default [`PREVIEW_SAMPLE_W`].
pub fn preview_scale(width: u32) -> (u32, u32) {
    preview_scale_for(width, PREVIEW_SAMPLE_W)
}

/// Preview rows that exactly fill a panel well `well_h` logical px tall
/// at [`PREVIEW_DISPLAY_W`] wide — the strip is painted 1:1, never
/// scaled to the well.
pub fn preview_fit_rows(well_h: f32, pw: u32) -> u32 {
    ((well_h * pw.max(1) as f32 / PREVIEW_DISPLAY_W).round() as u32).max(16)
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
    // Degenerate empty sheet: rows = ...max(1) below would exceed
    // sheet_h and `sheet_h - rows` would underflow — bail first.
    if sheet_h == 0 {
        return (0, 1);
    }
    // The window may never be shorter than the viewport: the highlight
    // must fit inside the strip, or the "what you are looking at" band
    // would be clipped by a panel-height accident.
    let rows = window_rows.max(viewport_h.min(sheet_h)).min(sheet_h).max(1);
    let bottom = viewport_top.saturating_add(viewport_h).min(sheet_h);
    let top = bottom.saturating_sub(rows).min(sheet_h - rows);
    (top, rows)
}

/// Exact fixed-point area-box downscale of a horizontal slice
/// `[top, top + rows)` of the sheet to `pw = width.min(target_w)`.
///
/// Unlike integer `k = ceil(width / target_w)` decimation — which halved
/// a 255 px capture to 128 px before a 2× GPU bilinear stretch — every
/// preview pixel integrates its exact fractional source footprint in
/// `1/256`-pixel coordinates so text stays crisp at arbitrary selection
/// widths and never triggers GPU upsampling blur.
pub fn preview_slice(
    rgba: &[u8],
    width: u32,
    height: u32,
    top: u32,
    rows: u32,
    target_w: u32,
) -> (u32, u32, Vec<u8>) {
    let needed = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || rgba.len() < needed {
        return (1, 1, vec![0u8; 4]);
    }
    let (_, pw) = preview_scale_for(width, target_w);
    let y0 = top.min(height.saturating_sub(1));
    let rows = rows.min(height.saturating_sub(y0)).max(1);
    let ph = ((u64::from(rows) * u64::from(pw)).div_ceil(u64::from(width))).max(1) as u32;

    if pw == width && ph == rows {
        let row_bytes = (width as usize) * 4;
        let start = (y0 as usize) * row_bytes;
        let end = start + (rows as usize) * row_bytes;
        return (pw, ph, rgba[start..end].to_vec());
    }

    let x_spans: Vec<(u32, u32, u64, u64)> = (0..pw)
        .map(|px_| {
            let sx0 = (u64::from(px_) * u64::from(width) * 256) / u64::from(pw);
            let sx1 = ((u64::from(px_ + 1) * u64::from(width) * 256) / u64::from(pw)).max(sx0 + 1);
            let x_first = (sx0 / 256).min(u64::from(width - 1)) as u32;
            let x_last = ((sx1 - 1) / 256).min(u64::from(width - 1)) as u32;
            (x_first, x_last, sx0, sx1)
        })
        .collect();

    let mut out = vec![0u8; (pw as usize) * (ph as usize) * 4];
    let stride = (width as usize) * 4;

    for py in 0..ph {
        let sy0 = (u64::from(py) * u64::from(rows) * 256) / u64::from(ph);
        let sy1 = ((u64::from(py + 1) * u64::from(rows) * 256) / u64::from(ph)).max(sy0 + 1);
        let y_first = (sy0 / 256).min(u64::from(rows - 1)) as u32;
        let y_last = ((sy1 - 1) / 256).min(u64::from(rows - 1)) as u32;

        for (px_, &(x_first, x_last, sx0, sx1)) in x_spans.iter().enumerate() {
            let mut acc = [0u64; 4];
            let mut w_sum = 0u64;

            for ry in y_first..=y_last {
                let ry_top = u64::from(ry) * 256;
                let ry_bot = ry_top + 256;
                let wy = sy1.min(ry_bot).saturating_sub(sy0.max(ry_top));
                if wy == 0 {
                    continue;
                }
                let row_base = ((y0 + ry) as usize) * stride;
                for rx in x_first..=x_last {
                    let rx_left = u64::from(rx) * 256;
                    let rx_right = rx_left + 256;
                    let wx = sx1.min(rx_right).saturating_sub(sx0.max(rx_left));
                    let w = wx * wy;
                    if w == 0 {
                        continue;
                    }
                    let i = row_base + (rx as usize) * 4;
                    acc[0] += u64::from(rgba[i]) * w;
                    acc[1] += u64::from(rgba[i + 1]) * w;
                    acc[2] += u64::from(rgba[i + 2]) * w;
                    acc[3] += u64::from(rgba[i + 3]) * w;
                    w_sum += w;
                }
            }

            if w_sum > 0 {
                let o = ((py as usize) * (pw as usize) + px_) * 4;
                let half = w_sum / 2;
                out[o] = ((acc[0] + half) / w_sum) as u8;
                out[o + 1] = ((acc[1] + half) / w_sum) as u8;
                out[o + 2] = ((acc[2] + half) / w_sum) as u8;
                out[o + 3] = ((acc[3] + half) / w_sum) as u8;
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
    /// Lateral layout changes cannot be safely combined with the document.
    LayoutShift,
    /// No overlap for long enough that the region cannot advance on its
    /// own (fixed chrome taller than the matcher's trim, a video pane, a
    /// region that does not scroll). Still not fatal: the user can
    /// nudge back into the overlap window, move the frame, or finish
    /// with what is already there.
    Stalled,
    /// The sheet reached [`super::SheetLimits::max_height`]: further
    /// growth is refused while keeping the session open in the panel so
    /// the truncation is visible and the user can choose Copy or Save.
    MaxLength,
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
    if reason == Refusal::TooTall {
        Phase::MaxLength
    } else if refusals >= NO_OVERLAP_FRAMES && reason == Refusal::LayoutShift {
        Phase::LayoutShift
    } else if refusals >= STALLED_FRAMES {
        Phase::Stalled
    } else if refusals >= NO_OVERLAP_FRAMES {
        match reason {
            Refusal::NoOverlap => Phase::NoOverlap,
            // `TooSmall` ends the session before it can reach the chip;
            // anything else is "the pixels moved in a way a translation
            // cannot explain".
            _ => Phase::Unstable,
        }
    } else {
        Phase::Following
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_slice_avoids_integer_decimation_cliffs() {
        let target_w = 254;
        for width in [254u32, 255, 509, 770, 1280] {
            let height = 200;
            let rgba = vec![180u8; (width * height * 4) as usize];
            let (pw, ph, out) = preview_slice(&rgba, width, height, 0, height, target_w);
            assert_eq!(
                pw, target_w,
                "width={width} must downscale directly to {target_w} without 2x cliff"
            );
            let expected_ph =
                ((u64::from(height) * u64::from(pw)).div_ceil(u64::from(width))) as u32;
            assert_eq!(ph, expected_ph);
            assert_eq!(out.len(), (pw * ph * 4) as usize);
            assert!(out.iter().all(|&b| (179..=181).contains(&b)));
        }
    }

    #[test]
    fn horizontal_preview_slice_transposes_back_to_screen_orientation() {
        // Screen frame 120 wide × 60 tall: top screen half (sy < 30) is red,
        // bottom screen half (sy >= 30) is green; left half has alpha 200,
        // right half has alpha 255.
        let (screen_w, screen_h) = (120u32, 60u32);
        let mut screen = vec![0u8; (screen_w * screen_h * 4) as usize];
        for sy in 0..screen_h {
            for sx in 0..screen_w {
                let i = ((sy * screen_w + sx) * 4) as usize;
                let top_half = sy < screen_h / 2;
                let left_half = sx < screen_w / 2;
                screen[i] = if top_half { 240 } else { 20 };
                screen[i + 1] = if top_half { 20 } else { 220 };
                screen[i + 2] = 40;
                screen[i + 3] = if left_half { 200 } else { 255 };
            }
        }
        // Internal sheet stores horizontal captures transposed: (sheet_w=60, sheet_h=120).
        let internal = transpose_rgba(&screen, screen_w, screen_h);
        let (out_w, out_h, oriented) = PreviewGeometry::slice_oriented(
            &internal,
            screen_h,
            screen_w,
            0,
            screen_w,
            30,
            Axis::Horizontal,
        );
        // Downscaled by 2x -> 60 wide × 30 tall in screen coordinates.
        assert_eq!((out_w, out_h), (60, 30));
        let top_left = &oriented[0..4];
        let bot_right = &oriented[((out_h - 1) * out_w + (out_w - 1)) as usize * 4..][..4];
        assert!(top_left[0] > 200 && top_left[1] < 40 && top_left[3] == 200);
        assert!(bot_right[0] < 40 && bot_right[1] > 200 && bot_right[3] == 255);
    }

    #[test]
    fn preview_geometry_packs_adaptive_box_metrics() {
        let geom = PreviewGeometry::new();
        assert_eq!(geom.get_box(), None);
        geom.set_box(480, 180, PreviewGeometry::sample_width_for(180.0, 1.5));
        assert_eq!(geom.get_box(), Some((480, 180, 270)));
        assert_eq!(geom.get(), Some((480, 270)));
        geom.set_box(0, 180, 270);
        assert_eq!(geom.get_box(), None);
    }
}
