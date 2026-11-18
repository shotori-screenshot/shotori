//! Pixel regressions over rasterized, independently generated document scenes.

use super::{Fold, ScrollSheet, SheetLimits};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use std::collections::HashMap;
use std::sync::OnceLock;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 640;
const PAGE_HEIGHT: u32 = 2_400;

fn scene_font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        FontRef::try_from_slice(include_bytes!("../../../assets/DejaVuSans-Bold.ttf")).unwrap()
    })
}

fn document_base() -> &'static [u8] {
    static DOC: OnceLock<Vec<u8>> = OnceLock::new();
    DOC.get_or_init(|| {
        let mut page = vec![255; (WIDTH * PAGE_HEIGHT * 4) as usize];
        let font = scene_font();
        let scaled = font.as_scaled(PxScale::from(14.));
        let mut y = 24;
        let mut line = 0;
        while y + 24 < PAGE_HEIGHT {
            let text = format!(
                "Item {line:03}: sample value {} / {}",
                line * 17 + 3,
                line * 31 + 7
            );
            let dark = line % 7 == 3;
            if dark {
                for row in y - 4..y + 20 {
                    for x in 24..WIDTH - 28 {
                        let i = ((row * WIDTH + x) * 4) as usize;
                        page[i..i + 3].fill(24);
                    }
                }
            }
            let mut pen = point(32., y as f32 + scaled.ascent());
            for ch in text.chars() {
                let mut glyph = scaled.scaled_glyph(ch);
                let advance = scaled.h_advance(glyph.id);
                glyph.position = pen;
                if let Some(outline) = font.outline_glyph(glyph) {
                    let bounds = outline.px_bounds();
                    outline.draw(|x, y, coverage| {
                        let px = bounds.min.x as i32 + x as i32;
                        let py = bounds.min.y as i32 + y as i32;
                        if (0..WIDTH as i32).contains(&px) && (0..PAGE_HEIGHT as i32).contains(&py)
                        {
                            let i = ((py as u32 * WIDTH + px as u32) * 4) as usize;
                            for value in &mut page[i..i + 3] {
                                *value = (f32::from(*value) * (1. - coverage)
                                    + if dark { 245. } else { 40. } * coverage)
                                    .round() as u8;
                            }
                        }
                    });
                }
                pen.x += advance;
            }
            y += 30 + line % 17;
            line += 1;
        }
        page
    })
}

fn document() -> Vec<u8> {
    document_base().to_vec()
}

fn viewport(page: &[u8], top: u32) -> Vec<u8> {
    page[(top * WIDTH * 4) as usize..((top + HEIGHT) * WIDTH * 4) as usize].to_vec()
}

fn segment_distance(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
    ((x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2)).sqrt()
}

/// A thin antialiased icon over a pale circular face, with a diffuse shadow.
fn control(frame: &mut [u8], center: (f32, f32), opacity: f32) {
    let x0 = (center.0 - 25.).floor().max(0.) as u32;
    let x1 = (center.0 + 25.).ceil().min(WIDTH as f32) as u32;
    let y0 = (center.1 - 25.).floor().max(0.) as u32;
    let y1 = (center.1 + 25.).ceil().min(HEIGHT as f32) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - center.0, y as f32 + 0.5 - center.1);
            let r = dx.hypot(dy);
            if r >= 24. {
                continue;
            }
            let shadow = ((24. - r) / 8.).clamp(0., 1.) * 0.07;
            let face = (16.5 - r).clamp(0., 1.) * opacity;
            let rim = (1. - (r - 16.).abs()).clamp(0., 1.);
            let distance = segment_distance(dx, dy, (0., -6.), (0., 6.))
                .min(segment_distance(dx, dy, (-5., 1.), (0., 6.)))
                .min(segment_distance(dx, dy, (0., 6.), (5., 1.)));
            let ink = (1.2 - distance).clamp(0., 1.);
            let i = ((y * WIDTH + x) * 4) as usize;
            for channel in &mut frame[i..i + 3] {
                let mut value = f32::from(*channel);
                for (alpha, color) in [(shadow, 150.), (face, 255.), (rim, 224.), (ink, 100.)] {
                    value = value * (1. - alpha) + color * alpha;
                }
                *channel = value.round() as u8;
            }
        }
    }
}

fn assert_pixels(actual: &[u8], expected: &[u8], first_row: u32, last_row: u32) {
    assert_eq!(actual.len(), expected.len());
    let range = (first_row * WIDTH * 4) as usize..(last_row * WIDTH * 4) as usize;
    if actual[range.clone()] == expected[range.clone()] {
        return;
    }
    let mismatches = actual[range.clone()]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected[range].as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize + first_row as usize))
        .collect::<Vec<_>>();
    panic!(
        "{} mismatches, first {:?}",
        mismatches.len(),
        mismatches.first()
    );
}

#[test]
fn pale_control_at_top_edge_does_not_repeat_over_text() {
    let page = document_base();
    let capture = |top| {
        let mut rgba = viewport(page, top);
        control(&mut rgba, (239.25, 22.75), 1.);
        rgba
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (16..=320).step_by(16) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 16, .. }),
            "{top}: {result:?}"
        );
    }
    let expected = &page[..((HEIGHT + 320) * WIDTH * 4) as usize];
    // The first viewport's very top is never exposed again on a downward walk.
    assert_pixels(sheet.canvas(), expected, 64, HEIGHT + 320);
}

#[test]
fn bottom_control_does_not_accumulate_and_exposed_tail_recovers() {
    let page = document_base();
    for step in [16, 40] {
        let capture = |top| {
            let mut rgba = viewport(page, top);
            control(&mut rgba, (239.25, HEIGHT as f32 - 22.75), 1.);
            rgba
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        for top in (step..=320).step_by(step as usize) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewBottom { .. }),
                "step {step}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 320) * WIDTH * 4) as usize];
        // The current bottommost occlusion has never been seen unobstructed.
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 320 - 80);
        assert!(matches!(
            sheet.push(&viewport(page, 328)),
            Fold::GrewBottom { .. }
        ));
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + 328) * WIDTH * 4) as usize],
            0,
            HEIGHT + 328,
        );
    }
}

#[test]
fn bottom_control_does_not_repeat_when_capturing_upward() {
    let page = document_base();
    for (gap, opacity, step) in [(22.75, 1., 16), (4.75, 0.85, 40)] {
        let capture = |top| {
            let mut rgba = viewport(page, top);
            control(&mut rgba, (239.25, HEIGHT as f32 - gap), opacity);
            rgba
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(320), SheetLimits::default()).unwrap();
        for top in (0..320).step_by(step as usize).rev() {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewTop { .. }),
                "step {step}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 320) * WIDTH * 4) as usize];
        export_scene(
            &format!("up-{step}-{gap}-{opacity}"),
            &capture(320),
            &sheet,
            expected,
        );
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 320 - 80);
    }
}

