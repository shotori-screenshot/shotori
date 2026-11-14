//! # Export pipeline: logical selection → physical-pixel crop → image encode → disk
//!
//! A collection of pure functions (no Wayland/gpui window access) → unit
//! testable with synthetic pixels. Callers (the overlay) only pass the window
//! scale factor and handle error reporting.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gpui_kit::*;

use crate::model::settings::{
    DEFAULT_FILENAME_TEMPLATE, ExportFormat, ExportSettings, PngCompression,
    validate_filename_template,
};

/// Logical selection → physical-pixel crop.
/// Coordinates are scaled, rounded and clamped into the capture; an empty
/// selection returns `None`.
pub fn crop(
    rgba: &[u8],
    cap_w: u32,
    cap_h: u32,
    bounds: Bounds<Pixels>,
    scale: f32,
) -> Option<(u32, u32, Vec<u8>)> {
    // The row loop indexes rgba as cap_w×cap_h; a short buffer would
    // panic mid-copy — assert the contract where it's cheap.
    debug_assert_eq!(
        rgba.len(),
        cap_w as usize * cap_h as usize * 4,
        "crop buffer is not a tightly packed rgba8 capture"
    );
    let clamp = |v: f32, max: u32| v.round().clamp(0., max as f32) as u32;
    let x0 = clamp(f32::from(bounds.left()) * scale, cap_w);
    let y0 = clamp(f32::from(bounds.top()) * scale, cap_h);
    let x1 = clamp(f32::from(bounds.right()) * scale, cap_w);
    let y1 = clamp(f32::from(bounds.bottom()) * scale, cap_h);
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    let cap_stride = cap_w as usize * 4;
    for row in 0..h as usize {
        let src = (y0 as usize + row) * cap_stride + x0 as usize * 4;
        let dst = row * (w as usize * 4);
        out[dst..dst + w as usize * 4].copy_from_slice(&rgba[src..src + w as usize * 4]);
    }
    Some((w, h, out))
}

/// The conventional save directory `~/Pictures/Shotori` (dialog default /
/// created on demand)
pub(crate) fn save_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join("Pictures/Shotori"))
}

/// Encode RGBA8 pixels as PNG with balanced compression (the png crate's
/// default — the quality/size balance for files on disk)
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    encode_png_with(w, h, rgba, png::Compression::Balanced)
}

/// Encode RGBA8 pixels as PNG with fast compression (fdeflate — a
/// specialized DEFLATE tuned for PNG).
///
/// For the clipboard and notification thumbnails: the bytes go through a
/// pipe or a 256px downscale, not a download — the speedup is worth the
/// larger size (measured).
pub fn encode_png_fast(w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    encode_png_with(w, h, rgba, png::Compression::Fast)
}

fn encode_png_with(
    w: u32,
    h: u32,
    rgba: &[u8],
    compression: png::Compression,
) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(compression);
    let mut writer = enc.write_header().context("PNG header")?;
    writer.write_image_data(rgba).context("PNG data")?;
    writer.finish().context("PNG IEND chunk")?;
    Ok(out)
}

/// Encode RGBA8 pixels as PNG and write to `path` — the user-chosen save
/// location from the file dialog. Overwrites plainly: the dialog has
/// already asked for confirmation. (The pre-dialog fixed-path writer used
/// atomic `create_new` + suffixes against concurrent instances; with a
/// picker in front, collisions are the dialog's business.)
pub fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    // Balanced tier on purpose: saved files are kept and shared, so size
    // beats encode time (sub-second anyway, ~0.2 s on a 1440p screen).
    // Fast tier measured +14% on real desktop content, up to multi-MB on
    // gradients/photos.
    let bytes = encode_png(w, h, rgba)?;
    std::fs::write(path, &bytes)
        .with_context(|| format!("writing {} ({} KB)", path.display(), bytes.len() / 1024))?;
    Ok(())
}

