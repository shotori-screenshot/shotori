//! QR code detection and decoding via `rqrr`.

use anyhow::{Context as _, bail};
use image::{GrayImage, Luma};

/// White quiet-zone border added around cropped selections so finder patterns
/// sitting right against the image edge remain detectable.
const QUIET_ZONE_PX: u32 = 16;

/// Scan an RGBA8 buffer for QR codes and return all distinct non-empty payloads
/// in detection order.
///
/// Composite transparent pixels over white before luma conversion, pad with a
/// synthetic quiet zone for tight crops, and fall back to inverted luminance
/// when a dark-background (light-on-dark) code yields no hits.
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

    let luma = rgba_to_luma_over_white(rgba, w, h);
    let results = decode_luma(&luma);
    if !results.is_empty() {
        return Ok(results);
    }

    // `rqrr` binarizes dark modules as `< row_avg`, so light-on-dark QR codes
    // are invisible on the first pass; retrying on inverted luma catches them.
    let inverted = invert_luma(&luma);
    Ok(decode_luma(&inverted))
}

/// Join multiple decoded QR payloads into a single clipboard string.
pub fn join_payloads(payloads: &[String]) -> String {
    payloads.join("\n")
}

fn rgba_to_luma_over_white(rgba: &[u8], w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    let (chunks, _) = rgba.as_chunks::<4>();
    for (pixel, px) in out.pixels_mut().zip(chunks) {
        let a = px[3] as u32;
        let inv_a = 255 - a;
        let r = (px[0] as u32 * a + 255 * inv_a) / 255;
        let g = (px[1] as u32 * a + 255 * inv_a) / 255;
        let b = (px[2] as u32 * a + 255 * inv_a) / 255;
        let luma = ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8;
        *pixel = Luma([luma]);
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

fn decode_luma(img: &GrayImage) -> Vec<String> {
    let padded = pad_white_quiet_zone(img);
    let mut prepared = rqrr::PreparedImage::prepare(padded);
    let grids = prepared.detect_grids();

    let mut decoded = Vec::new();
    let mut raw = Vec::new();
    for grid in grids {
        raw.clear();
        if grid.decode_to(&mut raw).is_ok() {
            let text = match String::from_utf8(raw.clone()) {
                Ok(s) => s,
                Err(_) => String::from_utf8_lossy(&raw).into_owned(),
            };
            if !text.is_empty() && !decoded.contains(&text) {
                decoded.push(text);
            }
        }
    }
    decoded
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