fn export_scene(name: &str, capture: &[u8], sheet: &ScrollSheet, expected: &[u8]) {
    let Some(directory) = std::env::var_os("SHOTORI_SCROLL_TEST_ARTIFACTS") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    for (suffix, rgba, height) in [
        ("capture", capture, HEIGHT),
        ("stitched", sheet.canvas(), sheet.dimensions().1),
        ("expected", expected, sheet.dimensions().1),
    ] {
        image::save_buffer(
            directory.join(format!("{name}-{suffix}.png")),
            rgba,
            WIDTH,
            height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[test]
fn changed_text_near_an_edge_control_refreshes_when_exposed() {
    let page = document_base();
    let mut first = viewport(page, 0);
    control(&mut first, (239.25, HEIGHT as f32 - 22.75), 1.);
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &first, SheetLimits::default()).unwrap();
    let mut changed = page.to_vec();
    for y in (530..546).chain(574..590) {
        for x in 28..WIDTH - 28 {
            let i = ((y * WIDTH + x) * 4) as usize;
            changed[i..i + 4].copy_from_slice(&[35, 110, 180, 255]);
        }
    }
    // Content inside the conservative control halo updates once a later
    // viewport exposes it outside the masked area.
    for top in [8, 16, 24, 32] {
        let mut capture = viewport(&changed, top);
        control(&mut capture, (239.25, HEIGHT as f32 - 22.75), 1.);
        assert!(matches!(sheet.push(&capture), Fold::GrewBottom { .. }));
        assert_pixels(
            sheet.canvas(),
            &changed[..((HEIGHT + top) * WIDTH * 4) as usize],
            530,
            546,
        );
    }
    assert_pixels(
        sheet.canvas(),
        &changed[..((HEIGHT + 32) * WIDTH * 4) as usize],
        574,
        590,
    );
}

/// A translucent edge layer with horizontally varying opacity and optional backdrop blur.
/// Blurs only the bottom strip (`fade_height` + kernel pad) rather than the full viewport.
fn blurred_viewport(page: &[u8], top: u32, sigma: f32, fade_height: u32) -> Vec<u8> {
    let mut rgba = viewport(page, top);
    let pad = (sigma.ceil() as u32 * 3).min(HEIGHT - fade_height);
    let strip_top = HEIGHT - fade_height - pad;
    let strip_h = fade_height + pad;
    let strip_bytes = (strip_top * WIDTH * 4) as usize;
    let blurred_raw = if sigma > 0. {
        let strip =
            image::RgbaImage::from_raw(WIDTH, strip_h, rgba[strip_bytes..].to_vec()).unwrap();
        image::imageops::blur(&strip, sigma).into_raw()
    } else {
        rgba[strip_bytes..].to_vec()
    };
    for y in HEIGHT - fade_height..HEIGHT {
        let fade = (y - (HEIGHT - fade_height)) as f32 / fade_height as f32;
        let local_y = y - strip_top;
        for x in 0..WIDTH {
            let alpha = fade * (0.45 + 0.2 * x as f32 / WIDTH as f32);
            let dst = ((y * WIDTH + x) * 4) as usize;
            let src = ((local_y * WIDTH + x) * 4) as usize;
            for c in 0..3 {
                let background = f32::from(blurred_raw[src + c]);
                rgba[dst + c] = (background * (1. - alpha) + 255. * alpha).round() as u8;
            }
        }
    }
    control(&mut rgba, (239.25, HEIGHT as f32 - 22.75), 1.);
    rgba
}

#[test]
fn blurred_edge_backdrop_does_not_slice_clear_text_into_pale_stripes() {
    let page = document_base();
    for (sigma, fade_height) in [(0., 120), (4., 160)] {
        let mut cache: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut capture = |top| {
            cache
                .entry(top)
                .or_insert_with(|| blurred_viewport(page, top, sigma, fade_height))
                .clone()
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(320), SheetLimits::default()).unwrap();
        for top in (0..320).step_by(16).rev() {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewTop { dy: 16, .. }),
                "sigma {sigma}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 320) * WIDTH * 4) as usize];
        export_scene(
            &format!("blurred-edge-{sigma}-{fade_height}"),
            &capture(320),
            &sheet,
            expected,
        );
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 320 - fade_height - 40);
        for top in (32..=320).step_by(32) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::Shifted { dy: 32 }),
                "sigma {sigma}, top {top}: {result:?}"
            );
        }
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 320 - fade_height - 40);
        // Advance past `320 + fade_height + halo` so rows `..HEIGHT + 320` clear the bottom fade.
        for top in (336..=576).step_by(16) {
            assert!(matches!(
                sheet.push(&capture(top)),
                Fold::GrewBottom { dy: 16, .. }
            ));
        }
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + 576) * WIDTH * 4) as usize],
            0,
            HEIGHT + 320,
        );
    }
}

fn scene_label(pixels: &mut [u8], x: f32, y: f32, text: &str) {
    let font = scene_font();
    let scaled = font.as_scaled(PxScale::from(12.));
    let mut pen = point(x, y + scaled.ascent());
    let height = pixels.len() / (WIDTH as usize * 4);
    for ch in text.chars() {
        let mut glyph = scaled.scaled_glyph(ch);
        let advance = scaled.h_advance(glyph.id);
        glyph.position = pen;
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|x, y, coverage| {
                let px = bounds.min.x as i32 + x as i32;
                let py = bounds.min.y as i32 + y as i32;
                if (0..WIDTH as i32).contains(&px) && (0..height as i32).contains(&py) {
                    let i = (py as usize * WIDTH as usize + px as usize) * 4;
                    for value in &mut pixels[i..i + 3] {
                        *value =
                            (f32::from(*value) * (1. - coverage) + 40. * coverage).round() as u8;
                    }
                }
            });
        }
        pen.x += advance;
    }
}

fn two_column_base() -> &'static [u8] {
    static BASE: OnceLock<Vec<u8>> = OnceLock::new();
    BASE.get_or_init(|| {
        let mut page = document();
        for y in 0..PAGE_HEIGHT {
            for x in 312..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                let value = if x == 320 || (y % 83 == 0 && x >= 340) {
                    220
                } else {
                    255
                };
                page[i..i + 3].fill(value);
            }
        }
        for (index, y) in (12..PAGE_HEIGHT - 36).step_by(83).enumerate() {
            scene_label(&mut page, 344., y as f32, &format!("Field {index:02}"));
            scene_label(
                &mut page,
                344.,
                (y + 20) as f32,
                &format!("Value {}", index * 19 + 5),
            );
        }
        page
    })
}

/// A two-column document with rules and a full-width bar appearing only after
/// the original heading has left the viewport.
fn pinned_heading_scene(background: u8) -> (Vec<u8>, Vec<u8>) {
    let page = two_column_base().to_vec();
    let mut bar = vec![255; (WIDTH * 48 * 4) as usize];
    for pixel in bar.as_chunks_mut::<4>().0 {
        pixel[..3].fill(background);
    }
    scene_label(&mut bar, 40., 12., "Sample document heading");
    for x in 0..WIDTH {
        let i = ((47 * WIDTH + x) * 4) as usize;
        bar[i..i + 3].fill(220);
    }
    (page, bar)
}

#[test]
fn heading_pinning_mid_scroll_preserves_both_columns() {
    for (background, step) in [(246, 24u32), (32, 160)] {
        let (page, bar) = pinned_heading_scene(background);
        let end = 480u32;
        let capture = |top| {
            let mut frame = viewport(&page, top);
            if top >= 160 {
                frame[..bar.len()].copy_from_slice(&bar);
            }
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        for top in (step..=end).step_by(step as usize) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewBottom { dy, .. } if dy == step),
                "{step} at {top}: {result:?}"
            );
            assert_eq!(sheet.dimensions(), (WIDTH, HEIGHT + top), "at {top}");
        }
        let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
        export_scene("late-heading", &capture(end), &sheet, expected);
        for top in (0..end).step_by(step as usize).rev() {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::Shifted { dy } if dy == -(step as i32)),
                "{step} at {top}: {result:?}"
            );
        }
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
    }
}

#[test]
fn sliding_heading_does_not_write_its_rules_into_the_document() {
    let (page, bar) = pinned_heading_scene(246);
    let capture = |top: u32| {
        let mut frame = viewport(&page, top);
        let visible = top.saturating_sub(152).min(48) as usize * WIDTH as usize * 4;
        frame[..visible].copy_from_slice(&bar[bar.len() - visible..]);
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (16..=320).step_by(16) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 16, .. }),
            "{top}: {result:?}"
        );
    }
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 320) * WIDTH * 4) as usize],
        0,
        HEIGHT + 320,
    );
}

#[test]
fn heading_unpins_when_returning_to_the_document_start() {
    let (page, bar) = pinned_heading_scene(246);
    let capture = |top| {
        let mut frame = viewport(&page, top);
        if top >= 160 {
            frame[..bar.len()].copy_from_slice(&bar);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(320), SheetLimits::default()).unwrap();
    for top in (0..320).step_by(16).rev() {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewTop { dy: 16, .. }),
            "{top}: {result:?}"
        );
    }
    assert_eq!(sheet.dimensions(), (WIDTH, HEIGHT + 320));
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 320) * WIDTH * 4) as usize],
        0,
        HEIGHT + 320,
    );
}

