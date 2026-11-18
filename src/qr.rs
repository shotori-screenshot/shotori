//! QR code detection and decoding via `rqrr`.
//!
//! Real desktop captures hit several failure modes if we only run a single
//! BT.601 grayscale pass composited over white:
//! 1. Mixed-polarity screenshots (e.g. light + dark theme comparison cards)
//!    contain both dark-on-light and light-on-dark codes; returning early on
//!    the normal pass silently drops every inverted code.
//! 2. White/light modules on a transparent PNG background (`[255,255,255,255]`
//!    over `[0,0,0,0]`) collapse to solid `255` when composited over white.
//! 3. Compact UI widgets on 1x displays (1–2 px per module, or ~2 px blurred
//!    by fractional DPI scaling) fall below `rqrr`'s `1:1:3:1:1` scanline
//!    tolerance (`err = avg * 3 / 4 == 0` when `avg == 1`).
//! 4. Styled brand QR codes with iso-luminant foreground/background pairs
//!    (such as red `[220,30,30]` on cyan/blue `[0,110,210]`) collapse to near-
//!    zero contrast under BT.601 luma weights (`77 R + 150 G + 29 B`).
//!
//! To keep normal full-screen opaque scans fast and bounded in memory, the
//! initial RGBA conversion tracks opacity and chroma in one pass:
//! - Both normal and inverted polarities always run at 1x, with hits sorted
//!   into deterministic top-to-bottom, left-to-right reading order.
//! - Black-background alpha recovery only runs when at least one pixel has
//!   `alpha < 255`.
//! - Integer upscaling (plus a 3x3 sharpen retry for antialiased ~2 px modules)
//!   is strictly gated to crops `<= 640x640` (`<= ~1.65 MP` upscaled), so full-
//!   screen 1080p/4K captures never allocate large upscaled buffers.
//! - Red/Blue single-channel fallback only runs when prior passes found zero
//!   codes AND the image actually contains non-trivial chroma, downsampling
//!   large captures to `<= ~1.64 MP`. Because BT.601 luma satisfies
//!   `77 dR + 150 dG + 29 dB = 0` on iso-luminant pairs, `max(|dR|, |dB|) >=
//!   (150 / 106) |dG|`, so Red and Blue channels mathematically span every
//!   iso-luminant color pair in RGB space.

use std::collections::HashSet;

use anyhow::{Context as _, bail};
use image::{GrayImage, Luma, imageops::FilterType};

/// White quiet-zone border added around cropped selections so finder patterns
/// sitting right against the image edge remain detectable.
const QUIET_ZONE_PX: u32 = 16;

/// Minimum RGB channel spread (`max(r,g,b) - min(r,g,b)`) required before
/// considering chromatic fallback passes.
const CHROMA_THRESHOLD: u32 = 32;

/// Dimension threshold below which upscaling is always attempted (within the
/// input pixel budget) even if a larger code in the same crop decoded at 1x.
const SMALL_CROP_DIM_PX: u32 = 180;

/// Dimension threshold below which 4x upscaling is preferred over 3x so 1 px
/// modules become a clean 4 px wide.
const TINY_CROP_DIM_PX: u32 = 96;

/// Maximum input area (`640 * 640`) eligible for integer upscaling, preventing
/// large non-QR desktop captures from allocating multi-megapixel buffers.
const MAX_UPSCALE_INPUT_PIXELS: usize = 640 * 640;

/// Hard ceiling (`~1.65 MP`) on the upscaled unpadded image area.
const MAX_UPSCALED_PIXELS: usize = 1_650_000;

/// Target pixel ceiling (`1280 * 1280`) for chromatic fallback planes; larger
/// captures are downsampled by an integer stride during channel extraction.
const MAX_CHROMA_FULLRES_PIXELS: usize = 1280 * 1280;

struct RgbaScan {
    luma_over_white: GrayImage,
    has_transparency: bool,
    has_chroma: bool,
}

#[derive(Debug, Clone)]
struct DecodedHit {
    payload: String,
    cx: f64,
    cy: f64,
    span_y: f64,
}

#[derive(Clone, Copy)]
enum ColorChannel {
    Red,
    Blue,
}