/// Encode RGBA8 pixels into PNG, JPEG (composited over white), or lossless WebP.
pub fn encode_image(
    w: u32,
    h: u32,
    rgba: &[u8],
    format: ExportFormat,
    png_compression: PngCompression,
    jpeg_quality: u8,
) -> anyhow::Result<Vec<u8>> {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    anyhow::ensure!(
        rgba.len() == expected,
        "RGBA buffer length {} does not match {}x{}x4",
        rgba.len(),
        w,
        h
    );
    match format {
        ExportFormat::Png => encode_png_with(w, h, rgba, png_compression.to_png()),
        ExportFormat::Jpeg => {
            let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
            for &[r, g, b, alpha] in rgba.as_chunks::<4>().0 {
                let a = u16::from(alpha);
                if a == 255 {
                    rgb.extend_from_slice(&[r, g, b]);
                } else {
                    let blend = |c: u8| ((u16::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
                    rgb.push(blend(r));
                    rgb.push(blend(g));
                    rgb.push(blend(b));
                }
            }
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out,
                jpeg_quality.clamp(1, 100),
            )
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .context("JPEG encode")?;
            Ok(out)
        }
        ExportFormat::Webp => {
            let mut out = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .encode(rgba, w, h, image::ExtendedColorType::Rgba8)
                .context("WebP encode")?;
            Ok(out)
        }
    }
}

/// Infer the image format from `path`'s extension, falling back to `fallback`
/// when unknown or missing.
pub fn format_for_path(path: &Path, fallback: ExportFormat) -> ExportFormat {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(ExportFormat::from_id)
        .unwrap_or(fallback)
}

/// Format a filename stem (without extension) from a template, timestamp, and
/// physical dimensions. Never panics and never produces path separators: any
/// invalid template or formatting failure degrades to `DEFAULT_FILENAME_TEMPLATE`.
pub fn format_filename_stem(
    template: &str,
    now: chrono::DateTime<chrono::Local>,
    w: u32,
    h: u32,
) -> String {
    let try_render = |tpl: &str| -> Option<String> {
        validate_filename_template(tpl).ok()?;
        let sub = tpl
            .replace("{w}", &w.to_string())
            .replace("{h}", &h.to_string());
        let mut out = String::new();
        write!(&mut out, "{}", now.format(&sub)).ok()?;
        let trimmed = out.trim();
        if trimmed.is_empty()
            || trimmed == "."
            || trimmed == ".."
            || trimmed.contains(['/', '\\', '\0'])
        {
            return None;
        }
        Some(trimmed.to_owned())
    };
    try_render(template)
        .or_else(|| try_render(DEFAULT_FILENAME_TEMPLATE))
        .unwrap_or_else(|| format!("Shotori_{w}x{h}"))
}

/// Format a suggested filename (stem + configured extension) for `export`.
pub fn suggested_filename_for(
    export: &ExportSettings,
    now: chrono::DateTime<chrono::Local>,
    w: u32,
    h: u32,
) -> String {
    let stem = format_filename_stem(&export.filename_template, now, w, h);
    format!("{stem}.{}", export.format.extension())
}

/// Format a suggested filename using the active `[export]` settings.
pub fn suggested_filename(w: u32, h: u32) -> String {
    suggested_filename_for(
        crate::model::settings::current().export(),
        chrono::Local::now(),
        w,
        h,
    )
}

/// Encode and write `rgba` to `path`, choosing format from `path`'s extension
/// (or `export.format` when extensionless/unrecognized).
pub fn save_image_with(
    path: &Path,
    w: u32,
    h: u32,
    rgba: &[u8],
    export: &ExportSettings,
) -> anyhow::Result<()> {
    let format = format_for_path(path, export.format);
    let bytes = encode_image(
        w,
        h,
        rgba,
        format,
        export.png_compression,
        export.jpeg_quality,
    )?;
    std::fs::write(path, &bytes)
        .with_context(|| format!("writing {} ({} KB)", path.display(), bytes.len() / 1024))?;
    Ok(())
}

/// Encode and write `rgba` to `path` using the active `[export]` settings.
pub fn save_image(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    save_image_with(path, w, h, rgba, crate::model::settings::current().export())
}

/// Atomically create a uniquely named file inside `dir` (`{stem}.{ext}`, then
/// `{stem}_1.{ext}`, `{stem}_2.{ext}`, …) so rapid consecutive direct saves
/// never overwrite an existing screenshot (issue #42).
pub fn save_to_dir_with(
    dir: &Path,
    stem: &str,
    w: u32,
    h: u32,
    rgba: &[u8],
    export: &ExportSettings,
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating save directory {}", dir.display()))?;
    let bytes = encode_image(
        w,
        h,
        rgba,
        export.format,
        export.png_compression,
        export.jpeg_quality,
    )?;
    let ext = export.format.extension();
    for attempt in 0u32..10_000 {
        let filename = if attempt == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}_{attempt}.{ext}")
        };
        let candidate = dir.join(filename);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                if let Err(err) = file.write_all(&bytes).and_then(|()| file.flush()) {
                    drop(file);
                    drop(std::fs::remove_file(&candidate));
                    return Err(err).with_context(|| format!("writing {}", candidate.display()));
                }
                return Ok(candidate);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("creating {}", candidate.display()));
            }
        }
    }
    anyhow::bail!(
        "could not find an unused filename for {stem}.{ext} in {}",
        dir.display()
    )
}