fn nearest_scale_rgba(src: &[u8], w: u32, h: u32, scale: u32) -> Vec<u8> {
    let dst_w = (w * scale) as usize;
    let mut out = Vec::with_capacity(dst_w * (h * scale) as usize * 4);
    let mut scaled_row = Vec::with_capacity(dst_w * 4);
    for y in 0..h as usize {
        scaled_row.clear();
        let row = &src[y * w as usize * 4..(y + 1) * w as usize * 4];
        for px in row.as_chunks::<4>().0 {
            for _ in 0..scale {
                scaled_row.extend_from_slice(px);
            }
        }
        for _ in 0..scale {
            out.extend_from_slice(&scaled_row);
        }
    }
    out
}

#[test]
fn wide_capture_keeps_rules_exact_across_the_sample_grid() {
    let (page, bar) = pinned_heading_scene(246);
    let scale = 3;
    let width = WIDTH * scale;
    let height = HEIGHT * scale;
    let used_rows = HEIGHT + 640;
    let page = nearest_scale_rgba(
        &page[..(used_rows * WIDTH * 4) as usize],
        WIDTH,
        used_rows,
        scale,
    );
    let bar = nearest_scale_rgba(&bar, WIDTH, 48, scale);
    let row = width as usize * 4;
    let capture = |top: usize| {
        let mut frame = page[top * row..(top + height as usize) * row].to_vec();
        if top >= 160 * scale as usize {
            frame[..bar.len()].copy_from_slice(&bar);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(width, height, &capture(0), SheetLimits::default()).unwrap();
    for top in [480, 960, 1_440, 1_920] {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 480, .. }),
            "{top}: {result:?}"
        );
        assert_eq!(sheet.dimensions(), (width, height + top as u32));
    }
    assert_eq!(sheet.canvas(), &page[..sheet.canvas().len()]);
}

#[test]
fn sidebar_that_stops_scrolling_remains_one_complete_column() {
    let (mut page, bar) = pinned_heading_scene(246);
    // A finite metadata panel ends before the viewport bottom when it pins.
    for y in 720..PAGE_HEIGHT {
        for x in 340..WIDTH {
            let i = ((y * WIDTH + x) * 4) as usize;
            page[i..i + 4].fill(255);
        }
    }
    let step = 40u32;
    let end = 640u32;
    let capture = |top: u32| {
        let mut frame = viewport(&page, top);
        let side_top = top.min(200);
        for y in 0..HEIGHT {
            let dst = ((y * WIDTH + 336) * 4) as usize;
            let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
            let bytes = ((WIDTH - 336) * 4) as usize;
            frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
        }
        if top >= 160 {
            frame[..bar.len()].copy_from_slice(&bar);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (step..=end).step_by(step as usize) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy, .. } if dy == step),
            "at {top}: {result:?}"
        );
    }
    let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
    export_scene("pinned-sidebar", &capture(end), &sheet, expected);
    assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
    for top in (0..end).step_by(step as usize).rev() {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::Shifted { dy } if dy == -(step as i32)),
            "return at {top}: {result:?}"
        );
    }
    assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
}

#[test]
fn pinned_column_keeps_observed_content_while_document_continues() {
    let (page, _) = pinned_heading_scene(246);
    let capture = |top: u32, pinned: bool| {
        let mut frame = viewport(&page, top);
        let side_top = if pinned { top.min(200) } else { top };
        for y in 0..HEIGHT {
            let dst = ((y * WIDTH + 336) * 4) as usize;
            let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
            let bytes = ((WIDTH - 336) * 4) as usize;
            frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
        }
        frame
    };
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &capture(0, true), SheetLimits::default()).unwrap();
    for top in (40..=200).step_by(40) {
        assert!(matches!(
            sheet.push(&capture(top, true)),
            Fold::GrewBottom { dy: 40, .. }
        ));
    }
    let observed_side_bottom = HEIGHT + 200;
    for top in [240, 280] {
        assert!(matches!(
            sheet.push(&capture(top, true)),
            Fold::GrewBottom { dy: 40, .. }
        ));
        let mut expected = page[..((HEIGHT + top) * WIDTH * 4) as usize].to_vec();
        for y in observed_side_bottom..HEIGHT + top {
            for x in 336..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                expected[i..i + 4].fill(255);
            }
        }
        assert_pixels(sheet.canvas(), &expected, 0, HEIGHT + top);
    }
    assert!(matches!(
        sheet.push(&capture(320, false)),
        Fold::GrewBottom { dy: 40, .. }
    ));
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 320) * WIDTH * 4) as usize],
        0,
        HEIGHT + 320,
    );
}

#[test]
fn sidebar_pinned_from_first_capture_is_not_repeated() {
    let (mut page, _) = pinned_heading_scene(246);
    for y in 500..PAGE_HEIGHT {
        for x in 340..WIDTH {
            let i = ((y * WIDTH + x) * 4) as usize;
            page[i..i + 4].fill(255);
        }
    }
    let capture = |top: u32| {
        let mut frame = viewport(&page, top);
        for y in 0..HEIGHT {
            let dst = ((y * WIDTH + 336) * 4) as usize;
            let bytes = ((WIDTH - 336) * 4) as usize;
            frame[dst..dst + bytes].copy_from_slice(&page[dst..dst + bytes]);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (40..=480).step_by(40) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 40, .. }),
            "{top}: {result:?}"
        );
    }
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 480) * WIDTH * 4) as usize],
        0,
        HEIGHT + 480,
    );
}

#[test]
fn sparse_sidebar_below_a_wide_heading_pins_as_a_complete_column() {
    for heading_height in [0, 480] {
        let (mut page, bar) = pinned_heading_scene(246);
        for y in 0..PAGE_HEIGHT {
            for x in 0..WIDTH {
                if y < heading_height || x >= 336 {
                    let i = ((y * WIDTH + x) * 4) as usize;
                    page[i..i + 4].fill(255);
                }
            }
        }
        if heading_height > 0 {
            scene_label(
                &mut page,
                32.,
                (heading_height / 2) as f32,
                "A generated document heading spanning the article and metadata columns",
            );
            assert!(
                (heading_height / 2..heading_height / 2 + 18).any(|y| {
                    (336..344).any(|x| {
                        let i = ((y * WIDTH + x) * 4) as usize;
                        page[i..i + 3] != [255; 3]
                    })
                }),
                "heading must cross the column gutter"
            );
        }
        for index in 0..5 {
            let y = heading_height + 24 + index * 108;
            scene_label(&mut page, 344., y as f32, &format!("Group {index}"));
            scene_label(
                &mut page,
                344.,
                (y + 20) as f32,
                &format!("Data {}", index * 17 + 13),
            );
        }
        if heading_height == 480 {
            page[..bar.len()].copy_from_slice(&bar);
        }
        let pin_at = heading_height + 80;
        let capture = |top: u32| {
            let mut frame = viewport(&page, top);
            let side_top = top.min(pin_at);
            for y in 0..HEIGHT {
                let dst = ((y * WIDTH + 336) * 4) as usize;
                let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
                let bytes = ((WIDTH - 336) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
            }
            if heading_height == 480 {
                frame[..bar.len()].copy_from_slice(&bar);
            }
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        assert!(
            sheet.sidebar.is_some(),
            "undetected sidebar below heading {heading_height}"
        );
        assert!(
            (312..344).contains(&sheet.sidebar.as_ref().unwrap().left()),
            "split inside sidebar below heading {heading_height}"
        );
        for top in (40..=720).step_by(40) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewBottom { dy: 40, .. }),
                "heading {heading_height}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 720) * WIDTH * 4) as usize];
        if heading_height == 480 {
            export_scene("sparse-sidebar", &capture(720), &sheet, expected);
        }
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 720);
    }
}

