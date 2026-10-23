//! Pixel regressions over rasterized, independently generated document scenes.

use super::{Fold, ScrollSheet, SheetLimits};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 640;
const PAGE_HEIGHT: u32 = 2_400;

fn document() -> Vec<u8> {
    let mut page = vec![255; (WIDTH * PAGE_HEIGHT * 4) as usize];
    let font =
        FontRef::try_from_slice(include_bytes!("../../../assets/DejaVuSans-Bold.ttf")).unwrap();
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
                    if (0..WIDTH as i32).contains(&px) && (0..PAGE_HEIGHT as i32).contains(&py) {
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
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
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
    let mismatches = actual[range.clone()]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected[range].as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize + first_row as usize))
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first {:?}",
        mismatches.len(),
        mismatches.first()
    );
}

#[test]
fn pale_control_at_top_edge_does_not_repeat_over_text() {
    let page = document();
    let capture = |top| {
        let mut rgba = viewport(&page, top);
        control(&mut rgba, (239.25, 22.75), 1.);
        rgba
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (8..=400).step_by(8) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 8, .. }),
            "{top}: {result:?}"
        );
    }
    let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
    // The first viewport's very top is never exposed again on a downward walk.
    assert_pixels(sheet.canvas(), expected, 64, HEIGHT + 400);
}

#[test]
fn pale_control_over_document_whitespace_does_not_repeat() {
    let page = document();
    let capture = |top| {
        let mut rgba = viewport(&page, top);
        control(&mut rgba, (239.25, 318.75), 1.);
        rgba
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (8..=400).step_by(8) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 8, .. }),
            "{top}: {result:?}"
        );
    }
    let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
    assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400);
}

#[test]
fn bottom_control_does_not_accumulate_and_exposed_tail_recovers() {
    let page = document();
    for step in [8, 40] {
        let capture = |top| {
            let mut rgba = viewport(&page, top);
            control(&mut rgba, (239.25, HEIGHT as f32 - 22.75), 1.);
            rgba
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
        for top in (step..=400).step_by(step as usize) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewBottom { .. }),
                "step {step}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
        // The current bottommost occlusion has never been seen unobstructed.
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400 - 80);
        assert!(matches!(
            sheet.push(&viewport(&page, 408)),
            Fold::GrewBottom { .. }
        ));
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + 408) * WIDTH * 4) as usize],
            0,
            HEIGHT + 408,
        );
    }
}

#[test]
fn bottom_control_does_not_repeat_when_capturing_upward() {
    let page = document();
    for (gap, opacity, step) in [
        (22.75, 1., 8),
        (22.75, 1., 40),
        (12.75, 0.85, 8),
        (44.75, 0.85, 40),
        (4.75, 1., 8),
    ] {
        let capture = |top| {
            let mut rgba = viewport(&page, top);
            control(&mut rgba, (239.25, HEIGHT as f32 - gap), opacity);
            rgba
        };
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(400), SheetLimits::default()).unwrap();
        for top in (0..400).step_by(step as usize).rev() {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewTop { .. }),
                "step {step}, top {top}: {result:?}"
            );
        }
        let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
        export_scene(
            &format!("up-{step}-{gap}-{opacity}"),
            &capture(400),
            &sheet,
            expected,
        );
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400 - 80);
    }
}

#[test]
fn bottom_fade_behind_a_control_does_not_wash_out_earlier_text() {
    let page = document();
    let capture = |top| {
        let mut rgba = viewport(&page, top);
        for y in HEIGHT - 96..HEIGHT {
            let alpha = (y - (HEIGHT - 96)) as f32 / 96. * 0.65;
            for x in 0..WIDTH {
                let i = ((y * WIDTH + x) * 4) as usize;
                for value in &mut rgba[i..i + 3] {
                    *value = (f32::from(*value) * (1. - alpha) + 255. * alpha).round() as u8;
                }
            }
        }
        control(&mut rgba, (239.25, HEIGHT as f32 - 22.75), 1.);
        rgba
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(400), SheetLimits::default()).unwrap();
    for top in (0..400).step_by(8).rev() {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewTop { .. }),
            "top {top}: {result:?}"
        );
    }
    let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
    export_scene("bottom-fade", &capture(400), &sheet, expected);
    assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400 - 128);
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
    let page = document();
    let mut first = viewport(&page, 0);
    control(&mut first, (239.25, HEIGHT as f32 - 22.75), 1.);
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &first, SheetLimits::default()).unwrap();
    let mut changed = page.clone();
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
        // The changed block outside the control halo refreshes immediately,
        // even though it lies within the edge-fade detection area.
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

