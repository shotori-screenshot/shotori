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