#[test]
fn slow_scroll_continues_when_pinned_sidebar_ends_near_capture_bottom() {
    let (mut page, _) = pinned_heading_scene(246);
    for y in 720..PAGE_HEIGHT {
        for x in 340..WIDTH {
            let i = ((y * WIDTH + x) * 4) as usize;
            page[i..i + 4].fill(255);
        }
    }
    for (dark, step) in [(false, 2u32), (true, 8)] {
        if dark {
            for pixel in page.as_chunks_mut::<4>().0 {
                for value in &mut pixel[..3] {
                    *value = 255 - *value;
                }
            }
        }
        let end = 112u32;
        let capture = |top: u32| {
            let mut frame = viewport(&page, top);
            let side_top = top.min(80);
            for y in 0..HEIGHT {
                let dst = ((y * WIDTH + 336) * 4) as usize;
                let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
                let bytes = ((WIDTH - 336) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
            }
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        let mut accepted_top = 0;
        let mut uncertain = 0;
        for top in (step..=end).step_by(step as usize) {
            let result = sheet.push(&capture(top));
            match result {
                Fold::GrewBottom { dy, .. } => {
                    assert_eq!(dy, top - accepted_top);
                    accepted_top = top;
                    uncertain = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    uncertain += 1;
                    assert!(uncertain <= 3, "slow scroll cannot remain stalled at {top}");
                }
                _ => panic!("step {step} at {top}: {result:?}"),
            }
            assert!(!sheet.has_incomplete_column());
        }
        assert_eq!(accepted_top, end);
        let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
        export_scene(
            if dark {
                "short-sidebar-margin-dark"
            } else {
                "short-sidebar-margin"
            },
            &capture(end),
            &sheet,
            expected,
        );
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
    }
}

#[test]
fn column_repaint_failure_does_not_block_body_and_recovers_without_losing_rows() {
    let (page, _) = pinned_heading_scene(246);
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &viewport(&page, 0), SheetLimits::default()).unwrap();
    assert!(matches!(
        sheet.push(&viewport(&page, 40)),
        Fold::GrewBottom { dy: 40, .. }
    ));
    let before = sheet.canvas().to_vec();
    for top in [80, 120] {
        let mut frame = viewport(&page, top);
        for y in 0..HEIGHT {
            for x in 336..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                let value = ((x * 71 + y * 193 + x * y * 17) % 251) as u8;
                frame[i..i + 3].fill(value);
            }
        }
        let result = sheet.push(&frame);
        assert!(
            matches!(result, Fold::GrewBottom { dy: 40, .. }),
            "{top}: {result:?}"
        );
        assert!(
            sheet.has_incomplete_column(),
            "repeated rejected column stays incomplete"
        );
        for y in 0..HEIGHT + top {
            let i = (y * WIDTH * 4) as usize;
            assert_eq!(&sheet.canvas()[i..i + 312 * 4], &page[i..i + 312 * 4]);
            if y < HEIGHT + 40 {
                assert_eq!(
                    &sheet.canvas()[i + 344 * 4..i + WIDTH as usize * 4],
                    &before[i + 344 * 4..i + WIDTH as usize * 4]
                );
            }
        }
    }
    let revision = sheet.top_revision();
    assert!(matches!(
        sheet.push(&viewport(&page, 120)),
        Fold::Shifted { dy: 0 }
    ));
    assert!(sheet.top_revision() > revision);
    assert!(!sheet.has_incomplete_column());
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 120) * WIDTH * 4) as usize],
        0,
        HEIGHT + 120,
    );
    let result = sheet.push(&viewport(&page, 160));
    assert!(
        matches!(result, Fold::GrewBottom { dy: 40, .. }),
        "{result:?}"
    );
    assert!(!sheet.has_incomplete_column());
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 160) * WIDTH * 4) as usize],
        0,
        HEIGHT + 160,
    );
}

#[test]
fn patterned_column_uses_gutter_padding_and_reports_unobserved_extent() {
    let (mut page, _) = pinned_heading_scene(246);
    let mut random = 17u32;
    for y in 0..PAGE_HEIGHT {
        for x in 340..WIDTH {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let value = 80 + (random % 150) as u8;
            let i = ((y * WIDTH + x) * 4) as usize;
            page[i..i + 3].fill(value);
        }
    }
    let capture = |top: u32, pin: bool| {
        let mut frame = viewport(&page, top);
        let side_top = if pin { top.min(80) } else { top };
        for y in 0..HEIGHT {
            let dst = ((y * WIDTH + 336) * 4) as usize;
            let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
            let bytes = ((WIDTH - 336) * 4) as usize;
            frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
        }
        frame
    };
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &capture(0, true), SheetLimits::default()).unwrap();
    for top in [40, 80, 120, 160] {
        let result = sheet.push(&capture(top, true));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 40, .. }),
            "{top}: {result:?}"
        );
        assert_eq!(sheet.has_incomplete_column(), top > 80);
        let left = sheet.sidebar.as_ref().unwrap().left();
        for y in 0..HEIGHT + top {
            let i = (y * WIDTH * 4) as usize;
            assert_eq!(&sheet.canvas()[i..i + left * 4], &page[i..i + left * 4]);
            let side = &sheet.canvas()[i + left * 4..i + WIDTH as usize * 4];
            if y < HEIGHT + 80 {
                assert_eq!(side, &page[i + left * 4..i + WIDTH as usize * 4]);
            } else {
                assert!(
                    side.as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| *pixel == [255; 4])
                );
            }
        }
    }
    assert!(matches!(
        sheet.push(&capture(200, false)),
        Fold::GrewBottom { dy: 40, .. }
    ));
    assert!(!sheet.has_incomplete_column());
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 200) * WIDTH * 4) as usize],
        0,
        HEIGHT + 200,
    );
}

#[test]
fn moving_outer_scrollbar_does_not_stall_the_body_or_repeat_pinned_column_text() {
    let (mut page, _) = pinned_heading_scene(246);
    for y in 720..PAGE_HEIGHT {
        for x in 340..WIDTH {
            let i = ((y * WIDTH + x) * 4) as usize;
            page[i..i + 4].fill(255);
        }
    }
    let capture = |top: u32| {
        let mut frame = viewport(&page, top);
        let side_top = top.min(80);
        for y in 0..HEIGHT {
            let dst = ((y * WIDTH + 336) * 4) as usize;
            let src = (((side_top + y) * WIDTH + 336) * 4) as usize;
            let bytes = ((WIDTH - 336) * 4) as usize;
            frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
            for x in WIDTH - 4..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                let value = if (24 + top / 3..104 + top / 3).contains(&y) {
                    120
                } else {
                    240
                };
                frame[i..i + 3].fill(value);
            }
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (16..=240).step_by(16) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 16, .. }),
            "{top}: {result:?}"
        );
    }
    let left = sheet.sidebar.as_ref().unwrap().left();
    for y in 0..HEIGHT + 240 {
        let i = (y * WIDTH * 4) as usize;
        assert_eq!(&sheet.canvas()[i..i + left * 4], &page[i..i + left * 4]);
        assert_eq!(
            &sheet.canvas()[i + left * 4..i + (WIDTH - 4) as usize * 4],
            &page[i + left * 4..i + (WIDTH - 4) as usize * 4]
        );
    }
}