#[test]
fn document_local_controls_and_text_are_preserved() {
    let mut page = document();
    // This control is part of the document, so it moves with the page.
    control(
        &mut page[..(WIDTH * HEIGHT * 4) as usize],
        (239.25, 570.75),
        0.85,
    );
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &viewport(&page, 0), SheetLimits::default()).unwrap();
    for top in (8..=400).step_by(8) {
        assert!(matches!(
            sheet.push(&viewport(&page, top)),
            Fold::GrewBottom { .. }
        ));
        assert!(sheet.floating.is_empty(), "unexpected mask at {top}");
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + top) * WIDTH * 4) as usize],
            0,
            HEIGHT + top,
        );
    }
}

#[test]
fn translucent_edge_control_disappearance_exposes_latest_content() {
    let page = document();
    let capture = |top| {
        let mut rgba = viewport(&page, top);
        control(&mut rgba, (239.25, HEIGHT as f32 - 12.75), 0.85);
        rgba
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(400), SheetLimits::default()).unwrap();
    for top in (200..400).step_by(8).rev() {
        assert!(matches!(sheet.push(&capture(top)), Fold::GrewTop { .. }));
    }
    assert!(!sheet.floating.is_empty());
    for top in (0..200).step_by(8).rev() {
        assert!(matches!(
            sheet.push(&viewport(&page, top)),
            Fold::GrewTop { .. }
        ));
    }
    assert!(sheet.floating.is_empty());
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 400) * WIDTH * 4) as usize],
        0,
        HEIGHT + 400 - 80,
    );
}

/// A translucent edge layer with horizontally varying opacity and backdrop blur.
fn blurred_viewport(page: &[u8], top: u32, sigma: f32, fade_height: u32) -> Vec<u8> {
    let mut rgba = viewport(page, top);
    let image = image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba.clone()).unwrap();
    let blurred = image::imageops::blur(&image, sigma);
    for y in HEIGHT - fade_height..HEIGHT {
        let fade = (y - (HEIGHT - fade_height)) as f32 / fade_height as f32;
        for x in 0..WIDTH {
            let alpha = fade * (0.45 + 0.2 * x as f32 / WIDTH as f32);
            let i = ((y * WIDTH + x) * 4) as usize;
            for c in 0..3 {
                let background = f32::from(blurred.as_raw()[i + c]);
                rgba[i + c] = (background * (1. - alpha) + 255. * alpha).round() as u8;
            }
        }
    }
    control(&mut rgba, (239.25, HEIGHT as f32 - 22.75), 1.);
    rgba
}