/// Scan an RGBA8 buffer for QR codes and return all distinct non-empty payloads
/// in deterministic top-to-bottom, left-to-right reading order.
///
/// Composites over white for the primary luma scan (checking both normal and
/// inverted polarities), recovers light-on-transparent codes when alpha is
/// present, upscales bounded small/dense crops for 1–2 px modules, and falls
/// back to chromatic channels for iso-luminant colored QR codes.
pub fn scan_qr(rgba: &[u8], w: u32, h: u32) -> anyhow::Result<Vec<String>> {
    if w == 0 || h == 0 {
        bail!("empty image ({w}x{h})");
    }
    let expected = (w as usize)
        .checked_mul(h as usize)
        .and_then(|px| px.checked_mul(4))
        .context("image dimensions overflow")?;
    if rgba.len() != expected {
        bail!(
            "RGBA buffer size mismatch: expected {expected} bytes for {w}x{h}, got {}",
            rgba.len()
        );
    }

    let scan = rgba_to_luma_over_white(rgba, w, h);
    let mut hits = Vec::new();

    scan_luma_plane(&scan.luma_over_white, 1.0, &mut hits);

    if scan.has_transparency {
        let luma_over_black = rgba_to_luma_over_black(rgba, w, h);
        scan_luma_plane(&luma_over_black, 1.0, &mut hits);
    }

    if hits.is_empty() && scan.has_chroma {
        let step = chroma_downsample_step(w, h);
        let base_scale = 1.0 / (step as f64);

        let red = extract_channel_over_white(rgba, w, h, step, ColorChannel::Red);
        scan_luma_plane(&red, base_scale, &mut hits);

        if hits.is_empty() {
            let blue = extract_channel_over_white(rgba, w, h, step, ColorChannel::Blue);
            scan_luma_plane(&blue, base_scale, &mut hits);
        }
    }

    Ok(finalize_hits(hits))
}

/// Join multiple decoded QR payloads into a single clipboard string.
pub fn join_payloads(payloads: &[String]) -> String {
    payloads.join("\n")
}

fn rgba_to_luma_over_white(rgba: &[u8], w: u32, h: u32) -> RgbaScan {
    let mut out = GrayImage::new(w, h);
    let mut has_transparency = false;
    let mut has_chroma = false;
    let (chunks, _) = rgba.as_chunks::<4>();

    for (pixel, px) in out.pixels_mut().zip(chunks) {
        let a = px[3] as u32;
        if a < 255 {
            has_transparency = true;
        }
        let inv_a = 255 - a;
        let r = (px[0] as u32 * a + 255 * inv_a) / 255;
        let g = (px[1] as u32 * a + 255 * inv_a) / 255;
        let b = (px[2] as u32 * a + 255 * inv_a) / 255;
        if !has_chroma {
            let max_c = r.max(g).max(b);
            let min_c = r.min(g).min(b);
            if max_c - min_c >= CHROMA_THRESHOLD {
                has_chroma = true;
            }
        }
        let luma = ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8;
        *pixel = Luma([luma]);
    }

    RgbaScan {
        luma_over_white: out,
        has_transparency,
        has_chroma,
    }
}

fn rgba_to_luma_over_black(rgba: &[u8], w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    let (chunks, _) = rgba.as_chunks::<4>();
    for (pixel, px) in out.pixels_mut().zip(chunks) {
        let a = px[3] as u32;
        let r = (px[0] as u32 * a) / 255;
        let g = (px[1] as u32 * a) / 255;
        let b = (px[2] as u32 * a) / 255;
        let luma = ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8;
        *pixel = Luma([luma]);
    }
    out
}

fn chroma_downsample_step(w: u32, h: u32) -> u32 {
    let pixels = (w as usize).saturating_mul(h as usize);
    if pixels <= MAX_CHROMA_FULLRES_PIXELS {
        return 1;
    }
    let ratio = pixels.div_ceil(MAX_CHROMA_FULLRES_PIXELS);
    let mut step = ratio.isqrt() as u32;
    if (step as usize).saturating_mul(step as usize) < ratio {
        step = step.saturating_add(1);
    }
    step.max(1)
}

