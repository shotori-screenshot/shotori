//! Pure floating-pin geometry, magnetic alignment, thumbnail collapse math,
//! and lossless RGBA8 pixel transforms.
//!
//! Kept free of Wayland surfaces and GPUI element trees so every rotation,
//! flip, grayscale/invert pass, edge/sibling snap rule, and collapse/expand
//! round-trip is unit-tested deterministically.

use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

/// Magnetic snap radius (logical px) when aligning against output edges.
pub const EDGE_SNAP_PX: f32 = 10.;
/// Magnetic snap radius (logical px) when aligning against sibling pins.
/// Slightly tighter than output-edge snap so densely packed staggered pins
/// remain easy to nudge freely without fighting nearby neighbors.
pub const PIN_SNAP_PX: f32 = 8.;
/// Square logical side of a collapsed thumbnail pin.
pub const COLLAPSED_SIDE: f32 = 56.;
/// Minimum grabbable slice (logical px) kept on at least one output.
pub const MIN_VISIBLE: f32 = 24.;

/// Clamp that never panics when `lo > hi` (degenerate tiny output bounds).
pub fn clamp_to(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > hi { lo } else { v.clamp(lo, hi) }
}

/// Keep a pin grabbable on `desk`: at least [`MIN_VISIBLE`] stays inside on each axis.
pub fn clamp_to_output(
    origin: Point<Pixels>,
    pin: Size<Pixels>,
    desk: Bounds<Pixels>,
) -> Point<Pixels> {
    let (w, h) = (f32::from(pin.width), f32::from(pin.height));
    let (vw, vh) = (
        w.min(MIN_VISIBLE).min(f32::from(desk.size.width)),
        h.min(MIN_VISIBLE).min(f32::from(desk.size.height)),
    );
    let (dl, dt) = (f32::from(desk.left()), f32::from(desk.top()));
    let (dr, db) = (f32::from(desk.right()), f32::from(desk.bottom()));
    point(
        px(clamp_to(f32::from(origin.x), dl - (w - vw), dr - vw)),
        px(clamp_to(f32::from(origin.y), dt - (h - vh), db - vh)),
    )
}

/// Choose the nearest position with a grabbable slice on an actual output,
/// rather than treating dead space in a multi-monitor bounding box as visible.
pub fn clamp_origin(
    origin: Point<Pixels>,
    pin: Size<Pixels>,
    outputs: &[Bounds<Pixels>],
) -> Point<Pixels> {
    let distance = |p: &Point<Pixels>| f32::from(p.x - origin.x).hypot(f32::from(p.y - origin.y));
    outputs
        .iter()
        .filter(|output| output.size.width > px(0.) && output.size.height > px(0.))
        .map(|output| clamp_to_output(origin, pin, *output))
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or(origin)
}