#[test]
fn blurred_edge_backdrop_does_not_slice_clear_text_into_pale_stripes() {
    let page = document();
    for (sigma, fade_height) in [(0., 120), (1.2, 120), (4., 120), (8., 120), (4., 220)] {
        let capture = |top| blurred_viewport(&page, top, sigma, fade_height);
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &capture(400), SheetLimits::default()).unwrap();
        for top in (0..400).step_by(8).rev() {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewTop { dy: 8, .. }),
                "sigma {sigma}, top {top}: {result:?}"
            );
            // Every previously accepted clear row survives the next frame.
            assert_pixels(
                sheet.canvas(),
                &page[(top * WIDTH * 4) as usize..((HEIGHT + 400) * WIDTH * 4) as usize],
                8,
                HEIGHT + 400 - fade_height - 40 - top,
            );
        }
        let expected = &page[..((HEIGHT + 400) * WIDTH * 4) as usize];
        export_scene(
            &format!("blurred-edge-{sigma}-{fade_height}"),
            &capture(400),
            &sheet,
            expected,
        );
        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400 - fade_height - 40);
        for top in (8..=400).step_by(8) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::Shifted { dy: 8 }),
                "sigma {sigma}, top {top}: {result:?}"
            );
            assert_pixels(sheet.canvas(), expected, 0, HEIGHT + 400 - fade_height - 40);
        }
        for top in (408..=640).step_by(8) {
            assert!(matches!(
                sheet.push(&capture(top)),
                Fold::GrewBottom { dy: 8, .. }
            ));
        }
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + 640) * WIDTH * 4) as usize],
            0,
            HEIGHT + 400,
        );
    }
}

fn scene_label(pixels: &mut [u8], x: f32, y: f32, text: &str) {
    let font =
        FontRef::try_from_slice(include_bytes!("../../../assets/DejaVuSans-Bold.ttf")).unwrap();
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

/// A two-column document with rules and a full-width bar appearing only after
/// the original heading has left the viewport. All content is generated here.
fn pinned_heading_scene(background: u8) -> (Vec<u8>, Vec<u8>) {
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
    for background in [255, 246, 32] {
        let (page, bar) = pinned_heading_scene(background);
        for step in [8u32, 40, 80, 160, 320] {
            let capture = |top| {
                let mut frame = viewport(&page, top);
                if top >= 160 {
                    frame[..bar.len()].copy_from_slice(&bar);
                }
                frame
            };
            let mut sheet =
                ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
            for top in (step..=640).step_by(step as usize) {
                let result = sheet.push(&capture(top));
                assert!(
                    matches!(result, Fold::GrewBottom { dy, .. } if dy == step),
                    "{step} at {top}: {result:?}"
                );
                assert_eq!(sheet.dimensions(), (WIDTH, HEIGHT + top), "at {top}");
                assert_pixels(
                    sheet.canvas(),
                    &page[..((HEIGHT + top) * WIDTH * 4) as usize],
                    0,
                    HEIGHT + top,
                );
            }
            if background == 246 && step == 160 {
                export_scene(
                    "late-heading",
                    &capture(640),
                    &sheet,
                    &page[..((HEIGHT + 640) * WIDTH * 4) as usize],
                );
            }
            for top in (0..640).step_by(step as usize).rev() {
                let result = sheet.push(&capture(top));
                assert!(
                    matches!(result, Fold::Shifted { dy } if dy == -(step as i32)),
                    "{step} at {top}: {result:?}"
                );
                assert_pixels(
                    sheet.canvas(),
                    &page[..((HEIGHT + 640) * WIDTH * 4) as usize],
                    0,
                    HEIGHT + 640,
                );
            }
        }
    }
}

#[test]
fn sliding_heading_does_not_write_its_rules_into_the_document() {
    let (page, bar) = pinned_heading_scene(246);
    let capture = |top| {
        let mut frame = viewport(&page, top);
        let visible = top.saturating_sub(152).min(48) as usize * WIDTH as usize * 4;
        frame[..visible].copy_from_slice(&bar[bar.len() - visible..]);
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (8..=400).step_by(8) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 8, .. }),
            "{top}: {result:?}"
        );
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + top) * WIDTH * 4) as usize],
            0,
            HEIGHT + top,
        );
    }
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
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(400), SheetLimits::default()).unwrap();
    for top in (0..400).step_by(8).rev() {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewTop { dy: 8, .. }),
            "{top}: {result:?}"
        );
    }
    assert_eq!(sheet.dimensions(), (WIDTH, HEIGHT + 400));
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 400) * WIDTH * 4) as usize],
        0,
        HEIGHT + 400,
    );
}