fn extract_channel_over_white(
    rgba: &[u8],
    w: u32,
    h: u32,
    step: u32,
    channel: ColorChannel,
) -> GrayImage {
    let step = step.max(1);
    let dw = w.div_ceil(step);
    let dh = h.div_ceil(step);
    let mut out = GrayImage::new(dw, dh);
    let idx = match channel {
        ColorChannel::Red => 0,
        ColorChannel::Blue => 2,
    };

    let row_stride = (w as usize) * 4;
    for dy in 0..dh {
        let sy = (dy * step) as usize;
        let row_start = sy * row_stride;
        for dx in 0..dw {
            let sx = (dx * step) as usize;
            let offset = row_start + sx * 4;
            let c = rgba[offset + idx] as u32;
            let a = rgba[offset + 3] as u32;
            let val = ((c * a + 255 * (255 - a)) / 255) as u8;
            out.put_pixel(dx, dy, Luma([val]));
        }
    }
    out
}

fn invert_luma(img: &GrayImage) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for (dst, src) in out.pixels_mut().zip(img.pixels()) {
        *dst = Luma([255 - src[0]]);
    }
    out
}

fn pad_white_quiet_zone(img: &GrayImage) -> GrayImage {
    let pw = img.width().saturating_add(QUIET_ZONE_PX * 2);
    let ph = img.height().saturating_add(QUIET_ZONE_PX * 2);
    let mut padded = GrayImage::from_pixel(pw, ph, Luma([255]));
    image::imageops::replace(&mut padded, img, QUIET_ZONE_PX as i64, QUIET_ZONE_PX as i64);
    padded
}

fn choose_upscale_factor(w: u32, h: u32) -> Option<u32> {
    let px_count = (w as usize).checked_mul(h as usize)?;
    if px_count == 0 || px_count > MAX_UPSCALE_INPUT_PIXELS {
        return None;
    }
    let preferred = if w.min(h) < TINY_CROP_DIM_PX { 4 } else { 3 };
    for factor in (2..=preferred).rev() {
        let scale_area = (factor as usize).saturating_mul(factor as usize);
        if px_count.saturating_mul(scale_area) <= MAX_UPSCALED_PIXELS {
            return Some(factor);
        }
    }
    None
}

fn upscale_nearest(img: &GrayImage, factor: u32) -> GrayImage {
    if factor <= 1 {
        return img.clone();
    }
    let sw = img.width();
    let sh = img.height();
    let dw = sw.saturating_mul(factor);
    let dh = sh.saturating_mul(factor);
    let sw_usize = sw as usize;
    let dw_usize = dw as usize;
    let factor_usize = factor as usize;
    let src = img.as_raw();
    let mut dst = vec![255u8; dw_usize.saturating_mul(dh as usize)];

    for sy in 0..sh as usize {
        let src_row = &src[sy * sw_usize..(sy + 1) * sw_usize];
        let first_dy = sy * factor_usize;
        let first_dst_start = first_dy * dw_usize;
        for (sx, &val) in src_row.iter().enumerate() {
            let dx_start = sx * factor_usize;
            dst[first_dst_start + dx_start..first_dst_start + dx_start + factor_usize].fill(val);
        }
        for rep in 1..factor_usize {
            let dst_start = (first_dy + rep) * dw_usize;
            dst.copy_within(first_dst_start..first_dst_start + dw_usize, dst_start);
        }
    }

    GrayImage::from_raw(dw, dh, dst).unwrap_or_else(|| GrayImage::new(0, 0))
}

fn sharpen_3x3(img: &GrayImage) -> GrayImage {
    let w = img.width() as usize;
    let h = img.height() as usize;
    if w < 3 || h < 3 {
        return img.clone();
    }
    let src = img.as_raw();
    let mut dst = src.clone();

    for y in 1..(h - 1) {
        let row = y * w;
        let up = (y - 1) * w;
        let down = (y + 1) * w;
        for x in 1..(w - 1) {
            let c = src[row + x] as i32;
            let neighbors = src[up + x] as i32
                + src[down + x] as i32
                + src[row + x - 1] as i32
                + src[row + x + 1] as i32;
            let sharpened = (5 * c - neighbors).clamp(0, 255) as u8;
            dst[row + x] = sharpened;
        }
    }

    GrayImage::from_raw(img.width(), img.height(), dst).unwrap_or_else(|| img.clone())
}