/// Magnetically align `origin` to output edges (within [`EDGE_SNAP_PX`]) and/or
/// sibling pin edges (within [`PIN_SNAP_PX`]: flush adjacency and shared edges).
pub fn snap_pin_origin(
    origin: Point<Pixels>,
    pin: Size<Pixels>,
    outputs: &[Bounds<Pixels>],
    siblings: &[Bounds<Pixels>],
    snap_edges: bool,
    snap_pins: bool,
) -> Point<Pixels> {
    let mut x = f32::from(origin.x);
    let mut y = f32::from(origin.y);
    let (w, h) = (f32::from(pin.width), f32::from(pin.height));
    let mut best_dx = f32::INFINITY;
    let mut best_dy = f32::INFINITY;
    let mut snapped_x = None;
    let mut snapped_y = None;

    if snap_edges {
        for out in outputs {
            if out.size.width <= px(0.) || out.size.height <= px(0.) {
                continue;
            }
            let (ol, ot, or, ob) = (
                f32::from(out.left()),
                f32::from(out.top()),
                f32::from(out.right()),
                f32::from(out.bottom()),
            );
            let v_overlap = y + h > ot - EDGE_SNAP_PX && y < ob + EDGE_SNAP_PX;
            let h_overlap = x + w > ol - EDGE_SNAP_PX && x < or + EDGE_SNAP_PX;
            if v_overlap {
                for target_x in [ol, or - w] {
                    let d = (x - target_x).abs();
                    if d <= EDGE_SNAP_PX && d < best_dx {
                        best_dx = d;
                        snapped_x = Some(target_x);
                    }
                }
            }
            if h_overlap {
                for target_y in [ot, ob - h] {
                    let d = (y - target_y).abs();
                    if d <= EDGE_SNAP_PX && d < best_dy {
                        best_dy = d;
                        snapped_y = Some(target_y);
                    }
                }
            }
        }
    }

    if snap_pins {
        for sib in siblings {
            if sib.size.width <= px(0.) || sib.size.height <= px(0.) {
                continue;
            }
            let (sl, st, sr, sb) = (
                f32::from(sib.left()),
                f32::from(sib.top()),
                f32::from(sib.right()),
                f32::from(sib.bottom()),
            );
            // Snap X when vertically near or overlapping the sibling band.
            let near_v = y + h >= st - 48. && y <= sb + 48.;
            if near_v {
                // Flush left-to-right, flush right-to-left, aligned lefts, aligned rights.
                for target_x in [sr, sl - w, sl, sr - w] {
                    let d = (x - target_x).abs();
                    if d <= PIN_SNAP_PX && d < best_dx {
                        best_dx = d;
                        snapped_x = Some(target_x);
                    }
                }
            }
            // Snap Y when horizontally near or overlapping the sibling band.
            let near_h = x + w >= sl - 48. && x <= sr + 48.;
            if near_h {
                // Flush top-to-bottom, flush bottom-to-top, aligned tops, aligned bottoms.
                for target_y in [sb, st - h, st, sb - h] {
                    let d = (y - target_y).abs();
                    if d <= PIN_SNAP_PX && d < best_dy {
                        best_dy = d;
                        snapped_y = Some(target_y);
                    }
                }
            }
        }
    }

    if let Some(sx) = snapped_x {
        x = sx;
    }
    if let Some(sy) = snapped_y {
        y = sy;
    }
    point(px(x), px(y))
}

/// Saved full-size geometry while a pin is collapsed into a compact thumbnail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollapsedPin {
    /// Expanded size at the moment the pin was collapsed (preserves active zoom).
    pub expanded_size: Size<Pixels>,
}

/// Compute the collapsed `56×56` thumbnail rect anchored at `current.origin`.
pub fn collapse_bounds(
    current: Bounds<Pixels>,
    outputs: &[Bounds<Pixels>],
) -> (Bounds<Pixels>, CollapsedPin) {
    let thumb = size(px(COLLAPSED_SIDE), px(COLLAPSED_SIDE));
    let origin = clamp_origin(current.origin, thumb, outputs);
    (
        Bounds::new(origin, thumb),
        CollapsedPin {
            expanded_size: current.size,
        },
    )
}

/// Restore the expanded rect from the thumbnail's current origin (so dragging a
/// collapsed badge to another spot expands it at that new spot, clamped cleanly).
pub fn expand_bounds(
    thumb_rect: Bounds<Pixels>,
    collapsed: CollapsedPin,
    outputs: &[Bounds<Pixels>],
) -> Bounds<Pixels> {
    let sz = collapsed.expanded_size;
    let origin = clamp_origin(thumb_rect.origin, sz, outputs);
    Bounds::new(origin, sz)
}

