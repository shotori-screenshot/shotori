//! # In-memory pixels → gpui display: the one place that owns the contract
//!
//! **RenderImage expects BGRA bytes** (gpui's img.rs decode path performs the
//! same RGBA→BGRA swap; we skip decoding and feed memory directly, so we must
//! swap R/B ourselves). Hard-learned lesson: "capture looks right, display
//! looks wrong" — two independent swaps scattered around is a trap, so the
//! conversion is centralized here.
//!
//! This is a leaf module on purpose: `model/` and `annotation/` render caches
//! need the same swap, and both must not depend on `ui/` (dependency
//! direction is `ui → model → platform`). `ui::image_util` re-exports the
//! constructor so existing ui call sites keep their path.

use std::sync::Arc;

use gpui_kit::*;
use image::{Frame, ImageBuffer};
use smallvec::SmallVec;

/// RGBA8 in-memory pixels → a RenderImage ready for `img()` (swaps R/B inside)
pub fn rgba_to_render_image(rgba: Vec<u8>, w: u32, h: u32) -> Arc<RenderImage> {
    let mut bgra = rgba;
    // Safety boundary for the u32 write loop below: a short buffer would be
    // an out-of-bounds *write* (UB, not a panic), so repair the length before
    // any unsafe — pad with transparent black or truncate. Callers derive w/h
    // from the same buffer so this never fires in practice; debug_assert
    // still flags the contract breach under test.
    let needed = (w as usize) * (h as usize) * 4;
    debug_assert_eq!(bgra.len(), needed, "pixel buffer size mismatch");
    bgra.resize(needed, 0);
    // Whole-pixel u32 swap (little-endian word R|G<<8|B<<16|A<<24 →
    // B|G<<8|R<<16|A<<24): two shifts + two ANDs/ORS per pixel, fully
    // register-resident. The API does not promise 4-byte alignment —
    // fall back to byte swaps for the rare unaligned buffer.
    // (usize math: a u32 w*h could overflow mid-range dims in debug builds.)
    let n = (w as usize) * (h as usize);
    let ptr = bgra.as_mut_ptr() as usize;
    if n > 0 && ptr.is_multiple_of(4) {
        let words = unsafe { std::slice::from_raw_parts_mut(bgra.as_mut_ptr().cast::<u32>(), n) };
        for word in words {
            let w0 = *word;
            *word = (w0 & 0xFF00_FF00) | ((w0 & 0xFF) << 16) | ((w0 >> 16) & 0xFF);
        }
    } else {
        let (chunks, _) = bgra.as_chunks_mut::<4>();
        for px in chunks {
            px.swap(0, 2);
        }
    }
    // After the resize, from_raw only fails if w*h overflows usize —
    // impossible on 64-bit for u32 dims. Degrade to a blank 1×1 rather than
    // panicking the render path if a narrower target ever lands here.
    let buf = ImageBuffer::from_raw(w, h, bgra)
        .unwrap_or_else(|| ImageBuffer::from_pixel(1, 1, image::Rgba([0, 0, 0, 255])));
    Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(buf), 1)))
}