#[test]
fn sidebar_padding_is_opaque_and_later_full_width_sections_survive() {
    for (background, contrasting, step) in [(255u8, false, 40u32), (0, true, 24)] {
        let (mut page, _) = pinned_heading_scene(246);
        for y in 720..PAGE_HEIGHT {
            for x in 336..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                page[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        // Independently rasterized article content replaces the two-column
        // layout, with both unchanged and contrasting section backgrounds.
        let full_width = document_base();
        let first = (1_000 * WIDTH * 4) as usize;
        page[first..].copy_from_slice(&full_width[first..]);
        scene_label(&mut page, 300., 1_002., "HHHHxxxxHHHH next section");
        if contrasting {
            for pixel in page[first..].as_chunks_mut::<4>().0 {
                if pixel[..3] == [255; 3] {
                    pixel[..3].fill(218);
                }
            }
        }
        if background != 255 {
            for pixel in page.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = 255 - *channel;
                }
            }
        }
        let (_, mut chrome) = pinned_heading_scene(226);
        scene_label(&mut chrome, 348., 10., "Fixed tools");
        page[..chrome.len()].copy_from_slice(&chrome);
        if background != 255 {
            for pixel in chrome.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = 255 - *channel;
                }
            }
            page[..chrome.len()].copy_from_slice(&chrome);
        }
        let capture = |top: u32| {
            let mut frame = viewport(&page, top);
            for y in 0..HEIGHT {
                if top + y < 1_000 {
                    let dst = ((y * WIDTH + 336) * 4) as usize;
                    let src = (((top.min(80) + y) * WIDTH + 336) * 4) as usize;
                    let bytes = ((WIDTH - 336) * 4) as usize;
                    frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
                }
                for x in WIDTH - 4..WIDTH {
                    let i = ((y * WIDTH + x) * 4) as usize;
                    let value = if (24 + top / 3..104 + top / 3).contains(&y) {
                        120
                    } else {
                        240
                    };
                    frame[i..i + 3].fill(value);
                }
            }
            frame[..chrome.len()].copy_from_slice(&chrome);
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        let end = 1_120u32.div_ceil(step) * step;
        let mut accepted = 0;
        let mut unclear = 0;
        for top in (step..=end).step_by(step as usize) {
            let result = sheet.push(&capture(top));
            match result {
                Fold::GrewBottom { dy, .. } => {
                    assert_eq!(dy, top - accepted);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "stalled at {top}");
                }
                _ => panic!("background {background}, step {step}, at {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, end);
        assert!(
            sheet
                .canvas()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255),
            "transparent padding"
        );
        let assert_content = |sheet: &ScrollSheet| {
            for y in 0..HEIGHT + end {
                let i = (y * WIDTH * 4) as usize;
                let bytes = (WIDTH - 4) as usize * 4;
                assert_eq!(
                    &sheet.canvas()[i..i + bytes],
                    &page[i..i + bytes],
                    "background {background}, contrast {contrasting}, step {step}, row {y}"
                );
            }
        };
        assert_content(&sheet);
        for top in (0..end).step_by(step.max(80) as usize).rev() {
            let result = sheet.push(&capture(top));
            match result {
                Fold::Shifted { dy } => {
                    assert_eq!(dy, top as i32 - accepted as i32);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "return stalled at {top}");
                }
                _ => panic!("return {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, 0);
        assert_content(&sheet);
        export_scene(
            if background == 255 {
                "sidebar-to-full-width"
            } else {
                "sidebar-to-full-width-dark"
            },
            &capture(end),
            &sheet,
            &page[..((HEIGHT + end) * WIDTH * 4) as usize],
        );
    }
}

#[test]
fn a_pinned_panel_resuming_with_a_wide_rule_is_not_new_document_content() {
    for (footer, dark, step) in [(false, false, 24u32), (true, true, 80)] {
        let (mut page, _) = pinned_heading_scene(246);
        for y in 980..PAGE_HEIGHT {
            for x in 326..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                page[i..i + 4].fill(255);
            }
        }
        for x in 326..WIDTH {
            let i = ((300 * WIDTH + x) * 4) as usize;
            page[i..i + 3].fill(200);
        }
        if footer {
            let full_width = document_base();
            let first = (1_400 * WIDTH * 4) as usize;
            page[first..].copy_from_slice(&full_width[first..]);
            for pixel in page[first..].as_chunks_mut::<4>().0 {
                if pixel[..3] == [255; 3] {
                    pixel[..3].fill(218);
                }
            }
        }
        let (_, mut chrome) = pinned_heading_scene(226);
        scene_label(&mut chrome, 348., 10., "Panel controls");
        page[..chrome.len()].copy_from_slice(&chrome);
        if dark {
            for pixel in page.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = 255 - *channel;
                }
            }
            let bytes = chrome.len();
            chrome.copy_from_slice(&page[..bytes]);
        }
        let capture = |top: u32| {
            let side_top = if top > 600 { top - 400 } else { top.min(200) };
            let mut frame = viewport(&page, top);
            for y in 0..HEIGHT {
                if footer && top + y >= 1_400 {
                    continue;
                }
                let dst = ((y * WIDTH + 326) * 4) as usize;
                let src = (((side_top + y) * WIDTH + 326) * 4) as usize;
                let bytes = ((WIDTH - 326) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
            }
            frame[..chrome.len()].copy_from_slice(&chrome);
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        assert!(sheet.sidebar.is_some());
        let end = 1_200u32.div_ceil(step) * step;
        let mut accepted = 0;
        let mut unclear = 0;
        for top in (step..=end).step_by(step as usize) {
            match sheet.push(&capture(top)) {
                Fold::GrewBottom { dy, .. } => {
                    assert_eq!(dy, top - accepted);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "stalled at {top}");
                }
                result => panic!("footer {footer}, dark {dark}, step {step}, at {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, end);
        let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
        for top in (0..end).step_by(step.max(80) as usize).rev() {
            match sheet.push(&capture(top)) {
                Fold::Shifted { dy } => {
                    assert_eq!(dy, top as i32 - accepted as i32);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "return stalled at {top}");
                }
                result => panic!("return {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, 0);
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
        if !dark {
            export_scene("resumed-panel", &capture(end), &sheet, expected);
        }
    }
}

#[test]
fn a_resumed_column_recovers_after_partial_repaints_without_splitting_its_tail() {
    for (step, bad_frames, dark) in [(24u32, 1u32, false), (80, 3, true)] {
        let (mut page, _) = pinned_heading_scene(246);
        for y in 980..PAGE_HEIGHT {
            for x in 326..WIDTH {
                let at = ((y * WIDTH + x) * 4) as usize;
                page[at..at + 4].fill(255);
            }
        }
        for y in [300, 810] {
            for x in 326..WIDTH {
                let at = ((y * WIDTH + x) * 4) as usize;
                page[at..at + 3].fill(200);
            }
        }
        let (_, mut chrome) = pinned_heading_scene(226);
        scene_label(&mut chrome, 348., 10., "Panel controls");
        page[..chrome.len()].copy_from_slice(&chrome);
        if dark {
            for pixel in page.as_chunks_mut::<4>().0 {
                for value in &mut pixel[..3] {
                    *value = 255 - *value;
                }
            }
            let bytes = chrome.len();
            chrome.copy_from_slice(&page[..bytes]);
        }
        let first_bad = (600 / step + 1) * step;
        let bad = first_bad..first_bad + bad_frames * step;
        let capture = |top: u32| {
            let side_top = if top > 600 { top - 400 } else { top.min(200) };
            let mut frame = viewport(&page, top);
            for y in 0..HEIGHT {
                let dst = ((y * WIDTH + 326) * 4) as usize;
                let src = (((side_top + y) * WIDTH + 326) * 4) as usize;
                let bytes = ((WIDTH - 326) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
                if bad.contains(&top) && (48..440).contains(&y) {
                    for (x, pixel) in frame[dst..dst + bytes]
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .enumerate()
                    {
                        pixel[..3].fill(((y as usize * 17 + x * 37) % 160) as u8);
                    }
                }
            }
            frame[..chrome.len()].copy_from_slice(&chrome);
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        let end = 1_200u32.div_ceil(step) * step;
        let mut accepted = 0;
        let mut unclear = 0;
        for top in (step..=end).step_by(step as usize) {
            match sheet.push(&capture(top)) {
                Fold::GrewBottom { dy, .. } => {
                    assert_eq!(dy, top - accepted);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "body stalled at {top}");
                }
                result => {
                    panic!("step {step}, bad {bad_frames}, dark {dark}, at {top}: {result:?}")
                }
            }
        }
        assert_eq!(accepted, end);
        let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
        if !dark {
            export_scene("repainted-column", &capture(end), &sheet, expected);
        }
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
        for top in (0..end).step_by(step.max(80) as usize).rev() {
            match sheet.push(&capture(top)) {
                Fold::Shifted { dy } => {
                    assert_eq!(dy, top as i32 - accepted as i32);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "return stalled at {top}");
                }
                result => panic!("return {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, 0);
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
    }
}

#[test]
fn a_document_rule_above_resumed_metadata_does_not_duplicate_the_panel() {
    for (step, dark, footer) in [(24u32, false, true), (80, true, false)] {
        let (mut page, _) = pinned_heading_scene(246);
        for y in 980..PAGE_HEIGHT {
            for x in 326..WIDTH {
                let at = ((y * WIDTH + x) * 4) as usize;
                page[at..at + 4].fill(255);
            }
        }
        if footer {
            let full_width = document_base();
            let first = (1_400 * WIDTH * 4) as usize;
            page[first..].copy_from_slice(&full_width[first..]);
            scene_label(&mut page, 300., 1_402., "Next full-width section");
        }
        let (_, mut chrome) = pinned_heading_scene(226);
        page[..chrome.len()].copy_from_slice(&chrome);
        if dark {
            for pixel in page.as_chunks_mut::<4>().0 {
                for value in &mut pixel[..3] {
                    *value = 255 - *value;
                }
            }
            let bytes = chrome.len();
            chrome.copy_from_slice(&page[..bytes]);
        }
        let capture = |top: u32| {
            let side_top = if top > 600 { top - 400 } else { top.min(200) };
            let mut frame = viewport(&page, top);
            for y in 0..HEIGHT {
                if footer && top + y >= 1_400 {
                    continue;
                }
                let dst = ((y * WIDTH + 326) * 4) as usize;
                let src = (((side_top + y) * WIDTH + 326) * 4) as usize;
                let bytes = ((WIDTH - 326) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
                // A wide document decoration can overlap a separately
                // positioned panel without owning its metadata below.
                let rule = if dark { 900..904 } else { 900..901 };
                if top >= 600 && rule.contains(&(top + y)) {
                    for x in 321..WIDTH {
                        let at = ((y * WIDTH + x) * 4) as usize;
                        frame[at..at + 3].fill(if dark { 75 } else { 180 });
                    }
                }
            }
            frame[..chrome.len()].copy_from_slice(&chrome);
            frame
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        let end = 1_200u32.div_ceil(step) * step;
        let mut accepted = 0;
        let mut unclear = 0;
        for top in (step..=end).step_by(step as usize) {
            match sheet.push(&capture(top)) {
                Fold::GrewBottom { dy, .. } => {
                    assert_eq!(dy, top - accepted);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "body stalled at {top}");
                }
                result => {
                    panic!("step {step}, dark {dark}, footer {footer}, at {top}: {result:?}")
                }
            }
        }
        assert_eq!(accepted, end);
        let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
        if !dark {
            export_scene(
                "column-crossing-rule-footer",
                &capture(end),
                &sheet,
                expected,
            );
        }
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
        for top in (0..end).step_by(step.max(80) as usize).rev() {
            match sheet.push(&capture(top)) {
                Fold::Shifted { dy } => {
                    assert_eq!(dy, top as i32 - accepted as i32);
                    accepted = top;
                    unclear = 0;
                }
                Fold::Refused(super::Refusal::Unclear) => {
                    unclear += 1;
                    assert!(unclear <= 3, "return stalled at {top}");
                }
                result => panic!("return {top}: {result:?}"),
            }
        }
        assert_eq!(accepted, 0);
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
    }
}

/// A compositor frame over a captured window, with antialiased rounded corners
/// and an opaque desktop behind it. Only perimeter rows/columns (`< margin` or
/// `>= dim - margin`) can differ from `fill = 1.0`, skipping interior no-ops.
fn rounded_window(pixels: &mut [u8], width: u32, radius: f32, border: [u8; 3]) {
    let w = width as usize;
    let height = pixels.len() / (w * 4);
    let margin = (radius.ceil() as usize + 3).min(w / 2);
    for y in 0..height {
        let py = y as f32 + 0.5;
        let cy = py.clamp(radius, height as f32 - radius);
        let y_edge = y < margin || y + margin >= height;
        let x_spans: &[(usize, usize)] = if y_edge {
            &[(0, w)]
        } else {
            &[(0, margin), (w - margin, w)]
        };
        for &(x0, x1) in x_spans {
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let cx = px.clamp(radius, width as f32 - radius);
                let distance = radius - (px - cx).hypot(py - cy);
                if distance >= 3. {
                    continue;
                }
                let inside = distance.clamp(0., 1.);
                let fill = (distance - 2.).clamp(0., 1.);
                let at = (y * w + x) * 4;
                for (channel, edge) in pixels[at..at + 3].iter_mut().zip(border) {
                    let window = f32::from(*channel) * fill + f32::from(edge) * (1. - fill);
                    *channel = (window * inside + 90. * (1. - inside)).round() as u8;
                }
            }
        }
    }
}

#[test]
fn rounded_window_preserves_pinned_column_glyphs_near_the_bottom_edge() {
    for (dark, step, radius, border) in [
        (false, 24, 16., [20, 130, 230]),
        (true, 80, 32., [215, 215, 215]),
    ] {
        check_rounded_window_column(step, radius, dark, border);
    }
}

fn check_rounded_window_column(step: u32, radius: f32, dark: bool, border: [u8; 3]) {
    let (mut page, mut chrome) = pinned_heading_scene(226);
    for y in 720..1_400 {
        for x in 326..WIDTH {
            let at = ((y * WIDTH + x) * 4) as usize;
            page[at..at + 4].fill(255);
        }
    }
    for y in (734..826).step_by(18) {
        scene_label(&mut page, 344.25, y as f32 + 0.25, &format!("Tail {y}"));
    }
    let full_width = document_base();
    let first = (1_400 * WIDTH * 4) as usize;
    page[first..].copy_from_slice(&full_width[first..]);
    scene_label(&mut page, 300., 1_402., "Wide footer content");
    page[..chrome.len()].copy_from_slice(&chrome);
    if dark {
        for pixels in [&mut page, &mut chrome] {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = 255 - *channel;
                }
            }
        }
    }
    let capture = |top: u32| {
        let mut frame = viewport(&page, top);
        for y in 0..HEIGHT {
            if top + y < 1_400 {
                let dst = ((y * WIDTH + 326) * 4) as usize;
                let src = (((top.min(200) + y) * WIDTH + 326) * 4) as usize;
                let bytes = ((WIDTH - 326) * 4) as usize;
                frame[dst..dst + bytes].copy_from_slice(&page[src..src + bytes]);
            }
        }
        frame[..chrome.len()].copy_from_slice(&chrome);
        rounded_window(&mut frame, WIDTH, radius, border);
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    let end = 1_200u32;
    let mut accepted = 0;
    let mut unclear = 0;
    for top in (step..end).step_by(step as usize).chain([end]) {
        let result = sheet.push(&capture(top));
        for y in HEIGHT - HEIGHT / 8..HEIGHT - 3 {
            for x in 80..WIDTH - 80 {
                assert!(
                    !sheet.floating.frame_edge.mask()[(y * WIDTH + x) as usize],
                    "frame mask covers document at {x},{y}, scroll {top}"
                );
            }
        }
        match result {
            Fold::GrewBottom { dy, .. } => {
                assert_eq!(dy, top - accepted);
                accepted = top;
                unclear = 0;
            }
            Fold::Refused(super::Refusal::Unclear) => {
                unclear += 1;
                assert!(unclear <= 3, "stalled at {top}");
            }
            result => panic!("step {step}, radius {radius}, dark {dark}, at {top}: {result:?}"),
        }
    }
    assert_eq!(accepted, end);
    let mut expected = page[..((HEIGHT + end) * WIDTH * 4) as usize].to_vec();
    rounded_window(&mut expected, WIDTH, radius, border);
    export_scene(
        &format!("rounded-window-{step}-{dark}"),
        &capture(end),
        &sheet,
        &expected,
    );
    assert_window_pixels(sheet.canvas(), &expected);
    for top in (0..end).step_by(step.max(80) as usize).rev() {
        let result = sheet.push(&capture(top));
        match result {
            Fold::Shifted { dy } => {
                assert_eq!(dy, top as i32 - accepted as i32);
                accepted = top;
                unclear = 0;
            }
            Fold::Refused(super::Refusal::Unclear) => {
                unclear += 1;
                assert!(unclear <= 3, "return stalled at {top}");
            }
            result => {
                panic!("return step {step}, radius {radius}, dark {dark}, at {top}: {result:?}")
            }
        }
    }
    assert_eq!(accepted, 0);
    assert_window_pixels(sheet.canvas(), &expected);
}

fn assert_window_pixels(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0)
        .enumerate()
    {
        let x = index % WIDTH as usize;
        // The two antialiased outer rails share the matcher's color tolerance.
        // Every document pixel must still match the independent source exactly.
        let tolerance = if x < 3 || x >= WIDTH as usize - 3 {
            3
        } else {
            0
        };
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(*b) <= tolerance),
            "pixel {x},{}: {actual:?}, expected {expected:?}",
            index / WIDTH as usize
        );
    }
}

#[test]
fn rounded_window_preserves_content_when_captured_from_the_bottom() {
    let mut page = document();
    let (_, chrome) = pinned_heading_scene(226);
    page[..chrome.len()].copy_from_slice(&chrome);
    let end = 640u32;
    let capture = |top| {
        let mut frame = viewport(&page, top);
        frame[..chrome.len()].copy_from_slice(&chrome);
        rounded_window(&mut frame, WIDTH, 24., [20, 130, 230]);
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(end), SheetLimits::default()).unwrap();
    assert!(sheet.sidebar.is_none());
    for top in (0..end).step_by(40).rev() {
        assert!(
            matches!(sheet.push(&capture(top)), Fold::GrewTop { dy: 40, .. }),
            "at {top}"
        );
    }
    let mut expected = page[..((HEIGHT + end) * WIDTH * 4) as usize].to_vec();
    rounded_window(&mut expected, WIDTH, 24., [20, 130, 230]);
    export_scene("rounded-window-upward", &capture(0), &sheet, &expected);
    assert_window_pixels(sheet.canvas(), &expected);
}

#[test]
fn moving_colored_rules_are_not_a_window_frame() {
    let mut page = document();
    for y in (HEIGHT - 3..PAGE_HEIGHT).step_by(97) {
        for pixel in page[(y * WIDTH * 4) as usize..((y + 1) * WIDTH * 4) as usize]
            .as_chunks_mut::<4>()
            .0
        {
            pixel.copy_from_slice(&[20, 130, 230, 255]);
        }
    }
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &viewport(&page, 0), SheetLimits::default()).unwrap();
    for top in (40..=640).step_by(40) {
        assert!(matches!(
            sheet.push(&viewport(&page, top)),
            Fold::GrewBottom { dy: 40, .. }
        ));
        assert!(sheet.floating.frame_edge.is_empty());
    }
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 640) * WIDTH * 4) as usize],
        0,
        HEIGHT + 640,
    );
}