#[test]
fn wide_capture_keeps_rules_exact_across_the_sample_grid() {
    let (page, bar) = pinned_heading_scene(246);
    let scale = 3;
    let width = WIDTH * scale;
    let height = HEIGHT * scale;
    let page = image::RgbaImage::from_raw(WIDTH, PAGE_HEIGHT, page).unwrap();
    let page = image::imageops::resize(
        &page,
        width,
        PAGE_HEIGHT * scale,
        image::imageops::FilterType::Nearest,
    )
    .into_raw();
    let bar = image::RgbaImage::from_raw(WIDTH, 48, bar).unwrap();
    let bar = image::imageops::resize(
        &bar,
        width,
        48 * scale,
        image::imageops::FilterType::Nearest,
    )
    .into_raw();
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
        let wrong = sheet
            .canvas()
            .iter()
            .zip(&page)
            .position(|(actual, expected)| actual != expected);
        assert!(wrong.is_none(), "at {top}: first incorrect byte {wrong:?}");
    }
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
    for step in [8u32, 40, 96] {
        for resume in [false, true] {
            let capture = |top: u32| {
                let mut frame = viewport(&page, top);
                let side_top = if resume && top > 600 {
                    top - 400
                } else {
                    top.min(200)
                };
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
            let end = 800u32.div_ceil(step) * step;
            let mut sheet =
                ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
            for top in (step..=end).step_by(step as usize) {
                let result = sheet.push(&capture(top));
                assert!(
                    matches!(result, Fold::GrewBottom { dy, .. } if dy == step),
                    "step {step}, resume {resume}, at {top}: {result:?}"
                );
                let expected = &page[..((HEIGHT + top) * WIDTH * 4) as usize];
                assert_pixels(sheet.canvas(), expected, 0, HEIGHT + top);
            }
            if step == 8 && !resume {
                export_scene(
                    "pinned-sidebar",
                    &capture(end),
                    &sheet,
                    &page[..((HEIGHT + end) * WIDTH * 4) as usize],
                );
            }
            for top in (0..end).step_by(step as usize).rev() {
                let result = sheet.push(&capture(top));
                assert!(
                    matches!(result, Fold::Shifted { dy } if dy == -(step as i32)),
                    "return step {step}, resume {resume}, at {top}: {result:?}"
                );
                assert_pixels(
                    sheet.canvas(),
                    &page[..((HEIGHT + end) * WIDTH * 4) as usize],
                    0,
                    HEIGHT + end,
                );
            }
        }
    }
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
    for top in (40..=640).step_by(40) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 40, .. }),
            "{top}: {result:?}"
        );
        assert_pixels(
            sheet.canvas(),
            &page[..((HEIGHT + top) * WIDTH * 4) as usize],
            0,
            HEIGHT + top,
        );
    }
}

