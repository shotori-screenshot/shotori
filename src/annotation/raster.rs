//! Pixel compositing shared by display previews and exported captures.
//!
//! Magnifiers sample the complete ordinary mark layer without recursively sampling
//! other lenses. Display order remains independent of that sampling pass.
use super::{
    Annotations, Shape, ShapeKind, eraser, filter, line, magnifier, number, spotlight, text,
};
use gpui_kit::{Pixels, Point};

impl Shape {
    fn rasterize_ellipse(
        &self,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        let rx = f32::from(self.bounds.size.width) * scale / 2.;
        let ry = f32::from(self.bounds.size.height) * scale / 2.;
        if rx <= 0. || ry <= 0. {
            return;
        }
        let cx = f32::from(self.bounds.left() - origin.x) * scale + rx;
        let cy = f32::from(self.bounds.top() - origin.y) * scale + ry;
        let inner_rx = rx - self.size() * scale;
        let inner_ry = ry - self.size() * scale;
        let left = (cx - rx).floor().clamp(0., w as f32) as usize;
        let right = (cx + rx).ceil().clamp(0., w as f32) as usize;
        let top = (cy - ry).floor().clamp(0., h as f32) as usize;
        let bottom = (cy + ry).ceil().clamp(0., h as f32) as usize;
        let color = self.color.to_be_bytes();
        let mut coverage = vec![0_f32; right - left];
        for row in top..bottom {
            coverage.fill(0.);
            // Analytic horizontal coverage and eight vertical samples smooth the edge
            // without testing every subpixel of the entire bounding rectangle.
            for sample in 0..8 {
                let dy = row as f32 + (sample as f32 + 0.5) / 8. - cy;
                if dy.abs() >= ry {
                    continue;
                }
                let outer = rx * (1. - (dy / ry).powi(2)).sqrt();
                let inner = if inner_rx > 0. && inner_ry > 0. && dy.abs() < inner_ry {
                    inner_rx * (1. - (dy / inner_ry).powi(2)).sqrt()
                } else {
                    0.
                };
                for (start, end) in [(cx - outer, cx - inner), (cx + inner, cx + outer)] {
                    let start_col = (start.floor().max(left as f32) as usize).min(right);
                    let end_col = (end.ceil().max(left as f32) as usize).min(right);
                    for col in start_col..end_col {
                        coverage[col - left] +=
                            (end.min(col as f32 + 1.) - start.max(col as f32)).max(0.) / 8.;
                    }
                }
            }
            for (index, coverage) in coverage.iter().copied().enumerate() {
                let offset = (row * w as usize + left + index) * 4;
                if rgba[offset + 3] == 0 || coverage == 0. {
                    continue;
                }
                let coverage = coverage.min(1.);
                for channel in 0..3 {
                    rgba[offset + channel] = (rgba[offset + channel] as f32 * (1. - coverage)
                        + color[channel] as f32 * coverage)
                        .round() as u8;
                }
            }
        }
    }
}

impl Annotations {
    pub(crate) fn rasterize(
        &self,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        Self::rasterize_shapes(self.visible(), rgba, w, h, origin, scale);
    }