#[test]
fn removed_window_frame_does_not_mask_later_document_content() {
    let mut page = document();
    let (_, chrome) = pinned_heading_scene(226);
    page[..chrome.len()].copy_from_slice(&chrome);
    let capture = |top| {
        let mut frame = viewport(&page, top);
        frame[..chrome.len()].copy_from_slice(&chrome);
        if top <= 240 {
            rounded_window(&mut frame, WIDTH, 24., [20, 130, 230]);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (40..=640).step_by(40) {
        assert!(
            matches!(sheet.push(&capture(top)), Fold::GrewBottom { dy: 40, .. }),
            "at {top}"
        );
        if top > 240 {
            assert!(sheet.floating.frame_edge.is_empty());
        }
    }
    // Older observations retain their captured appearance. Newly revealed
    // document rows must not inherit the removed decoration's mask.
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 640) * WIDTH * 4) as usize],
        HEIGHT + 240,
        HEIGHT + 640,
    );
}

#[test]
fn independent_column_exclusion_preserves_body_and_later_section_controls() {
    let page = document_base();
    let row = WIDTH as usize * 4;
    for (step, opacity) in [(16, 1.), (40, 0.85)] {
        let capture = |top| {
            let mut frame = viewport(page, top);
            control(&mut frame, (180.25, 318.75), opacity);
            control(&mut frame, (399.25, 510.75), opacity);
            frame
        };
        let mut canvas = capture(0);
        let mut floating = super::floating::FloatingOverlays::new(&canvas, WIDTH);
        for top in (step..=320).step_by(step as usize) {
            let frame = capture(top);
            let retained = 0..canvas.len() / row;
            canvas.resize((top + HEIGHT) as usize * row, 0);
            floating.observe_frame_edge(&frame, step as i32);
            floating.observe(&frame, (WIDTH, HEIGHT), Some((321, 0..400)));
            floating.composite(
                &mut canvas,
                &frame,
                WIDTH as usize,
                top as usize,
                0..HEIGHT as usize,
                retained,
            );
        }
        let expected = &page[..(320 + HEIGHT) as usize * row];
        assert_pixels(&canvas, expected, 0, 320 + HEIGHT);
        for top in (0..320).step_by(step as usize).rev() {
            let frame = capture(top);
            floating.observe_frame_edge(&frame, -(step as i32));
            floating.observe(&frame, (WIDTH, HEIGHT), Some((321, 0..400)));
            let retained = 0..canvas.len() / row;
            floating.composite(
                &mut canvas,
                &frame,
                WIDTH as usize,
                top as usize,
                0..HEIGHT as usize,
                retained,
            );
        }
        assert_pixels(&canvas, expected, 0, 320 + HEIGHT);
    }
}

