//! Pixel filters with rectangular or brush masks, applied in annotation order.
use super::{Shape, ShapeKind};
use gpui_kit::{Pixels, Point};

pub(super) fn rasterize(
    shape: &Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    match shape.filter_area() {
        None | Some(super::FilterArea::Rectangle) => {
            rasterize_rect(shape, rgba, w, h, origin, scale);
            return;
        }
        Some(super::FilterArea::Ellipse) => {
            rasterize_ellipse(shape, rgba, w, h, origin, scale);
            return;
        }
        Some(super::FilterArea::Brush { .. }) => {}
    }
    let Some(super::FilterArea::Brush { diameter }) = shape.filter_area() else {
        return;
    };
    let bounds = shape.ink_bounds();
    let left = (f32::from(bounds.left() - origin.x) * scale)
        .floor()
        .clamp(0., w as f32) as u32;
    let top = (f32::from(bounds.top() - origin.y) * scale)
        .floor()
        .clamp(0., h as f32) as u32;
    let right = (f32::from(bounds.right() - origin.x) * scale)
        .ceil()
        .clamp(0., w as f32) as u32;
    let bottom = (f32::from(bounds.bottom() - origin.y) * scale)
        .ceil()
        .clamp(0., h as f32) as u32;
    if right <= left || bottom <= top {
        return;
    }
    let (width, height) = (right - left, bottom - top);
    let local_origin = origin
        + gpui_kit::point(
            gpui_kit::px(left as f32 / scale),
            gpui_kit::px(top as f32 / scale),
        );
    let mut filtered = Vec::with_capacity((width * height * 4) as usize);
    for y in top..bottom {
        let start = ((y * w + left) * 4) as usize;
        filtered.extend_from_slice(&rgba[start..start + (width * 4) as usize]);
    }
    let mut region = shape.clone();
    region.bounds = gpui_kit::Bounds::new(
        local_origin,
        gpui_kit::size(
            gpui_kit::px(width as f32 / scale),
            gpui_kit::px(height as f32 / scale),
        ),
    );
    rasterize_rect(&region, &mut filtered, width, height, local_origin, scale);
    // Stream the exact capsule union coverage directly into destination pixels
    // without allocating a full-bbox mask image or scanning uncovered background.
    let mut stroke = shape.clone();
    stroke.kind = ShapeKind::Pencil;
    stroke.parameters = super::ShapeParameters::new(ShapeKind::Pencil, diameter);
    super::line::coverage(
        &stroke,
        width,
        height,
        local_origin,
        scale,
        |source, cov| {
            let pix_idx = source / 4;
            let x = pix_idx as u32 % width;
            let y = pix_idx as u32 / width;
            let target = (((y + top) * w + x + left) * 4) as usize;
            if rgba[target + 3] == 0 {
                return;
            }
            let coverage = (cov * 255.).round() as u32;
            for channel in 0..3 {
                rgba[target + channel] = ((filtered[source + channel] as u32 * coverage
                    + rgba[target + channel] as u32 * (255 - coverage)
                    + 127)
                    / 255) as u8;
            }
        },
    );
}