/// Lossless 90° clockwise rotation of an RGBA8 buffer: `(w, h) -> (h, w)`.
pub fn rotate_rgba_cw(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = (h, w);
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < expected {
        return (w, h, rgba.to_vec());
    }
    let mut out = vec![0u8; expected];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let nx = (h as usize - 1) - y;
            let ny = x;
            let src = (y * w as usize + x) * 4;
            let dst = (ny * nw as usize + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    (nw, nh, out)
}

/// Lossless 90° counter-clockwise rotation of an RGBA8 buffer: `(w, h) -> (h, w)`.
pub fn rotate_rgba_ccw(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = (h, w);
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < expected {
        return (w, h, rgba.to_vec());
    }
    let mut out = vec![0u8; expected];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let nx = y;
            let ny = (w as usize - 1) - x;
            let src = (y * w as usize + x) * 4;
            let dst = (ny * nw as usize + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    (nw, nh, out)
}

/// Lossless horizontal mirror of an RGBA8 buffer in place.
pub fn flip_rgba_horizontal(w: u32, h: u32, rgba: &mut [u8]) {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w <= 1 || h == 0 || rgba.len() < expected {
        return;
    }
    let row_bytes = w as usize * 4;
    for y in 0..h as usize {
        let row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..(w as usize / 2) {
            let l = x * 4;
            let r = (w as usize - 1 - x) * 4;
            for c in 0..4 {
                row.swap(l + c, r + c);
            }
        }
    }
}

/// Lossless vertical mirror of an RGBA8 buffer in place.
pub fn flip_rgba_vertical(w: u32, h: u32, rgba: &mut [u8]) {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h <= 1 || rgba.len() < expected {
        return;
    }
    let row_bytes = w as usize * 4;
    for y in 0..(h as usize / 2) {
        let top = y * row_bytes;
        let bot = (h as usize - 1 - y) * row_bytes;
        let (head, tail) = rgba.split_at_mut(bot);
        head[top..top + row_bytes].swap_with_slice(&mut tail[..row_bytes]);
    }
}

/// Apply non-destructive color filters (`grayscale` BT.709 luma, `invert` RGB)
/// to a copy/slice of RGBA8 pixels while preserving exact alpha.
pub fn apply_color_effects(rgba: &mut [u8], grayscale: bool, invert: bool) {
    if !grayscale && !invert {
        return;
    }
    for px in rgba.as_chunks_mut::<4>().0 {
        let (mut r, mut g, mut b) = (px[0], px[1], px[2]);
        if invert {
            r = 255 - r;
            g = 255 - g;
            b = 255 - b;
        }
        if grayscale {
            // Integer BT.709 luma: (54*R + 183*G + 19*B + 128) >> 8
            let y = ((54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b) + 128) >> 8)
                .min(255) as u8;
            r = y;
            g = y;
            b = y;
        }
        px[0] = r;
        px[1] = g;
        px[2] = b;
    }
}

/// Extract a centered square crop from `(w, h, rgba)` so collapsed thumbnails
/// display an aspect-true focal crop rather than squishing wide/tall banners.
pub fn center_square_crop(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < expected || w == h {
        return (w, h, rgba.to_vec());
    }
    let side = w.min(h);
    let x0 = (w - side) / 2;
    let y0 = (h - side) / 2;
    let mut out = Vec::with_capacity((side as usize) * (side as usize) * 4);
    for y in y0..(y0 + side) {
        let start = (y as usize * w as usize + x0 as usize) * 4;
        let end = start + side as usize * 4;
        out.extend_from_slice(&rgba[start..end]);
    }
    (side, side, out)
}

#[cfg(test)]
mod tests {
    use super::{
        COLLAPSED_SIDE, apply_color_effects, center_square_crop, collapse_bounds, expand_bounds,
        flip_rgba_horizontal, flip_rgba_vertical, rotate_rgba_ccw, rotate_rgba_cw, snap_pin_origin,
    };
    use gpui_kit::{Bounds, point, px, size};

    fn sample_2x3() -> (u32, u32, Vec<u8>) {
        // 2 wide × 3 tall, unique RGBA per pixel
        let pixels: Vec<u8> = (0u8..6)
            .flat_map(|i| [i * 10 + 1, i * 10 + 2, i * 10 + 3, 255 - i])
            .collect();
        (2, 3, pixels)
    }

    #[test]
    fn four_cw_or_ccw_rotations_are_lossless_identity() {
        let (orig_w, orig_h, orig) = sample_2x3();
        let (mut w, mut h, mut buf) = (orig_w, orig_h, orig.clone());
        for step in 0..4 {
            (w, h, buf) = rotate_rgba_cw(w, h, &buf);
            if step % 2 == 0 {
                assert_eq!((w, h), (orig_h, orig_w));
            } else {
                assert_eq!((w, h), (orig_w, orig_h));
            }
        }
        assert_eq!(buf, orig);

        // CW followed immediately by CCW is also exact identity.
        let (rw, rh, cw) = rotate_rgba_cw(orig_w, orig_h, &orig);
        let (bw, bh, back) = rotate_rgba_ccw(rw, rh, &cw);
        assert_eq!((bw, bh), (orig_w, orig_h));
        assert_eq!(back, orig);
    }

    #[test]
    fn double_horizontal_and_vertical_flips_are_identity() {
        let (w, h, orig) = sample_2x3();
        let mut buf = orig.clone();
        flip_rgba_horizontal(w, h, &mut buf);
        assert_ne!(buf, orig);
        flip_rgba_horizontal(w, h, &mut buf);
        assert_eq!(buf, orig);

        flip_rgba_vertical(w, h, &mut buf);
        assert_ne!(buf, orig);
        flip_rgba_vertical(w, h, &mut buf);
        assert_eq!(buf, orig);
    }

    #[test]
    fn color_effects_preserve_alpha_and_double_invert_restores_rgb() {
        let (_, _, orig) = sample_2x3();
        let mut inv = orig.clone();
        apply_color_effects(&mut inv, false, true);
        for (a, b) in orig.as_chunks::<4>().0.iter().zip(inv.as_chunks::<4>().0) {
            assert_eq!(b[0], 255 - a[0]);
            assert_eq!(b[1], 255 - a[1]);
            assert_eq!(b[2], 255 - a[2]);
            assert_eq!(b[3], a[3], "alpha must never be inverted");
        }
        apply_color_effects(&mut inv, false, true);
        assert_eq!(inv, orig);

        let mut gray = vec![255, 0, 0, 128, 0, 255, 0, 64];
        apply_color_effects(&mut gray, true, false);
        assert_eq!(gray[0], gray[1]);
        assert_eq!(gray[1], gray[2]);
        assert_eq!(gray[3], 128);
        assert!(gray[4] > gray[0], "BT.709 weights green brighter than red");
        assert_eq!(gray[7], 64);
    }

    #[test]
    fn pin_to_pin_snapping_aligns_flush_and_shared_edges() {
        let out = [Bounds::new(
            point(px(0.), px(0.)),
            size(px(1920.), px(1080.)),
        )];
        let sib = [Bounds::new(
            point(px(200.), px(100.)),
            size(px(150.), px(120.)),
        )];
        let my_size = size(px(100.), px(80.));

        // Dragging right beside `sib` (x=355 is 5px from sib.right=350; y=104 is 4px from sib.top=100)
        let snapped = snap_pin_origin(point(px(355.), px(104.)), my_size, &out, &sib, false, true);
        assert_eq!(snapped, point(px(350.), px(100.)));

        // Far away stays untouched
        let free = snap_pin_origin(point(px(600.), px(500.)), my_size, &out, &sib, true, true);
        assert_eq!(free, point(px(600.), px(500.)));
    }

    #[test]
    fn collapse_and_expand_preserve_moved_origin_and_zoomed_size() {
        let out = [Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)))];
        let initial = Bounds::new(point(px(120.), px(90.)), size(px(320.), px(200.)));
        let (thumb, saved) = collapse_bounds(initial, &out);
        assert_eq!(thumb.size, size(px(COLLAPSED_SIDE), px(COLLAPSED_SIDE)));
        assert_eq!(thumb.origin, point(px(120.), px(90.)));

        // Dragging the collapsed thumbnail near the bottom-right corner and expanding
        // clamps cleanly so the restored 320x200 pin remains grabbable.
        let moved_thumb = Bounds::new(point(px(300.), px(240.)), thumb.size);
        let restored = expand_bounds(moved_thumb, saved, &out);
        assert_eq!(restored.origin, point(px(300.), px(240.)));
        assert_eq!(restored.size, size(px(320.), px(200.)));
    }

    #[test]
    fn center_square_crop_extracts_middle_and_handles_degenerate_sizes() {
        let (w, h, rgba) = (4, 2, vec![128u8; 4 * 2 * 4]);
        let (cw, ch, cropped) = center_square_crop(w, h, &rgba);
        assert_eq!((cw, ch), (2, 2));
        assert_eq!(cropped.len(), 2 * 2 * 4);

        let (zw, zh, empty) = center_square_crop(0, 5, &[]);
        assert_eq!((zw, zh), (0, 5));
        assert!(empty.is_empty());
    }
}