#[test]
fn lateral_layout_shifts_preserve_the_complete_document() {
    let page = document_base();
    for (direction, amount) in [(-1, 1), (1, 8)] {
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &viewport(page, 0), SheetLimits::default()).unwrap();
        for top in (24..=288).step_by(24) {
            let offset = match top {
                0..=48 => 0,
                49..=144 => amount * direction,
                145..=216 => -(amount - 1).max(1) * direction,
                _ => 0,
            };
            let frame = shifted_scene(&viewport(page, top), offset);
            let outcome = sheet.push(&frame);
            assert!(
                matches!(outcome, Fold::GrewBottom { growth: 24, .. }),
                "top={top} offset={offset}: {outcome:?}"
            );
            assert_eq!(sheet.height(), top + HEIGHT);
        }
        assert_eq!(
            sheet.canvas(),
            &page[..((288 + HEIGHT) * WIDTH * 4) as usize],
        );
    }
}

#[test]
fn lateral_correction_preserves_prepend_and_stationary_alignment() {
    let page = document_base();
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &viewport(page, 240), SheetLimits::default()).unwrap();
    let shifted = shifted_scene(&viewport(page, 240), 8);
    assert!(matches!(
        sheet.push(&shifted),
        Fold::Still | Fold::Unchanged
    ));
    for top in (0..240).step_by(24).rev() {
        let outcome = sheet.push(&shifted_scene(&viewport(page, top), 8));
        assert!(
            matches!(outcome, Fold::GrewTop { growth: 24, .. }),
            "top={top}: {outcome:?}"
        );
    }
    assert_eq!(sheet.height(), 240 + HEIGHT);
    assert_eq!(
        sheet.canvas(),
        &page[..((240 + HEIGHT) * WIDTH * 4) as usize]
    );
}

#[test]
fn clipped_lateral_layout_retains_the_verified_canvas() {
    let mut page = document();
    // Content reaches the left capture edge; no unseen background may replace it.
    for y in 0..PAGE_HEIGHT {
        let at = (y * WIDTH * 4) as usize;
        page[at..at + 12].fill((y % 173) as u8);
    }
    let seed = viewport(&page, 0);
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &seed, SheetLimits::default()).unwrap();
    for top in [24, 48] {
        let outcome = sheet.push(&shifted_scene(&viewport(&page, top), 8));
        assert_eq!(outcome, Fold::Refused(super::Refusal::LayoutShift));
        assert_eq!(sheet.canvas(), seed);
        assert_eq!(sheet.height(), HEIGHT);
    }
    assert!(matches!(
        sheet.push(&viewport(&page, 72)),
        Fold::GrewBottom { growth: 72, .. }
    ));
    assert_eq!(
        sheet.canvas(),
        &page[..((72 + HEIGHT) * WIDTH * 4) as usize]
    );
}

fn shifted_scene(frame: &[u8], offset: i32) -> Vec<u8> {
    if offset == 0 {
        return frame.to_vec();
    }
    let mut shifted = vec![255; frame.len()];
    let w = WIDTH as usize;
    for y in 0..HEIGHT as usize {
        let row_off = y * w * 4;
        if offset > 0 {
            let dx = offset as usize;
            shifted[row_off + dx * 4..row_off + w * 4]
                .copy_from_slice(&frame[row_off..row_off + (w - dx) * 4]);
        } else {
            let dx = (-offset) as usize;
            shifted[row_off..row_off + (w - dx) * 4]
                .copy_from_slice(&frame[row_off + dx * 4..row_off + w * 4]);
        }
    }
    shifted
}