fn rasterize_ellipse(
    shape: &Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let rx = f32::from(shape.bounds.size.width) * scale / 2.;
    let ry = f32::from(shape.bounds.size.height) * scale / 2.;
    if rx <= 0. || ry <= 0. {
        return;
    }
    let cx = f32::from(shape.bounds.left() - origin.x) * scale + rx;
    let cy = f32::from(shape.bounds.top() - origin.y) * scale + ry;
    let left = (cx - rx).floor().clamp(0., w as f32) as u32;
    let right = (cx + rx).ceil().clamp(0., w as f32) as u32;
    let top = (cy - ry).floor().clamp(0., h as f32) as u32;
    let bottom = (cy + ry).ceil().clamp(0., h as f32) as u32;
    if right <= left || bottom <= top {
        return;
    }
    let (width, height) = (right - left, bottom - top);
    let local_origin = origin
        + gpui_kit::point(
            gpui_kit::px(left as f32 / scale),
            gpui_kit::px(top as f32 / scale),
        );
    let mut filtered = Vec::with_capacity((width * height * 4) as usize);
    for y in top..bottom {
        let start = ((y * w + left) * 4) as usize;
        filtered.extend_from_slice(&rgba[start..start + (width * 4) as usize]);
    }
    let mut region = shape.clone();
    region.bounds = gpui_kit::Bounds::new(
        local_origin,
        gpui_kit::size(
            gpui_kit::px(width as f32 / scale),
            gpui_kit::px(height as f32 / scale),
        ),
    );
    rasterize_rect(&region, &mut filtered, width, height, local_origin, scale);
    let mut row_cov = vec![0_f32; width as usize];
    for y in 0..height {
        row_cov.fill(0.);
        let global_row = top + y;
        for sample in 0..8 {
            let dy = global_row as f32 + (sample as f32 + 0.5) / 8. - cy;
            if dy.abs() >= ry {
                continue;
            }
            let half = rx * (1. - (dy / ry).powi(2)).sqrt();
            let (start, end) = (cx - half, cx + half);
            let start_col = (start.floor().max(left as f32) as u32).min(right);
            let end_col = (end.ceil().max(left as f32) as u32).min(right);
            for col in start_col..end_col {
                row_cov[(col - left) as usize] +=
                    (end.min(col as f32 + 1.) - start.max(col as f32)).max(0.) / 8.;
            }
        }
        for (ix, cov) in row_cov.iter().copied().enumerate() {
            if cov <= 0. {
                continue;
            }
            let source = ((y * width + ix as u32) * 4) as usize;
            let target = ((global_row * w + left + ix as u32) * 4) as usize;
            if rgba[target + 3] == 0 {
                continue;
            }
            let coverage = (cov.min(1.) * 255.).round() as u32;
            for channel in 0..3 {
                rgba[target + channel] = ((filtered[source + channel] as u32 * coverage
                    + rgba[target + channel] as u32 * (255 - coverage)
                    + 127)
                    / 255) as u8;
            }
        }
    }
}