#[test]
fn sparse_sidebar_below_a_wide_heading_pins_as_a_complete_column() {
    for heading_height in [0, 160, 360, 480] {
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
        }
        if heading_height > 0 {
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
        // The last variant includes a fixed browser-like strip above the page.
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
        for top in (40..=960).step_by(40) {
            let result = sheet.push(&capture(top));
            assert!(
                matches!(result, Fold::GrewBottom { dy: 40, .. }),
                "heading {heading_height}, top {top}: {result:?}"
            );
            assert_pixels(
                sheet.canvas(),
                &page[..((HEIGHT + top) * WIDTH * 4) as usize],
                0,
                HEIGHT + top,
            );
        }
        if heading_height == 480 {
            export_scene(
                "sparse-sidebar",
                &capture(960),
                &sheet,
                &page[..((HEIGHT + 960) * WIDTH * 4) as usize],
            );
        }
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
    for dark in [false, true] {
        if dark {
            for pixel in page.as_chunks_mut::<4>().0 {
                for value in &mut pixel[..3] {
                    *value = 255 - *value;
                }
            }
        }
        for step in [1u32, 4, 12] {
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
            for top in (step..=240).step_by(step as usize) {
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
                assert_pixels(
                    sheet.canvas(),
                    &page[..((HEIGHT + accepted_top) * WIDTH * 4) as usize],
                    0,
                    HEIGHT + accepted_top,
                );
                assert!(!sheet.has_incomplete_column());
            }
            assert_eq!(accepted_top, 240);
            if step == 1 {
                export_scene(
                    if dark {
                        "short-sidebar-margin-dark"
                    } else {
                        "short-sidebar-margin"
                    },
                    &capture(240),
                    &sheet,
                    &page[..((HEIGHT + 240) * WIDTH * 4) as usize],
                );
            }
        }
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
    for top in (8..=240).step_by(8) {
        let result = sheet.push(&capture(top));
        assert!(
            matches!(result, Fold::GrewBottom { dy: 8, .. }),
            "{top}: {result:?}"
        );
        let left = sheet.sidebar.as_ref().unwrap().left();
        for y in 0..HEIGHT + top {
            let i = (y * WIDTH * 4) as usize;
            assert_eq!(&sheet.canvas()[i..i + left * 4], &page[i..i + left * 4]);
            assert_eq!(
                &sheet.canvas()[i + left * 4..i + (WIDTH - 4) as usize * 4],
                &page[i + left * 4..i + (WIDTH - 4) as usize * 4]
            );
        }
    }
}

#[test]
fn sidebar_padding_is_opaque_and_later_full_width_sections_survive() {
    for (background, contrasting) in [(255u8, true), (0, true), (255, false), (0, false)] {
        for step in [8u32, 40, 96] {
            let (mut page, _) = pinned_heading_scene(246);
            for y in 720..PAGE_HEIGHT {
                for x in 336..WIDTH {
                    let i = ((y * WIDTH + x) * 4) as usize;
                    page[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
            // Independently rasterized article content replaces the two-column
            // layout, with both unchanged and contrasting section backgrounds.
            let full_width = document();
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
                assert!(
                    sheet
                        .canvas()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| pixel[3] == 255),
                    "transparent padding at {top}"
                );
            }
            assert_eq!(accepted, end);
            let assert_content = |sheet: &ScrollSheet| {
                for y in 0..HEIGHT + end {
                    let i = (y * WIDTH * 4) as usize;
                    let bytes = (WIDTH - 4) as usize * 4;
                    let wrong = sheet.canvas()[i..i + bytes]
                        .iter()
                        .zip(&page[i..i + bytes])
                        .position(|(a, b)| a != b);
                    assert!(
                        wrong.is_none(),
                        "background {background}, contrast {contrasting}, step {step}, row {y}, byte {wrong:?}"
                    );
                }
            };
            assert_content(&sheet);
            for top in (0..end).step_by(step as usize).rev() {
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
                assert_content(&sheet);
            }
            assert_eq!(accepted, 0);
            if step == 40 {
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
    }
}

#[test]
fn a_pinned_panel_resuming_with_a_wide_rule_is_not_new_document_content() {
    for footer in [false, true] {
        for dark in [false, true] {
            for step in [8u32, 40, 96] {
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
                    let full_width = document();
                    let first = (1_400 * WIDTH * 4) as usize;
                    page[first..].copy_from_slice(&full_width[first..]);
                    for pixel in page[first..].as_chunks_mut::<4>().0 {
                        if pixel[..3] == [255; 3] {
                            pixel[..3].fill(218);
                        }
                    }
                }
                if dark {
                    for pixel in page.as_chunks_mut::<4>().0 {
                        for channel in &mut pixel[..3] {
                            *channel = 255 - *channel;
                        }
                    }
                }
                let (_, mut chrome) = pinned_heading_scene(226);
                scene_label(&mut chrome, 348., 10., "Panel controls");
                page[..chrome.len()].copy_from_slice(&chrome);
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
                        result => panic!(
                            "footer {footer}, dark {dark}, step {step}, at {top}: {result:?}"
                        ),
                    }
                }
                assert_eq!(accepted, end);
                let assert_content = |sheet: &ScrollSheet| {
                    for y in 0..HEIGHT + end {
                        for x in 0..WIDTH {
                            let i = ((y * WIDTH + x) * 4) as usize;
                            assert_eq!(
                                &sheet.canvas()[i..i + 4],
                                &page[i..i + 4],
                                "footer {footer}, dark {dark}, step {step}: ({x}, {y})"
                            );
                        }
                    }
                };
                assert_content(&sheet);
                for top in (0..end).step_by(step as usize).rev() {
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
                    assert_content(&sheet);
                }
                assert_eq!(accepted, 0);
                if step == 40 && !dark {
                    export_scene(
                        if footer {
                            "resumed-panel-footer"
                        } else {
                            "resumed-panel"
                        },
                        &capture(end),
                        &sheet,
                        &page[..((HEIGHT + end) * WIDTH * 4) as usize],
                    );
                }
            }
        }
    }
}

#[test]
fn a_resumed_column_recovers_after_partial_repaints_without_splitting_its_tail() {
    for step in [8u32, 40, 96] {
        for bad_frames in [1, 3] {
            for dark in [false, true] {
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
                        result => panic!(
                            "step {step}, bad {bad_frames}, dark {dark}, at {top}: {result:?}"
                        ),
                    }
                }
                assert_eq!(accepted, end);
                let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
                if step == 40 && bad_frames == 1 && !dark {
                    export_scene("repainted-column", &capture(end), &sheet, expected);
                }
                assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
                for top in (0..end).step_by(step as usize).rev() {
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
                    if bad.contains(&top) {
                        assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
                    }
                }
                assert_eq!(accepted, 0);
                assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
            }
        }
    }
}