#[test]
fn lateral_correction_keeps_dark_background_and_refuses_fixed_regions() {
    let mut page = document();
    for pixel in page.as_chunks_mut::<4>().0 {
        for channel in &mut pixel[..3] {
            *channel = 255 - *channel;
        }
    }
    let frame = |top, dx| {
        let mut frame = shifted_scene(&viewport(&page, top), dx);
        for row in frame.as_chunks_mut::<{ WIDTH as usize * 4 }>().0 {
            if dx > 0 {
                for pixel in row[..dx as usize * 4].as_chunks_mut::<4>().0 {
                    pixel[..3].fill(0);
                }
            }
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &frame(0, 0), SheetLimits::default()).unwrap();
    for top in [24, 48, 72] {
        assert!(matches!(
            sheet.push(&frame(top, 8)),
            Fold::GrewBottom { growth: 24, .. }
        ));
    }
    assert_eq!(
        sheet.canvas(),
        &page[..((72 + HEIGHT) * WIDTH * 4) as usize]
    );

    let page = document_base();
    let stationary = |top, dx| {
        let mut frame = shifted_scene(&viewport(page, top), dx);
        // A fixed text strip does not share the translated document's coordinates.
        let prefix = (WIDTH * 120 * 4) as usize;
        frame[..prefix].copy_from_slice(&page[..prefix]);
        frame
    };
    let seed = stationary(0, 0);
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &seed, SheetLimits::default()).unwrap();
    let result = sheet.push(&stationary(24, 8));
    assert_eq!(result, Fold::Refused(super::Refusal::LayoutShift));
    assert_eq!(sheet.canvas(), seed);
}

fn fractional_viewport(page: &[u8], top_f32: f32) -> Vec<u8> {
    let row_bytes = (WIDTH * 4) as usize;
    let mut out = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for j in 0..HEIGHT {
        let y = (top_f32 + j as f32).max(0.0);
        let y0 = (y.floor() as u32).min(PAGE_HEIGHT - 1);
        let y1 = (y0 + 1).min(PAGE_HEIGHT - 1);
        let frac = y - y.floor();
        let src0 = &page[y0 as usize * row_bytes..(y0 as usize + 1) * row_bytes];
        let src1 = &page[y1 as usize * row_bytes..(y1 as usize + 1) * row_bytes];
        let dst = &mut out[j as usize * row_bytes..(j as usize + 1) * row_bytes];
        if frac <= 1e-6 {
            dst.copy_from_slice(src0);
        } else {
            for ((out_b, &a), &b) in dst.iter_mut().zip(src0.iter()).zip(src1.iter()) {
                *out_b = (f32::from(a) * (1.0 - frac) + f32::from(b) * frac).round() as u8;
            }
        }
    }
    out
}

#[test]
fn fractional_downward_scroll_folds_cleanly_without_seam_drift() {
    let page = document();
    for (step, steps, expected_scroll) in [
        (12.5_f32, 20usize, 250u32),
        (14.25, 20, 285),
        (18.25, 20, 365),
        (40.0 / 3.0, 15, 200),
        (13.6, 25, 340),
    ] {
        let mut sheet = ScrollSheet::new(
            WIDTH,
            HEIGHT,
            &fractional_viewport(&page, 0.0),
            SheetLimits::default(),
        )
        .unwrap();
        for i in 1..=steps {
            let top = i as f32 * step;
            let verdict = sheet.push(&fractional_viewport(&page, top));
            assert!(
                matches!(verdict, Fold::GrewBottom { .. }),
                "step={step} i={i} top={top}: {verdict:?}"
            );
            if (top - top.round()).abs() <= 1e-4 {
                let top_int = top.round() as u32;
                assert_eq!(sheet.height(), HEIGHT + top_int, "step={step} top={top}");
                assert_eq!(sheet.viewport_span(), (top_int, HEIGHT));
                assert_pixels(
                    sheet.canvas(),
                    &page[..((HEIGHT + top_int) * WIDTH * 4) as usize],
                    top_int + HEIGHT / 4,
                    HEIGHT + top_int,
                );
            }
        }
        assert_eq!(sheet.height(), HEIGHT + expected_scroll, "step={step}");
        let expected = &page[..((HEIGHT + expected_scroll) * WIDTH * 4) as usize];
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT / 4);
        assert_pixels(
            sheet.canvas(),
            expected,
            expected_scroll + HEIGHT / 4,
            HEIGHT + expected_scroll,
        );
    }

    // Starting at a non-zero fractional document phase (`0.75` -> `13.25` -> `25.75` ...)
    // exercises integer-boundary phase crossings (`0.75 -> 0.25`) without drift.
    let mut non_zero_phase = ScrollSheet::new(
        WIDTH,
        HEIGHT,
        &fractional_viewport(&page, 0.75),
        SheetLimits::default(),
    )
    .unwrap();
    for i in 1..=20 {
        let top = 0.75 + i as f32 * 12.5;
        let verdict = non_zero_phase.push(&fractional_viewport(&page, top));
        assert!(
            matches!(verdict, Fold::GrewBottom { .. }),
            "phase=0.75 i={i} top={top}: {verdict:?}"
        );
    }
    assert_eq!(non_zero_phase.height(), HEIGHT + 250);
}

#[test]
fn fractional_two_column_synchronized_scroll_stays_aligned() {
    let (page, _) = pinned_heading_scene(246);
    let mut sheet = ScrollSheet::new(
        WIDTH,
        HEIGHT,
        &fractional_viewport(&page, 64.0),
        SheetLimits::default(),
    )
    .unwrap();
    assert!(sheet.sidebar.is_some());
    for i in 1..=16 {
        let top = 64.0 + i as f32 * 12.5;
        let verdict = sheet.push(&fractional_viewport(&page, top));
        assert!(
            matches!(verdict, Fold::GrewBottom { .. }),
            "two-col i={i} top={top}: {verdict:?}"
        );
    }
    assert_eq!(sheet.height(), HEIGHT + 200);
    assert!(!sheet.has_incomplete_column());
    let expected = &page[(64 * WIDTH * 4) as usize..((64 + HEIGHT + 200) * WIDTH * 4) as usize];
    assert_pixels(sheet.canvas(), expected, 200 + HEIGHT / 4, HEIGHT + 200);
}

#[test]
fn fractional_upward_prepend_and_direction_reversal_preserve_exact_height() {
    let page = document();
    let mut sheet = ScrollSheet::new(
        WIDTH,
        HEIGHT,
        &fractional_viewport(&page, 200.0),
        SheetLimits::default(),
    )
    .unwrap();
    for i in 1..=16 {
        let top = 200.0 - i as f32 * 12.5;
        let verdict = sheet.push(&fractional_viewport(&page, top));
        assert!(
            matches!(verdict, Fold::GrewTop { .. }),
            "upward i={i} top={top}: {verdict:?}"
        );
    }
    assert_eq!(sheet.height(), HEIGHT + 200);
    assert_eq!(sheet.viewport_span(), (0, HEIGHT));
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 200) * WIDTH * 4) as usize],
        0,
        HEIGHT * 3 / 4,
    );

    // Reverse back down over the captured extent: no phantom growth, and
    // landing back on `top = 200.0` aligns pixel-for-pixel with `page`.
    for i in 1..=16 {
        let top = i as f32 * 12.5;
        let verdict = sheet.push(&fractional_viewport(&page, top));
        assert!(
            matches!(verdict, Fold::Shifted { .. }),
            "return i={i} top={top}: {verdict:?}"
        );
        assert_eq!(sheet.height(), HEIGHT + 200);
    }
    assert_eq!(sheet.viewport_span(), (200, HEIGHT));
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 200) * WIDTH * 4) as usize],
        200 + HEIGHT / 4,
        HEIGHT + 200,
    );
}

#[test]
fn sub_half_pixel_micro_scroll_on_rasterized_text_accumulates_to_exact_extent() {
    let page = document();
    let mut sheet = ScrollSheet::new(
        WIDTH,
        HEIGHT,
        &fractional_viewport(&page, 0.0),
        SheetLimits::default(),
    )
    .unwrap();
    assert_eq!(sheet.push(&fractional_viewport(&page, 0.3)), Fold::Still);
    assert_eq!(sheet.captured_extent(), 0);
    for i in 2..=20 {
        let top = i as f32 * 0.3;
        let verdict = sheet.push(&fractional_viewport(&page, top));
        assert!(
            matches!(verdict, Fold::Still | Fold::GrewBottom { .. }),
            "micro i={i} top={top}: {verdict:?}"
        );
    }
    assert_eq!(sheet.captured_extent(), 6);
    assert_eq!(sheet.height(), HEIGHT + 6);
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 6) * WIDTH * 4) as usize],
        6 + HEIGHT / 4,
        HEIGHT + 6,
    );
}