    pub(crate) fn rasterize_shapes<'a>(
        shapes: impl Iterator<Item = &'a Shape>,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        let shapes: Vec<_> = shapes.collect();
        let original = shapes
            .iter()
            .any(|s| {
                matches!(
                    s.kind,
                    ShapeKind::Magnifier | ShapeKind::Eraser | ShapeKind::EraserRect
                )
            })
            .then(|| rgba.to_vec());
        spotlight::rasterize(
            shapes
                .iter()
                .copied()
                .filter(|s| s.kind == ShapeKind::Spotlight),
            rgba,
            w,
            h,
            origin,
            scale,
        );
        if let Some(original) = original {
            // Sampling excludes lenses, but display compositing retains creation
            // order. Marks added over a lens must remain above that lens.
            let mut marked = rgba.to_vec();
            Self::rasterize_ordinary(
                shapes.iter().copied(),
                &mut marked,
                Some(&original),
                w,
                h,
                origin,
                scale,
            );
            for shape in shapes {
                if let Some(lens) = shape.lens() {
                    let source = if lens.annotations { &marked } else { &original };
                    magnifier::rasterize(shape, rgba, source, w, h, origin, scale);
                } else {
                    Self::rasterize_ordinary(
                        std::iter::once(shape),
                        rgba,
                        Some(&original),
                        w,
                        h,
                        origin,
                        scale,
                    );
                }
            }
        } else {
            Self::rasterize_ordinary(shapes.into_iter(), rgba, None, w, h, origin, scale);
        }
    }

    pub(super) fn rasterize_ordinary<'a>(
        shapes: impl Iterator<Item = &'a Shape>,
        rgba: &mut [u8],
        original: Option<&[u8]>,
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        for shape in shapes {
            if matches!(shape.kind, ShapeKind::Eraser | ShapeKind::EraserRect) {
                if let Some(original) = original {
                    eraser::rasterize(shape, rgba, original, w, h, origin, scale);
                }
                continue;
            }
            if shape.kind == ShapeKind::Spotlight {
                spotlight::rasterize_outline(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Magnifier {
                continue;
            }
            if shape.has_custom_geometry() {
                line::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Text {
                text::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if matches!(shape.kind, ShapeKind::Mosaic | ShapeKind::Blur) {
                filter::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Number {
                number::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if matches!(
                shape.kind,
                ShapeKind::Line
                    | ShapeKind::Arrow
                    | ShapeKind::Polyline
                    | ShapeKind::Pencil
                    | ShapeKind::Highlighter
            ) {
                line::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Ellipse {
                shape.rasterize_ellipse(rgba, w, h, origin, scale);
                continue;
            }
            let color = shape.color.to_be_bytes();
            for stroke in shape.strokes() {
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
                for row in y(stroke.top())..y(stroke.bottom()) {
                    for col in x(stroke.left())..x(stroke.right()) {
                        let offset = (row * w as usize + col) * 4;
                        // Preserve transparent gaps between desktop outputs.
                        if rgba[offset + 3] != 0 {
                            rgba[offset..offset + 4].copy_from_slice(&color);
                        }
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

    fn selection() -> Bounds<Pixels> {
        Bounds::new(point(px(-20.), px(0.)), size(px(100.), px(100.)))
    }

    fn rectangle(a: &mut Annotations) {
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.drag_to(point(px(30.), px(40.)), selection(), false);
        a.end();
    }

    #[test]
    fn pencil_click_exports_round_dot_at_fractional_scales() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Pencil);
        a.set_tool_size(5.);
        a.begin(point(px(20.), px(20.)), selection(), false);
        a.end();
        assert_eq!(a.visible().count(), 1);
        assert_eq!(
            a.visible()
                .next()
                .unwrap()
                .line_paths(point(px(0.), px(0.)))
                .len(),
            1
        );
        for scale in [1., 1.25, 1.7, 2.] {
            let mut pixels = vec![255; 100 * 100 * 4];
            let center = (20. * scale) as usize;
            // Transparent gaps between screens must remain transparent.
            pixels[(center * 100 + center + 1) * 4 + 3] = 0;
            a.rasterize(&mut pixels, 100, 100, point(px(0.), px(0.)), scale);
            let at = |x, y| &pixels[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
            assert_eq!(at(center, center), a.color().0.to_be_bytes());
            assert_eq!(at(center + 1, center)[3], 0);
            assert_eq!(at(center + 5, center + 5), [255; 4]);
        }
    }

    #[test]
    fn raster_strokes_preserve_interior_and_transparent_gaps() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        let mut pixels = [10, 20, 30, 255].repeat(100 * 100);
        let gap = (10 * 100 + 15) * 4;
        pixels[gap..gap + 4].fill(0);
        a.rasterize(&mut pixels, 100, 100, point(px(0.), px(0.)), 1.);
        assert_eq!(
            &pixels[(10 * 100 + 10) * 4..(10 * 100 + 10) * 4 + 4],
            &a.color().0.to_be_bytes()
        );
        assert_eq!(
            &pixels[(20 * 100 + 20) * 4..(20 * 100 + 20) * 4 + 4],
            &[10, 20, 30, 255]
        );
        assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
    }

    #[test]
    fn ellipse_raster_has_smooth_edges_and_preserves_hole_corners_and_gaps() {
        for scale in [1., 1.25, 1.5, 1.73, 2.] {
            let ellipse = super::Shape {
                number: None,
                text: None,
                text_style: Default::default(),
                kind: super::ShapeKind::Ellipse,
                bounds: Bounds::new(point(px(-10.), px(15.)), size(px(60.), px(40.))),
                color: 0xff0000ff,
                points: Vec::new(),
                parameters: crate::annotation::ShapeParameters::new(super::ShapeKind::Ellipse, 3.),
            };
            let w = (100. * scale) as u32;
            let mut pixels = [0, 0, 0, 255].repeat((w * w) as usize);
            let gap_x = (40. * scale) as usize;
            let gap_y = (16. * scale) as usize;
            let gap = (gap_y * w as usize + gap_x) * 4;
            pixels[gap..gap + 4].fill(0);
            ellipse.rasterize_ellipse(&mut pixels, w, w, point(px(-20.), px(0.)), scale);
            let pixel = |x: f32, y: f32| {
                let offset = (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
                &pixels[offset..offset + 4]
            };
            assert_eq!(pixel(40., 35.), &[0, 0, 0, 255], "center at {scale}");
            assert_eq!(pixel(11., 16.), &[0, 0, 0, 255], "corner at {scale}");
            assert_eq!(pixel(11., 35.), &[255, 0, 0, 255], "left edge at {scale}");
            assert_eq!(pixel(68., 35.), &[255, 0, 0, 255], "right edge at {scale}");
            assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255)
            );
            assert!(ellipse.ellipse_path(point(px(0.), px(0.))).is_some());
        }
    }

    #[test]
    fn tiny_and_clipped_ellipses_render_without_invalid_geometry() {
        for (width, height) in [(0., 0.), (2., 2.), (2., 60.), (60., 2.), (60., 40.)] {
            let ellipse = super::Shape {
                number: None,
                text: None,
                text_style: Default::default(),
                kind: super::ShapeKind::Ellipse,
                bounds: Bounds::new(point(px(-10.), px(-10.)), size(px(width), px(height))),
                color: 0xff0000ff,
                points: Vec::new(),
                parameters: crate::annotation::ShapeParameters::new(super::ShapeKind::Ellipse, 5.),
            };
            let mut pixels = [0, 0, 0, 255].repeat(400);
            ellipse.rasterize_ellipse(&mut pixels, 20, 20, point(px(0.), px(0.)), 1.25);
            assert_eq!(
                ellipse.ellipse_path(point(px(0.), px(0.))).is_some(),
                width > 0.
            );
        }
    }

    #[test]
    fn line_raster_preserves_gaps_and_has_round_caps_at_fractional_scales() {
        for scale in [1., 1.25, 1.73, 2.] {
            let shape = super::Shape {
                number: None,
                text: None,
                text_style: Default::default(),
                kind: super::ShapeKind::Polyline,
                bounds: selection(),
                points: vec![
                    point(px(10.), px(30.)),
                    point(px(60.), px(30.)),
                    point(px(60.), px(60.)),
                ],
                parameters: crate::annotation::ShapeParameters::new(super::ShapeKind::Polyline, 5.),
                color: 0xff0000ff,
            };
            let w = (100. * scale) as u32;
            let mut rgba = [0, 0, 0, 255].repeat((w * w) as usize);
            let at =
                |x: f32, y: f32| (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
            let gap = at(40., 30.);
            rgba[gap..gap + 4].fill(0);
            super::line::rasterize(&shape, &mut rgba, w, w, point(px(0.), px(0.)), scale);
            for (x, y) in [(30., 30.), (60., 30.), (60., 50.)] {
                assert_eq!(&rgba[at(x, y)..at(x, y) + 4], &[255, 0, 0, 255]);
            }
            assert!(rgba[at(8., 30.)] > 200, "round cap at scale {scale}");
            assert_eq!(&rgba[gap..gap + 4], &[0; 4]);
            assert_eq!(&rgba[at(30., 40.)..at(30., 40.) + 4], &[0, 0, 0, 255]);
            assert!(
                rgba.as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255)
            );
            assert_eq!(shape.line_paths(point(px(0.), px(0.))).len(), 2);
        }
    }
}