fn rasterize_rect(
    shape: &Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let x = |v: Pixels| {
        (f32::from(v - origin.x) * scale)
            .round()
            .clamp(0., w as f32) as usize
    };
    let y = |v: Pixels| {
        (f32::from(v - origin.y) * scale)
            .round()
            .clamp(0., h as f32) as usize
    };
    let (left, right, top, bottom) = (
        x(shape.bounds.left()),
        x(shape.bounds.right()),
        y(shape.bounds.top()),
        y(shape.bounds.bottom()),
    );
    if right <= left || bottom <= top {
        return;
    }
    let (width, height) = (right - left, bottom - top);
    let stride = w as usize;
    let strength = (shape.size() * scale).round().max(1.) as usize;
    if shape.kind == ShapeKind::Mosaic {
        for y in (top..bottom).step_by(strength) {
            for x in (left..right).step_by(strength) {
                let end_x = (x + strength).min(right);
                let end_y = (y + strength).min(bottom);
                let mut sum = [0_u64; 4];
                for row in y..end_y {
                    for col in x..end_x {
                        let pixel = &rgba[(row * stride + col) * 4..][..4];
                        for c in 0..3 {
                            sum[c] += pixel[c] as u64 * pixel[3] as u64;
                        }
                        sum[3] += pixel[3] as u64;
                    }
                }
                if sum[3] == 0 {
                    continue;
                }
                for row in y..end_y {
                    for col in x..end_x {
                        let pixel = &mut rgba[(row * stride + col) * 4..][..4];
                        if pixel[3] == 0 {
                            continue;
                        }
                        for c in 0..3 {
                            pixel[c] = ((sum[c] + sum[3] / 2) / sum[3]) as u8;
                        }
                    }
                }
            }
        }
    } else {
        // Separable sliding sums: O(area), even at high strength. Weight by alpha
        // so transparent desktop gaps never introduce a dark halo.
        let radius = (strength / 2).max(1);
        // Retain only the horizontal rows in the vertical window. Reading
        // future rows before overwriting output preserves the original input.
        let ring_height = (radius * 2 + 1).min(height);
        let mut horizontal = vec![[0_u64; 4]; width * ring_height];
        let mut vertical = vec![[0_u64; 4]; width];
        let mut loaded = 0;
        for row in 0..height {
            let needed = (row + radius + 1).min(height);
            while loaded < needed {
                let slot = loaded % ring_height;
                let values = &mut horizontal[slot * width..(slot + 1) * width];
                let mut sum = [0_u64; 4];
                let add = |sum: &mut [u64; 4], col: usize, subtract: bool| {
                    let p = &rgba[((top + loaded) * stride + left + col) * 4..][..4];
                    for c in 0..4 {
                        let value = if c == 3 {
                            p[3] as u64
                        } else {
                            p[c] as u64 * p[3] as u64
                        };
                        if subtract {
                            sum[c] -= value;
                        } else {
                            sum[c] += value;
                        }
                    }
                };
                for col in 0..=radius.min(width - 1) {
                    add(&mut sum, col, false);
                }
                for col in 0..width {
                    values[col] = sum;
                    for c in 0..4 {
                        vertical[col][c] += sum[c];
                    }
                    if col >= radius {
                        add(&mut sum, col - radius, true);
                    }
                    if col + radius + 1 < width {
                        add(&mut sum, col + radius + 1, false);
                    }
                }
                loaded += 1;
            }
            let output = &mut rgba[((top + row) * stride + left) * 4..][..width * 4];
            for (pixel, sum) in output.as_chunks_mut::<4>().0.iter_mut().zip(&vertical) {
                if pixel[3] != 0 && sum[3] != 0 {
                    for c in 0..3 {
                        pixel[c] = ((sum[c] + sum[3] / 2) / sum[3]) as u8;
                    }
                }
            }
            if row >= radius {
                let slot = (row - radius) % ring_height;
                for (sum, old) in vertical
                    .iter_mut()
                    .zip(&horizontal[slot * width..(slot + 1) * width])
                {
                    for c in 0..4 {
                        sum[c] -= old[c];
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Bounds, point, px, size};
    fn shape(kind: ShapeKind) -> Shape {
        Shape {
            kind,
            number: None,
            text: None,
            text_style: Default::default(),
            bounds: Bounds::new(point(px(2.), px(2.)), size(px(8.), px(8.))),
            color: 0,
            parameters: crate::annotation::ShapeParameters::new(kind, 4.),
            points: vec![],
        }
    }

    #[test]
    fn mosaic_averages_each_tile_and_preserves_outside_and_gaps() {
        let mut data = [20, 40, 60, 255].repeat(12 * 12);
        for y in 2..6 {
            for x in 2..6 {
                data[(y * 12 + x) * 4] = if x < 4 { 0 } else { 100 };
            }
        }
        data[(3 * 12 + 3) * 4..(3 * 12 + 3) * 4 + 4].fill(0);
        let before = data.clone();
        rasterize(
            &shape(ShapeKind::Mosaic),
            &mut data,
            12,
            12,
            point(px(0.), px(0.)),
            1.,
        );
        assert_eq!(
            &data[(2 * 12 + 2) * 4..(2 * 12 + 2) * 4 + 4],
            &[53, 40, 60, 255]
        );
        assert_eq!(&data[(3 * 12 + 3) * 4..(3 * 12 + 3) * 4 + 4], &[0; 4]);
        for y in 0..12 {
            for x in 0..12 {
                if !(2..10).contains(&x) || !(2..10).contains(&y) {
                    let at = (y * 12 + x) * 4;
                    assert_eq!(&data[at..at + 4], &before[at..at + 4]);
                }
            }
        }
    }
    #[test]
    fn blur_matches_naive_alpha_weighted_reference() {
        let before: Vec<_> = (0..144)
            .flat_map(|i| {
                [
                    (i * 11 % 256) as u8,
                    70,
                    120,
                    if i % 7 == 0 { 0 } else { 255 },
                ]
            })
            .collect();
        for strength in [1., 4., 7., 16., 100.] {
            let radius = (strength as usize / 2).max(1);
            let mut mark = shape(ShapeKind::Blur);
            mark.parameters.set_size(strength);
            let mut actual = before.clone();
            rasterize(&mark, &mut actual, 12, 12, point(px(0.), px(0.)), 1.);
            for y in 2_usize..10 {
                for x in 2_usize..10 {
                    let at = (y * 12 + x) * 4;
                    if before[at + 3] == 0 {
                        assert_eq!(&actual[at..at + 4], &before[at..at + 4]);
                        continue;
                    }
                    let mut total = [0_u64; 4];
                    for row in y.saturating_sub(radius).max(2)..=(y + radius).min(9) {
                        for col in x.saturating_sub(radius).max(2)..=(x + radius).min(9) {
                            let p = &before[(row * 12 + col) * 4..][..4];
                            for c in 0..3 {
                                total[c] += p[c] as u64 * p[3] as u64;
                            }
                            total[3] += p[3] as u64;
                        }
                    }
                    for c in 0..3 {
                        assert_eq!(actual[at + c], ((total[c] + total[3] / 2) / total[3]) as u8);
                    }
                }
            }
        }
    }
    #[test]
    fn filters_handle_reverse_origin_fractional_scale_and_tiny_clipped_regions() {
        for kind in [ShapeKind::Mosaic, ShapeKind::Blur] {
            for scale in [1., 1.25, 1.73, 2.] {
                let mut mark = shape(kind);
                mark.bounds.origin = point(px(-4.), px(-4.));
                let mut pixels = [20, 40, 60, 255].repeat(12 * 12);
                rasterize(&mark, &mut pixels, 12, 12, point(px(-2.), px(-2.)), scale);
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| *p == [20, 40, 60, 255])
                );
                mark.bounds.size = size(px(0.1), px(0.1));
                rasterize(&mark, &mut pixels, 12, 12, point(px(0.), px(0.)), scale);
            }
        }
    }
    #[test]
    fn brush_filters_preserve_pixels_outside_capsules_and_destination_alpha() {
        for kind in [ShapeKind::Mosaic, ShapeKind::Blur] {
            for scale in [1., 1.25, 2.] {
                for dot in [false, true] {
                    let origin = point(px(-3.), px(5.));
                    let mut mark = shape(kind);
                    mark.parameters = super::super::ShapeParameters::Filter {
                        strength: 8.,
                        area: super::super::FilterArea::Brush { diameter: 8. },
                    };
                    mark.points = vec![origin + point(px(12.), px(12.))];
                    if !dot {
                        mark.points.push(origin + point(px(36.), px(36.)));
                    }
                    let before: Vec<u8> = (0..96 * 96)
                        .flat_map(|i| {
                            [
                                (i * 37 % 251) as u8,
                                (i * 13 % 251) as u8,
                                80,
                                if i % 17 == 0 { 0 } else { 200 },
                            ]
                        })
                        .collect();
                    let mut pixels = before.clone();
                    rasterize(&mark, &mut pixels, 96, 96, origin, scale);
                    assert_ne!(pixels, before);
                    for y in 0..96 {
                        for x in 0..96 {
                            let at = (y * 96 + x) * 4;
                            assert_eq!(pixels[at + 3], before[at + 3]);
                            let (lx, ly) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
                            let t = if dot {
                                0.
                            } else {
                                ((lx + ly - 24.) / 48.).clamp(0., 1.)
                            };
                            let center = 12. + t * 24.;
                            if (lx - center).hypot(ly - center) > 5. || before[at + 3] == 0 {
                                assert_eq!(&pixels[at..at + 4], &before[at..at + 4]);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn elliptical_filters_preserve_corners_convert_in_place_and_resize() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(120.)));
        for kind in [ShapeKind::Mosaic, ShapeKind::Blur] {
            let mut marks = crate::annotation::Annotations::default();
            marks.toggle(kind);
            marks.set_filter_area_kind(crate::annotation::FilterAreaKind::Ellipse);
            marks.begin(point(px(20.), px(20.)), area, false);
            marks.drag_to(point(px(100.), px(100.)), area, false);
            marks.end();
            marks.select_index(0);
            assert_eq!(marks.shapes[0].handle_points().len(), 8);
            // Corner of the bounding box lies outside the inscribed circle.
            assert_eq!(marks.hit_test(point(px(22.), px(22.))), None);
            assert_eq!(marks.hit_test(point(px(60.), px(60.))), Some(0));

            let before: Vec<u8> = (0..120 * 120)
                .flat_map(|i| [(i * 29 % 251) as u8, (i * 17 % 251) as u8, 90, 255])
                .collect();
            let mut pixels = before.clone();
            marks.rasterize(&mut pixels, 120, 120, area.origin, 1.);
            let corner = (22 * 120 + 22) * 4;
            let center = (60 * 120 + 60) * 4;
            assert_eq!(&pixels[corner..corner + 4], &before[corner..corner + 4]);
            assert_ne!(&pixels[center..center + 4], &before[center..center + 4]);

            // Converting Ellipse -> Rectangle keeps the selection and records one undo step.
            marks.set_filter_area_kind(crate::annotation::FilterAreaKind::Rectangle);
            assert_eq!(marks.selected_index(), Some(0));
            assert_eq!(marks.hit_test(point(px(22.), px(22.))), Some(0));
            marks.undo();
            assert_eq!(
                marks.shapes[0].filter_area(),
                Some(super::super::FilterArea::Ellipse)
            );
        }
    }
}