/// Save directly into the configured default folder without opening a file
/// dialog (issue #42).
pub fn save_to_default_dir(w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<PathBuf> {
    let export = crate::model::settings::current().export();
    let dir = export.resolved_save_dir()?;
    let stem = format_filename_stem(&export.filename_template, chrono::Local::now(), w, h);
    save_to_dir_with(&dir, &stem, w, h, rgba, export)
}

#[cfg(test)]
mod tests {
    // Explicit imports (same reason as selection.rs: avoid gpui's test macro
    // shadowing the built-in #[test])
    use super::{
        crop, encode_image, format_filename_stem, format_for_path, save_image_with, save_png,
        save_to_dir_with,
    };
    use crate::model::settings::{ExportFormat, ExportSettings, PngCompression};
    use chrono::TimeZone as _;
    use gpui_kit::{Bounds, Pixels, point, px, size};
    use std::path::Path;

    /// 4×3 synthetic image: pixel value = (x, y, 0, 255) for easy
    /// coordinate-mapping assertions
    fn gradient_4x3() -> (u32, u32, Vec<u8>) {
        let (w, h) = (4u32, 3u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = x as u8;
                rgba[i + 1] = y as u8;
                rgba[i + 2] = 0;
                rgba[i + 3] = 255;
            }
        }
        (w, h, rgba)
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn crop_scale1_extracts_exact_pixels() {
        let (w, h, rgba) = gradient_4x3();
        let (cw, ch, out) = crop(&rgba, w, h, bounds(1., 1., 2., 1.), 1.0).unwrap();
        assert_eq!((cw, ch), (2, 1));
        // two pixels: (1,1) and (2,1)
        assert_eq!(&out[..4], &[1, 1, 0, 255]);
        assert_eq!(&out[4..8], &[2, 1, 0, 255]);
    }

    #[test]
    fn crop_scale2_logical_to_physical() {
        let (w, h, rgba) = gradient_4x3();
        // logical (0.5, 0.5) size 1×1 → physical x∈[1,3) y∈[1,3) → 2×2 pixels
        let (cw, ch, out) = crop(&rgba, w, h, bounds(0.5, 0.5, 1., 1.), 2.0).unwrap();
        assert_eq!((cw, ch), (2, 2));
        assert_eq!(&out[..4], &[1, 1, 0, 255]); // first row, leftmost (1,1)
        assert_eq!(&out[4..8], &[2, 1, 0, 255]); // (2,1)
    }

    #[test]
    fn crop_clamps_out_of_bounds() {
        let (w, h, rgba) = gradient_4x3();
        // bottom-right beyond the capture: clamped to (4,3), yields 2×2
        let (cw, ch, _) = crop(&rgba, w, h, bounds(2., 1., 99., 99.), 1.0).unwrap();
        assert_eq!((cw, ch), (2, 2));
    }

    #[test]
    fn crop_empty_selection_returns_none() {
        let (w, h, rgba) = gradient_4x3();
        assert!(crop(&rgba, w, h, bounds(4., 0., 4., 3.), 1.0).is_none()); // zero width
        assert!(crop(&rgba, w, h, bounds(0., 3., 4., 0.), 1.0).is_none()); // zero height
    }

    #[test]
    fn png_encodes_and_reads_back() {
        let (w, h, rgba) = gradient_4x3();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.png");
        save_png(&path, w, h, &rgba).unwrap();

        let img = image::open(&path).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (w, h));
        assert_eq!(img.get_pixel(2, 1).0, [2, 1, 0, 255]);
    }

    #[test]
    fn jpeg_and_webp_encode_and_decode_cleanly() {
        let (w, h, _) = gradient_4x3();
        // Translucent red pixel (255, 0, 0, 0) composites to white in JPEG
        // while staying lossless RGBA in WebP.
        let mut rgba = vec![200u8; (w * h * 4) as usize];
        rgba[..4].copy_from_slice(&[255, 0, 0, 0]);

        let jpg = encode_image(w, h, &rgba, ExportFormat::Jpeg, PngCompression::Fast, 95).unwrap();
        let decoded_jpg = image::load_from_memory(&jpg).unwrap().to_rgb8();
        assert_eq!(decoded_jpg.dimensions(), (w, h));
        assert!(decoded_jpg.get_pixel(0, 0).0.iter().all(|&c| c >= 250));

        let webp = encode_image(w, h, &rgba, ExportFormat::Webp, PngCompression::Best, 90).unwrap();
        let decoded_webp = image::load_from_memory(&webp).unwrap().to_rgba8();
        assert_eq!(decoded_webp.dimensions(), (w, h));
        assert_eq!(decoded_webp.get_pixel(0, 0).0, [255, 0, 0, 0]);
    }

    #[test]
    fn format_for_path_honors_extension_and_fallback() {
        assert_eq!(
            format_for_path(Path::new("shot.jpg"), ExportFormat::Png),
            ExportFormat::Jpeg
        );
        assert_eq!(
            format_for_path(Path::new("shot.JPEG"), ExportFormat::Png),
            ExportFormat::Jpeg
        );
        assert_eq!(
            format_for_path(Path::new("shot.webp"), ExportFormat::Png),
            ExportFormat::Webp
        );
        assert_eq!(
            format_for_path(Path::new("shot.unknown"), ExportFormat::Webp),
            ExportFormat::Webp
        );
    }

    #[test]
    fn filename_stem_substitutes_dimensions_and_falls_back_on_bad_template() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 14, 9, 5, 7)
            .single()
            .unwrap();
        assert_eq!(
            format_filename_stem("Clip_%Y%m%d_{w}x{h}", now, 1920, 1080),
            "Clip_20261114_1920x1080"
        );
        // Invalid template (contains path separator) degrades safely to factory
        let fallback = format_filename_stem("../escape", now, 800, 600);
        assert!(fallback.starts_with("Shotori_2026-11-14_09-05-07"));
    }

    #[test]
    fn save_to_dir_creates_missing_folders_and_suffixes_collisions() {
        let (w, h, rgba) = gradient_4x3();
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("sub/shots");
        let export = ExportSettings {
            format: ExportFormat::Webp,
            ..ExportSettings::default()
        };

        let p0 = save_to_dir_with(&nested, "Shot_test", w, h, &rgba, &export).unwrap();
        let p1 = save_to_dir_with(&nested, "Shot_test", w, h, &rgba, &export).unwrap();
        let p2 = save_to_dir_with(&nested, "Shot_test", w, h, &rgba, &export).unwrap();

        assert_eq!(p0.file_name().unwrap(), "Shot_test.webp");
        assert_eq!(p1.file_name().unwrap(), "Shot_test_1.webp");
        assert_eq!(p2.file_name().unwrap(), "Shot_test_2.webp");

        // Explicit extension override on save_image_with writes valid JPEG
        let jpg_path = nested.join("custom.jpg");
        save_image_with(&jpg_path, w, h, &rgba, &export).unwrap();
        assert_eq!(image::open(&jpg_path).unwrap().width(), w);
    }
}