#[test]
fn a_document_rule_above_resumed_metadata_does_not_duplicate_the_panel() {
    for step in [8u32, 40, 96] {
        for dark in [false, true] {
            for footer in [false, true] {
                let (mut page, _) = pinned_heading_scene(246);
                for y in 980..PAGE_HEIGHT {
                    for x in 326..WIDTH {
                        let at = ((y * WIDTH + x) * 4) as usize;
                        page[at..at + 4].fill(255);
                    }
                }
                if footer {
                    let full_width = document();
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
                        result => panic!(
                            "step {step}, dark {dark}, footer {footer}, at {top}: {result:?}"
                        ),
                    }
                }
                assert_eq!(accepted, end);
                let expected = &page[..((HEIGHT + end) * WIDTH * 4) as usize];
                if step == 40 && !dark {
                    export_scene(
                        if footer {
                            "column-crossing-rule-footer"
                        } else {
                            "column-crossing-rule"
                        },
                        &capture(end),
                        &sheet,
                        expected,
                    );
                }
                assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
                for top in (0..end).step_by(step as usize).rev() {
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
                    assert_pixels(sheet.canvas(), expected, 0, HEIGHT + end);
                }
                assert_eq!(accepted, 0);
            }
        }
    }
}

/// A compositor frame over a captured window, with antialiased rounded corners
/// and an opaque desktop behind it. The same geometry draws the expected outer
/// perimeter of a finished long image; it never draws interior seams.
fn rounded_window(pixels: &mut [u8], width: u32, radius: f32, border: [u8; 3]) {
    let height = pixels.len() / (width as usize * 4);
    for y in 0..height {
        for x in 0..width as usize {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let cx = px.clamp(radius, width as f32 - radius);
            let cy = py.clamp(radius, height as f32 - radius);
            let distance = radius - (px - cx).hypot(py - cy);
            let inside = distance.clamp(0., 1.);
            let fill = (distance - 2.).clamp(0., 1.);
            let at = (y * width as usize + x) * 4;
            for (channel, edge) in pixels[at..at + 3].iter_mut().zip(border) {
                let window = f32::from(*channel) * fill + f32::from(edge) * (1. - fill);
                *channel = (window * inside + 90. * (1. - inside)).round() as u8;
            }
        }
    }
}

#[test]
fn rounded_window_frame_stays_on_the_outer_perimeter_of_a_stitched_document() {
    for dark in [false, true] {
        for (step, radius) in [(8, 8.), (40, 24.), (96, 40.)] {
            let border = if dark { [170, 80, 220] } else { [20, 130, 230] };
            check_rounded_window_column(step, radius, dark, false, border);
        }
    }
}