fn scan_luma_plane(plane: &GrayImage, base_scale: f64, hits: &mut Vec<DecodedHit>) {
    let w = plane.width();
    let h = plane.height();
    let upscale = choose_upscale_factor(w, h);

    let normal_padded = pad_white_quiet_zone(plane);
    // `rqrr` binarizes dark modules as `< row_avg`, so light-on-dark QR codes
    // are invisible on the normal pass; always scanning inverted luma too
    // catches both single inverted codes and mixed dark/light pairs.
    let inverted = invert_luma(plane);
    let inverted_padded = pad_white_quiet_zone(&inverted);

    if let Some(factor) = upscale {
        decode_padded_into(normal_padded.clone(), QUIET_ZONE_PX, base_scale, hits);
        decode_padded_into(inverted_padded.clone(), QUIET_ZONE_PX, base_scale, hits);

        if w.min(h) < SMALL_CROP_DIM_PX || hits.is_empty() {
            let up_pad = QUIET_ZONE_PX.saturating_mul(factor);
            let up_scale = base_scale * (factor as f64);

            decode_padded_into(
                upscale_nearest(&normal_padded, factor),
                up_pad,
                up_scale,
                hits,
            );
            decode_padded_into(
                upscale_nearest(&inverted_padded, factor),
                up_pad,
                up_scale,
                hits,
            );

            if hits.is_empty() {
                // Fractional DPI scaling (~1.5x–2.2x) blurs 1-module finder
                // rings across adjacent pixels so `rqrr`'s adaptive row
                // threshold bridges or breaks the ring; sharpening before
                // bicubic upscale restores ring separation within the same
                // `<= ~1.65 MP` upscale budget.
                for src in [&normal_padded, &inverted_padded] {
                    let sharpened = sharpen_3x3(src);
                    let uw = sharpened.width().saturating_mul(factor);
                    let uh = sharpened.height().saturating_mul(factor);
                    let resized =
                        image::imageops::resize(&sharpened, uw, uh, FilterType::CatmullRom);
                    decode_padded_into(resized, up_pad, up_scale, hits);
                }
            }
        }
    } else {
        decode_padded_into(normal_padded, QUIET_ZONE_PX, base_scale, hits);
        decode_padded_into(inverted_padded, QUIET_ZONE_PX, base_scale, hits);
    }
}

fn decode_padded_into(padded: GrayImage, pad_px: u32, scale: f64, hits: &mut Vec<DecodedHit>) {
    let mut prepared = rqrr::PreparedImage::prepare(padded);
    let grids = prepared.detect_grids();

    let mut raw = Vec::new();
    for grid in grids {
        raw.clear();
        if grid.decode_to(&mut raw).is_ok() {
            let text = match String::from_utf8(raw.clone()) {
                Ok(s) => s,
                Err(_) => String::from_utf8_lossy(&raw).into_owned(),
            };
            if text.is_empty() {
                continue;
            }
            let sum_x: f64 = grid.bounds.iter().map(|p| p.x as f64).sum();
            let sum_y: f64 = grid.bounds.iter().map(|p| p.y as f64).sum();
            let min_y = grid.bounds.iter().map(|p| p.y).min().unwrap_or(0) as f64;
            let max_y = grid.bounds.iter().map(|p| p.y).max().unwrap_or(0) as f64;

            let safe_scale = if scale > 0.0 && scale.is_finite() {
                scale
            } else {
                1.0
            };
            let raw_cx = (sum_x * 0.25 - pad_px as f64) / safe_scale;
            let raw_cy = (sum_y * 0.25 - pad_px as f64) / safe_scale;
            let raw_span_y = ((max_y - min_y) / safe_scale).max(16.0);

            hits.push(DecodedHit {
                payload: text,
                cx: if raw_cx.is_finite() { raw_cx } else { 0.0 },
                cy: if raw_cy.is_finite() { raw_cy } else { 0.0 },
                span_y: if raw_span_y.is_finite() {
                    raw_span_y
                } else {
                    16.0
                },
            });
        }
    }
}