#[test]
fn rounded_window_preserves_pinned_column_glyphs_near_the_bottom_edge() {
    for dark in [false, true] {
        for (step, radius) in [(8, 8.), (40, 24.), (96, 40.)] {
            let border = if dark { [170, 80, 220] } else { [20, 130, 230] };
            check_rounded_window_column(step, radius, dark, true, border);
        }
        check_rounded_window_column(16, 24., dark, true, [if dark { 215 } else { 40 }; 3]);
    }
}

fn check_rounded_window_column(
    step: u32,
    radius: f32,
    dark: bool,
    low_text: bool,
    border: [u8; 3],
) {
    let (mut page, mut chrome) = pinned_heading_scene(226);
    for y in 720..1_400 {
        for x in 326..WIDTH {
            let at = ((y * WIDTH + x) * 4) as usize;
            page[at..at + 4].fill(255);
        }
    }
    if low_text {
        for y in (734..826).step_by(18) {
            scene_label(&mut page, 344.25, y as f32 + 0.25, &format!("Tail {y}"));
        }
    }
    let full_width = document();
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
    let mut accepted = 0;
    let mut unclear = 0;
    for top in (step..1_200).step_by(step as usize).chain([1_200]) {
        let result = sheet.push(&capture(top));
        if low_text {
            for y in HEIGHT - HEIGHT / 8..HEIGHT - 3 {
                for x in 80..WIDTH - 80 {
                    assert!(
                        !sheet.floating.frame_edge.mask()[(y * WIDTH + x) as usize],
                        "frame mask covers document at {x},{y}, scroll {top}"
                    );
                }
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
    assert_eq!(accepted, 1_200);
    let mut expected = page[..((HEIGHT + 1_200) * WIDTH * 4) as usize].to_vec();
    rounded_window(&mut expected, WIDTH, radius, border);
    export_scene(
        &format!("rounded-window-{step}-{dark}-{low_text}"),
        &capture(1_200),
        &sheet,
        &expected,
    );
    assert_window_pixels(sheet.canvas(), &expected);
    for top in (0..1_200).step_by(step as usize).rev() {
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
        assert_window_pixels(sheet.canvas(), &expected);
    }
    assert_eq!(accepted, 0);
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
    let capture = |top| {
        let mut frame = viewport(&page, top);
        frame[..chrome.len()].copy_from_slice(&chrome);
        rounded_window(&mut frame, WIDTH, 24., [20, 130, 230]);
        frame
    };
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &capture(1_200), SheetLimits::default()).unwrap();
    assert!(sheet.sidebar.is_none());
    for top in (0..1_200).step_by(40).rev() {
        assert!(
            matches!(sheet.push(&capture(top)), Fold::GrewTop { dy: 40, .. }),
            "at {top}"
        );
    }
    let mut expected = page[..((HEIGHT + 1_200) * WIDTH * 4) as usize].to_vec();
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
    for top in (40..=1_200).step_by(40) {
        assert!(matches!(
            sheet.push(&viewport(&page, top)),
            Fold::GrewBottom { dy: 40, .. }
        ));
        assert!(sheet.floating.frame_edge.is_empty());
    }
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 1_200) * WIDTH * 4) as usize],
        0,
        HEIGHT + 1_200,
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
        if top <= 400 {
            rounded_window(&mut frame, WIDTH, 24., [20, 130, 230]);
        }
        frame
    };
    let mut sheet = ScrollSheet::new(WIDTH, HEIGHT, &capture(0), SheetLimits::default()).unwrap();
    for top in (40..=1_200).step_by(40) {
        assert!(
            matches!(sheet.push(&capture(top)), Fold::GrewBottom { dy: 40, .. }),
            "at {top}"
        );
        if top > 400 {
            assert!(sheet.floating.frame_edge.is_empty());
        }
    }
    // Older observations retain their captured appearance. Newly revealed
    // document rows must not inherit the removed decoration's mask.
    assert_pixels(
        sheet.canvas(),
        &page[..((HEIGHT + 1_200) * WIDTH * 4) as usize],
        HEIGHT + 400,
        HEIGHT + 1_200,
    );
}

#[test]
fn independent_column_exclusion_preserves_body_and_later_section_controls() {
    let page = document();
    let row = WIDTH as usize * 4;
    for (step, opacity) in [(8, 1.), (40, 0.85)] {
        let capture = |top| {
            let mut frame = viewport(&page, top);
            control(&mut frame, (180.25, 318.75), opacity);
            control(&mut frame, (399.25, 510.75), opacity);
            frame
        };
        let mut canvas = capture(0);
        let mut floating = super::floating::FloatingOverlays::new(&canvas, WIDTH);
        for top in (step..=400).step_by(step as usize) {
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
        let expected = &page[..(400 + HEIGHT) as usize * row];
        assert_pixels(&canvas, expected, 0, 400 + HEIGHT);
        for top in (0..400).step_by(step as usize).rev() {
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
            assert_pixels(&canvas, expected, 0, 400 + HEIGHT);
        }
    }
}

#[test]
fn lateral_layout_shifts_preserve_the_complete_document() {
    let page = document();
    for (direction, amount) in [(-1, 1), (1, 1), (-1, 2), (1, 2), (-1, 8), (1, 8)] {
        let mut sheet =
            ScrollSheet::new(WIDTH, HEIGHT, &viewport(&page, 0), SheetLimits::default()).unwrap();
        for top in (24..=360).step_by(24) {
            let offset = match top {
                0..=48 => 0,
                49..=144 => amount * direction,
                145..=240 => -(amount - 1).max(1) * direction,
                _ => 0,
            };
            let original = viewport(&page, top);
            let frame = shifted_scene(&original, offset);
            let outcome = sheet.push(&frame);
            assert!(
                matches!(outcome, Fold::GrewBottom { growth: 24, .. }),
                "top={top} offset={offset}: {outcome:?}"
            );
            assert_eq!(sheet.height(), top + HEIGHT);
            assert_eq!(
                sheet.canvas(),
                &page[..((top + HEIGHT) * WIDTH * 4) as usize],
                "top={top} offset={offset}"
            );
        }
    }
}

#[test]
fn lateral_correction_preserves_prepend_and_stationary_alignment() {
    let page = document();
    let mut sheet =
        ScrollSheet::new(WIDTH, HEIGHT, &viewport(&page, 240), SheetLimits::default()).unwrap();
    let shifted = shifted_scene(&viewport(&page, 240), 8);
    assert!(matches!(
        sheet.push(&shifted),
        Fold::Still | Fold::Unchanged
    ));
    for top in (0..240).step_by(24).rev() {
        let outcome = sheet.push(&shifted_scene(&viewport(&page, top), 8));
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
    let mut shifted = vec![255; frame.len()];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let source_x = x as i32 - offset;
            if (0..WIDTH as i32).contains(&source_x) {
                let source = (y * WIDTH as usize + source_x as usize) * 4;
                let destination = (y * WIDTH as usize + x) * 4;
                shifted[destination..destination + 4].copy_from_slice(&frame[source..source + 4]);
            }
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
        // The synthetic viewport exposes the same dark background at its edge.
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

    let page = document();
    let stationary = |top, dx| {
        let mut frame = shifted_scene(&viewport(&page, top), dx);
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

#[test]
fn lateral_layout_warning_does_not_turn_into_scroll_speed_advice() {
    use super::{Phase, Refusal, phase_of};
    assert_eq!(phase_of(1, Refusal::LayoutShift), Phase::Following);
    for count in [2, 40, 100] {
        assert_eq!(phase_of(count, Refusal::LayoutShift), Phase::LayoutShift);
    }
    assert_eq!(phase_of(2, Refusal::NoOverlap), Phase::NoOverlap);
    assert_eq!(phase_of(2, Refusal::Mismatch), Phase::Unstable);
}