fn finalize_hits(mut hits: Vec<DecodedHit>) -> Vec<String> {
    if hits.is_empty() {
        return Vec::new();
    }

    // Group codes whose vertical centers fall within the same horizontal row
    // band before sorting left-to-right: perspective corner estimation between
    // normal and inverted codes on the same row can jitter `cy` by 1–2 px,
    // which would otherwise flip reading order under a naive `(cy, cx)` sort.
    hits.sort_by(|a, b| a.cy.total_cmp(&b.cy).then_with(|| a.cx.total_cmp(&b.cx)));

    let mut row_start = 0;
    while row_start < hits.len() {
        let anchor_cy = hits[row_start].cy;
        let anchor_span = hits[row_start].span_y;
        let mut row_end = row_start + 1;
        while row_end < hits.len() {
            let tol = (anchor_span.min(hits[row_end].span_y) * 0.4).max(8.0);
            if hits[row_end].cy - anchor_cy <= tol {
                row_end += 1;
            } else {
                break;
            }
        }
        hits[row_start..row_end]
            .sort_by(|a, b| a.cx.total_cmp(&b.cx).then_with(|| a.cy.total_cmp(&b.cy)));
        row_start = row_end;
    }

    let mut seen = HashSet::with_capacity(hits.len());
    let mut payloads = Vec::with_capacity(hits.len());
    for hit in hits {
        if seen.insert(hit.payload.clone()) {
            payloads.push(hit.payload);
        }
    }
    payloads
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrcode::{Color, QrCode};

    fn render_qr_rgba(
        data: &[u8],
        module_px: u32,
        quiet_modules: u32,
        dark: [u8; 4],
        light: [u8; 4],
    ) -> (Vec<u8>, u32, u32) {
        let code = QrCode::new(data).expect("valid qr payload");
        let n = code.width() as u32;
        let total_modules = n + quiet_modules * 2;
        let side = total_modules * module_px;
        let mut rgba = Vec::with_capacity((side * side * 4) as usize);

        for y in 0..side {
            let my = y / module_px;
            for x in 0..side {
                let mx = x / module_px;
                let is_dark = mx >= quiet_modules
                    && mx < quiet_modules + n
                    && my >= quiet_modules
                    && my < quiet_modules + n
                    && code[((mx - quiet_modules) as usize, (my - quiet_modules) as usize)]
                        == Color::Dark;
                rgba.extend_from_slice(if is_dark { &dark } else { &light });
            }
        }
        (rgba, side, side)
    }

    #[test]
    fn decodes_standard_qr_code() {
        let (rgba, w, h) = render_qr_rgba(
            b"https://github.com/shotori-screenshot/shotori/issues/46",
            6,
            4,
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        );
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(
            decoded,
            vec!["https://github.com/shotori-screenshot/shotori/issues/46"]
        );
    }

    #[test]
    fn decodes_inverted_light_on_dark_qr_code() {
        let (rgba, w, h) = render_qr_rgba(
            b"shotori-inverted-qr",
            6,
            4,
            [255, 255, 255, 255],
            [18, 18, 22, 255],
        );
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(decoded, vec!["shotori-inverted-qr"]);
    }

    #[test]
    fn decodes_tightly_cropped_qr_code_without_quiet_zone() {
        let (rgba, w, h) = render_qr_rgba(
            b"tight-crop-payload",
            6,
            0,
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        );
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(decoded, vec!["tight-crop-payload"]);
    }

    #[test]
    fn decodes_transparent_background_qr_code() {
        let (rgba, w, h) = render_qr_rgba(b"transparent-bg-qr", 6, 4, [0, 0, 0, 255], [0, 0, 0, 0]);
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(decoded, vec!["transparent-bg-qr"]);
    }

    #[test]
    fn decodes_multiple_distinct_qr_codes_and_deduplicates_repeats() {
        let (q1, w1, h1) = render_qr_rgba(b"first-qr", 6, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let (q2, w2, h2) = render_qr_rgba(b"second-qr", 6, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let (q3, w3, h3) = render_qr_rgba(b"first-qr", 6, 4, [0, 0, 0, 255], [255, 255, 255, 255]);

        let gap = 24u32;
        let total_w = w1 + gap + w2 + gap + w3;
        let total_h = h1.max(h2).max(h3) + 16;
        let mut canvas = vec![255u8; (total_w * total_h * 4) as usize];

        let mut blit = |src: &[u8], sw: u32, sh: u32, ox: u32, oy: u32| {
            for y in 0..sh {
                for x in 0..sw {
                    let s = ((y * sw + x) * 4) as usize;
                    let d = (((oy + y) * total_w + (ox + x)) * 4) as usize;
                    canvas[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        };
        blit(&q1, w1, h1, 0, 8);
        blit(&q2, w2, h2, w1 + gap, 8);
        blit(&q3, w3, h3, w1 + gap + w2 + gap, 8);

        let decoded = scan_qr(&canvas, total_w, total_h).expect("scan succeeds");
        assert_eq!(decoded, vec!["first-qr", "second-qr"]);
        assert_eq!(join_payloads(&decoded), "first-qr\nsecond-qr");
    }

    #[test]
    fn decodes_mixed_normal_and_inverted_qr_codes_in_one_image() {
        let (normal, nw, nh) =
            render_qr_rgba(b"light-card-qr", 6, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let (inverted, iw, ih) = render_qr_rgba(
            b"dark-card-qr",
            6,
            4,
            [255, 255, 255, 255],
            [18, 18, 22, 255],
        );

        let gap = 24u32;
        let total_w = nw + gap + iw;
        let total_h = nh.max(ih) + 16;

        let make_pair = |left: (&[u8], u32, u32), right: (&[u8], u32, u32)| {
            let mut canvas = [240u8, 240, 240, 255].repeat((total_w * total_h) as usize);
            let mut blit = |src: &[u8], sw: u32, sh: u32, ox: u32, oy: u32| {
                for y in 0..sh {
                    for x in 0..sw {
                        let s = ((y * sw + x) * 4) as usize;
                        let d = (((oy + y) * total_w + (ox + x)) * 4) as usize;
                        canvas[d..d + 4].copy_from_slice(&src[s..s + 4]);
                    }
                }
            };
            blit(left.0, left.1, left.2, 0, 8);
            blit(right.0, right.1, right.2, left.1 + gap, 10);
            canvas
        };

        let left_normal = make_pair((&normal, nw, nh), (&inverted, iw, ih));
        assert_eq!(
            scan_qr(&left_normal, total_w, total_h).expect("scan succeeds"),
            vec!["light-card-qr", "dark-card-qr"]
        );

        // Flip horizontal order (inverted on the left, normal on the right) to
        // verify reading order is spatial rather than pass-dependent.
        let left_inverted = make_pair((&inverted, iw, ih), (&normal, nw, nh));
        assert_eq!(
            scan_qr(&left_inverted, total_w, total_h).expect("scan succeeds"),
            vec!["dark-card-qr", "light-card-qr"]
        );

        // Verify 2x2 multi-row reading order (top-left, top-right, bottom-left,
        // bottom-right) with alternating polarities.
        let (tl, tlw, tlh) =
            render_qr_rgba(b"row1-left", 5, 4, [255, 255, 255, 255], [15, 15, 18, 255]);
        let (tr, trw, trh) =
            render_qr_rgba(b"row1-right", 5, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let (bl, blw, blh) =
            render_qr_rgba(b"row2-left", 5, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let (br, brw, brh) =
            render_qr_rgba(b"row2-right", 5, 4, [255, 255, 255, 255], [15, 15, 18, 255]);
        let gw = tlw.max(blw) + gap + trw.max(brw);
        let gh = tlh.max(trh) + gap + blh.max(brh) + 8;
        let mut grid_canvas = [240u8, 240, 240, 255].repeat((gw * gh) as usize);
        let mut blit_grid = |src: &[u8], sw: u32, sh: u32, ox: u32, oy: u32| {
            for y in 0..sh {
                for x in 0..sw {
                    let s = ((y * sw + x) * 4) as usize;
                    let d = (((oy + y) * gw + (ox + x)) * 4) as usize;
                    grid_canvas[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        };
        blit_grid(&tl, tlw, tlh, 0, 0);
        blit_grid(&tr, trw, trh, tlw.max(blw) + gap, 2);
        blit_grid(&bl, blw, blh, 0, tlh.max(trh) + gap + 2);
        blit_grid(&br, brw, brh, tlw.max(blw) + gap, tlh.max(trh) + gap);

        assert_eq!(
            scan_qr(&grid_canvas, gw, gh).expect("scan succeeds"),
            vec!["row1-left", "row1-right", "row2-left", "row2-right"]
        );
    }

    #[test]
    fn decodes_tiny_1px_and_2px_module_qr_codes() {
        let cases = [
            (
                "tiny-1px-standard",
                1u32,
                4u32,
                [0, 0, 0, 255],
                [255, 255, 255, 255],
            ),
            ("tiny-1px-tight", 1, 0, [0, 0, 0, 255], [255, 255, 255, 255]),
            (
                "tiny-1px-inverted",
                1,
                0,
                [255, 255, 255, 255],
                [16, 16, 20, 255],
            ),
            (
                "tiny-2px-standard",
                2,
                4,
                [0, 0, 0, 255],
                [255, 255, 255, 255],
            ),
            (
                "tiny-2px-tight-inverted",
                2,
                0,
                [250, 250, 250, 255],
                [20, 20, 24, 255],
            ),
        ];

        for (payload, module_px, quiet_modules, dark, light) in cases {
            let (rgba, w, h) =
                render_qr_rgba(payload.as_bytes(), module_px, quiet_modules, dark, light);
            let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
            assert_eq!(decoded, vec![payload], "failed case: {payload} ({w}x{h})");
        }
    }

    #[test]
    fn decodes_white_modules_on_transparent_background() {
        let (rgba, w, h) = render_qr_rgba(
            b"white-on-alpha-qr",
            6,
            4,
            [255, 255, 255, 255],
            [0, 0, 0, 0],
        );
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(decoded, vec!["white-on-alpha-qr"]);

        let (tiny_rgba, tw, th) = render_qr_rgba(
            b"tiny-white-on-alpha",
            1,
            0,
            [255, 255, 255, 255],
            [0, 0, 0, 0],
        );
        assert_eq!(
            scan_qr(&tiny_rgba, tw, th).expect("scan succeeds"),
            vec!["tiny-white-on-alpha"]
        );
    }

    #[test]
    fn decodes_isoluminant_colored_qr_code() {
        let fg = [220u8, 30, 30, 255];
        let bg = [0u8, 110, 210, 255];
        let bt601_luma = |px: [u8; 4]| -> i32 {
            ((77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32 + 128) >> 8) as i32
        };
        assert!(
            (bt601_luma(fg) - bt601_luma(bg)).abs() <= 2,
            "expected iso-luminant pair, got {} vs {}",
            bt601_luma(fg),
            bt601_luma(bg)
        );

        let (rgba, w, h) = render_qr_rgba(b"isoluminant-brand-qr", 6, 4, fg, bg);
        let decoded = scan_qr(&rgba, w, h).expect("scan succeeds");
        assert_eq!(decoded, vec!["isoluminant-brand-qr"]);

        // Exercise the Blue-channel leg too: equal Red (`50`), iso-luminant
        // green-vs-blue pair (`77*50 + 150*48 == 11050` vs `77*50 + 29*248 == 11042`).
        let fg_equal_r = [50u8, 48, 0, 255];
        let bg_equal_r = [50u8, 0, 248, 255];
        assert!((bt601_luma(fg_equal_r) - bt601_luma(bg_equal_r)).abs() <= 2);
        let (rgba_b, wb, hb) =
            render_qr_rgba(b"isoluminant-blue-axis", 6, 4, fg_equal_r, bg_equal_r);
        assert_eq!(
            scan_qr(&rgba_b, wb, hb).expect("scan succeeds"),
            vec!["isoluminant-blue-axis"]
        );
    }

    #[test]
    fn decodes_antialiased_scaled_qr_code() {
        let payload = b"https://shotori.example/fractional-dpi-qr";
        let code = QrCode::new(payload).expect("valid qr payload");
        let total_modules = code.width() as u32 + 8;
        let (src_rgba, sw, sh) =
            render_qr_rgba(payload, 6, 4, [0, 0, 0, 255], [255, 255, 255, 255]);
        let src_img = image::RgbaImage::from_raw(sw, sh, src_rgba).expect("valid rgba image");

        // Test both exact 2.0 px/module and non-integer ~1.85 px/module to
        // cover subpixel phase shifts from 1.25x/1.5x fractional scaling.
        for target_side in [total_modules * 2, (total_modules * 185) / 100] {
            let scaled = image::imageops::resize(
                &src_img,
                target_side,
                target_side,
                image::imageops::FilterType::Triangle,
            );
            let decoded =
                scan_qr(scaled.as_raw(), scaled.width(), scaled.height()).expect("scan succeeds");
            assert_eq!(
                decoded,
                vec!["https://shotori.example/fractional-dpi-qr"],
                "failed antialiased resize at {target_side}x{target_side}"
            );
        }
    }

    #[test]
    fn returns_empty_when_no_qr_present() {
        let blank = vec![240u8; 128 * 128 * 4];
        let decoded = scan_qr(&blank, 128, 128).expect("scan succeeds");
        assert!(decoded.is_empty());
    }

    #[test]
    fn rejects_zero_dimensions_or_mismatched_buffer() {
        assert!(scan_qr(&[], 0, 10).is_err());
        assert!(scan_qr(&[], 10, 0).is_err());
        assert!(scan_qr(&[0u8; 12], 4, 4).is_err());
    }
}
