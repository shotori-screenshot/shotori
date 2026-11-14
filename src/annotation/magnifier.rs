//! Editable source and destination geometry for pixel magnification.
use super::{Annotations, Shape, ShapeKind, ShapeParameters};
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

#[derive(Clone, Copy, Debug)]
pub(crate) enum LensOption {
    Ellipse,
    Smooth,
    Annotations,
    Shadow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LensConnector {
    Center,
    CenterDot,
    Border,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Lens {
    pub zoom: f32,
    pub offset: Point<Pixels>,
    pub ellipse: bool,
    pub smooth: bool,
    pub annotations: bool,
    pub connector: LensConnector,
    pub border: f32,
    pub shadow: bool,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            zoom: 2.,
            offset: point(px(0.), px(0.)),
            ellipse: false,
            smooth: true,
            annotations: true,
            connector: LensConnector::Center,
            border: 2.,
            shadow: false,
        }
    }
}

impl Shape {
    pub(super) fn lens(&self) -> Option<Lens> {
        match self.parameters {
            ShapeParameters::Magnifier(lens) => Some(lens),
            _ => None,
        }
    }

    pub(crate) fn lens_source_outline(&self) -> Option<Shape> {
        let lens = self.lens()?;
        let mut outline = self.clone();
        outline.kind = if lens.ellipse {
            ShapeKind::Ellipse
        } else {
            ShapeKind::Rectangle
        };
        outline.parameters = ShapeParameters::new(outline.kind, 1.);
        Some(outline)
    }

    pub(crate) fn magnified_bounds(&self) -> Option<Bounds<Pixels>> {
        let lens = self.lens()?;
        let extent = size(
            self.bounds.size.width * lens.zoom,
            self.bounds.size.height * lens.zoom,
        );
        Some(Bounds::new(
            self.bounds.center() + lens.offset - point(extent.width / 2., extent.height / 2.),
            extent,
        ))
    }

    /// Place the enlarged image beside the source, preferring a fully visible side.
    /// Large sources may overlap when the selection cannot contain both regions.
    pub(super) fn place_lens_beside_source(&mut self, selection: Bounds<Pixels>) {
        const GAP: f32 = 16.;
        let Some(destination) = self.magnified_bounds() else {
            return;
        };
        let extent = destination.size;
        let source = self.bounds;
        let centered = source.center() - point(extent.width / 2., extent.height / 2.);
        let candidates = [
            point(source.right() + px(GAP), centered.y),
            point(source.left() - extent.width - px(GAP), centered.y),
            point(centered.x, source.bottom() + px(GAP)),
            point(centered.x, source.top() - extent.height - px(GAP)),
        ];
        let mut best = None;
        for candidate in candidates {
            let origin = point(
                candidate.x.clamp(
                    selection.left(),
                    (selection.right() - extent.width).max(selection.left()),
                ),
                candidate.y.clamp(
                    selection.top(),
                    (selection.bottom() - extent.height).max(selection.top()),
                ),
            );
            let rect = Bounds::new(origin, extent);
            let overlap = (rect.right().min(source.right()) - rect.left().max(source.left()))
                .max(px(0.))
                * f32::from(
                    (rect.bottom().min(source.bottom()) - rect.top().max(source.top())).max(px(0.)),
                );
            let shift = candidate - origin;
            let score = (
                f32::from(overlap),
                f32::from(shift.x).abs() + f32::from(shift.y).abs(),
            );
            if best.is_none_or(|(_, previous)| score < previous) {
                best = Some((rect.center(), score));
            }
        }
        if let Some((center, _)) = best
            && let ShapeParameters::Magnifier(lens) = &mut self.parameters
        {
            lens.offset = center - source.center();
        }
    }

    pub(crate) fn move_lens(&mut self, delta: Point<Pixels>) {
        if let ShapeParameters::Magnifier(lens) = &mut self.parameters {
            lens.offset += delta;
        }
    }
}

impl Annotations {
    /// Scroll over either lens region changes zoom; the surrounding canvas is inert.
    pub(crate) fn step_size_at(&mut self, position: Point<Pixels>, up: bool) -> bool {
        let Some(shape) = self.selected().filter(|s| s.kind == ShapeKind::Magnifier) else {
            return self.step_size(up);
        };
        let ellipse = shape.lens().is_some_and(|lens| lens.ellipse);
        let over_image = inside(shape.bounds, position, ellipse)
            || shape
                .magnified_bounds()
                .is_some_and(|bounds| inside(bounds, position, ellipse));
        over_image && self.step_size(up)
    }

    pub(crate) fn lens_shadow(&self) -> bool {
        self.selected()
            .and_then(Shape::lens)
            .unwrap_or(self.lens)
            .shadow
    }

    pub(crate) fn move_selected_lens(&mut self, delta: Point<Pixels>) {
        if let Some(ix) = self.selected_index() {
            self.shapes[ix].move_lens(delta);
        }
    }

    pub(crate) fn lens_options(&self) -> (bool, bool, bool, LensConnector) {
        let lens = self.selected().and_then(Shape::lens).unwrap_or(self.lens);
        (lens.ellipse, lens.smooth, lens.annotations, lens.connector)
    }

    pub(crate) fn set_lens_connector(&mut self, value: LensConnector) {
        self.lens.connector = value;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Magnifier(lens) = &mut self.shapes[ix].parameters {
                lens.connector = value;
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn set_lens_option(&mut self, option: LensOption, enabled: bool) {
        let apply = |lens: &mut Lens| match option {
            LensOption::Ellipse => lens.ellipse = enabled,
            LensOption::Smooth => lens.smooth = enabled,
            LensOption::Annotations => lens.annotations = enabled,
            LensOption::Shadow => lens.shadow = enabled,
        };
        apply(&mut self.lens);
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Magnifier(lens) = &mut self.shapes[ix].parameters {
                apply(lens);
            }
            self.commit_move(ix, before);
        }
    }
}

pub(super) fn contains(shape: &Shape, p: Point<Pixels>) -> bool {
    let Some(bounds) = shape.magnified_bounds() else {
        return false;
    };
    let Some(lens) = shape.lens() else {
        return false;
    };
    inside(bounds, p, lens.ellipse) || inside(shape.bounds, p, lens.ellipse)
}

fn inside(bounds: Bounds<Pixels>, p: Point<Pixels>, ellipse: bool) -> bool {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return false;
    }
    if !ellipse {
        return bounds.contains(&p);
    }
    let delta = p - bounds.center();
    (f32::from(delta.x) / f32::from(bounds.size.width / 2.)).powi(2)
        + (f32::from(delta.y) / f32::from(bounds.size.height / 2.)).powi(2)
        <= 1.
}

/// Intersect a center ray with the rectangular or elliptical region boundary.
fn boundary_towards(bounds: Bounds<Pixels>, target: Point<Pixels>, ellipse: bool) -> Point<Pixels> {
    let delta = target - bounds.center();
    let x = f32::from(delta.x) / f32::from(bounds.size.width / 2.).max(0.001);
    let y = f32::from(delta.y) / f32::from(bounds.size.height / 2.).max(0.001);
    let divisor = if ellipse {
        x.hypot(y)
    } else {
        x.abs().max(y.abs())
    };
    if divisor < 0.001 {
        bounds.center()
    } else {
        bounds.center() + delta / divisor
    }
}

/// Samples an immutable input so an overlapping destination never recursively magnifies itself.
pub(super) fn rasterize(
    shape: &Shape,
    rgba: &mut [u8],
    source: &[u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let Some(lens) = shape.lens() else { return };
    let Some(bounds) = shape.magnified_bounds() else {
        return;
    };
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    let color = shape.color.to_be_bytes();
    // Paint the source outline after capturing pixels, so it never enters the sample.
    if let Some(mut source_outline) = shape.lens_source_outline()
        && lens.border > 0.
    {
        source_outline.parameters.set_size(lens.border);
        Annotations::rasterize_shapes(std::iter::once(&source_outline), rgba, w, h, origin, scale);
    }
    if lens.connector != LensConnector::None && lens.offset != point(px(0.), px(0.)) {
        let mut connector = shape.clone();
        connector.kind = ShapeKind::Line;
        connector.parameters = ShapeParameters::Stroke {
            ends: Default::default(),
            arrow: super::ArrowStyle::default(),
            width: lens.border.max(1.),
            style: super::LineStyle::Solid,
        };
        let start = if lens.connector == LensConnector::Border {
            boundary_towards(shape.bounds, bounds.center(), lens.ellipse)
        } else {
            shape.bounds.center()
        };
        let end = boundary_towards(bounds, shape.bounds.center(), lens.ellipse);
        connector.points = vec![start, end];
        super::line::rasterize(&connector, rgba, w, h, origin, scale);
        if lens.connector == LensConnector::CenterDot {
            let diameter = px((lens.border * 3.).max(6.));
            let mut dot = connector.clone();
            dot.kind = ShapeKind::Ellipse;
            dot.points.clear();
            dot.bounds = Bounds::new(
                start - point(diameter / 2., diameter / 2.),
                size(diameter, diameter),
            );
            dot.parameters = ShapeParameters::Geometry {
                width: 0.,
                fill: true,
                radius: 0.,
                transform: Default::default(),
                style: super::LineStyle::Solid,
            };
            Annotations::rasterize_shapes(std::iter::once(&dot), rgba, w, h, origin, scale);
        }
    }
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
    let inset = px(lens.border);
    let inner = Bounds::new(
        bounds.origin + point(inset, inset),
        size(
            (bounds.size.width - inset * 2.).max(px(0.)),
            (bounds.size.height - inset * 2.).max(px(0.)),
        ),
    );
    for y in top..bottom {
        for x in left..right {
            let p = origin + point(px((x as f32 + 0.5) / scale), px((y as f32 + 0.5) / scale));
            if !inside(bounds, p, lens.ellipse) {
                continue;
            }
            let at = ((y * w + x) * 4) as usize;
            if rgba[at + 3] == 0 {
                continue;
            }
            if !inside(inner, p, lens.ellipse) {
                for c in 0..3 {
                    rgba[at + c] = ((color[c] as u32 * color[3] as u32
                        + rgba[at + c] as u32 * (255 - color[3] as u32)
                        + 127)
                        / 255) as u8;
                }
                continue;
            }
            let sample = shape.bounds.center() + (p - bounds.center()) / lens.zoom;
            let sx = f32::from(sample.x - origin.x) * scale - 0.5;
            let sy = f32::from(sample.y - origin.y) * scale - 0.5;
            let mut total = [0.; 4];
            let positions = if lens.smooth {
                let (fx, fy) = (sx - sx.floor(), sy - sy.floor());
                [
                    (sx.floor(), sy.floor(), (1. - fx) * (1. - fy)),
                    (sx.floor() + 1., sy.floor(), fx * (1. - fy)),
                    (sx.floor(), sy.floor() + 1., (1. - fx) * fy),
                    (sx.floor() + 1., sy.floor() + 1., fx * fy),
                ]
            } else {
                [
                    (sx.round(), sy.round(), 1.),
                    (0., 0., 0.),
                    (0., 0., 0.),
                    (0., 0., 0.),
                ]
            };
            for (xx, yy, weight) in positions {
                if xx < 0. || yy < 0. || xx >= w as f32 || yy >= h as f32 {
                    continue;
                }
                let pixel = &source[((yy as u32 * w + xx as u32) * 4) as usize..][..4];
                let alpha = pixel[3] as f32 * weight;
                for c in 0..3 {
                    total[c] += pixel[c] as f32 * alpha;
                }
                total[3] += alpha;
            }
            if total[3] > 0. {
                for c in 0..3 {
                    let shade = if lens.shadow {
                        let delta = p - bounds.center();
                        let edge = if lens.ellipse {
                            let rx = f32::from(inner.size.width / 2.).max(0.001);
                            let ry = f32::from(inner.size.height / 2.).max(0.001);
                            (1. - (f32::from(delta.x) / rx).hypot(f32::from(delta.y) / ry)).max(0.)
                                * rx.min(ry)
                        } else {
                            f32::from(
                                (p.x - inner.left())
                                    .min(inner.right() - p.x)
                                    .min(p.y - inner.top())
                                    .min(inner.bottom() - p.y),
                            )
                            .max(0.)
                        };
                        1. - 0.22 * (1. - edge / 8.).clamp(0., 1.).powi(2)
                    } else {
                        1.
                    };
                    rgba[at + c] = (total[c] / total[3] * shade).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark() -> Shape {
        Shape {
            kind: ShapeKind::Magnifier,
            number: None,
            text: None,
            text_style: Default::default(),
            bounds: Bounds::new(point(px(12.), px(12.)), size(px(16.), px(16.))),
            color: 0xff0000ff,
            parameters: ShapeParameters::Magnifier(Lens {
                ellipse: false,
                smooth: false,
                connector: LensConnector::None,
                ..Lens::default()
            }),
            points: Vec::new(),
        }
    }

    #[test]
    fn connector_styles_have_distinct_pixels_and_preserve_sampling() {
        let mut shape = mark();
        shape.move_lens(point(px(40.), px(0.)));
        let original = [255; 4].repeat(100 * 80);
        let mut outputs = Vec::new();
        for connector in [
            LensConnector::None,
            LensConnector::Center,
            LensConnector::CenterDot,
            LensConnector::Border,
        ] {
            if let ShapeParameters::Magnifier(lens) = &mut shape.parameters {
                lens.connector = connector;
            }
            let mut pixels = original.clone();
            rasterize(
                &shape,
                &mut pixels,
                &original,
                100,
                80,
                point(px(0.), px(0.)),
                1.,
            );
            let center = (20 * 100 + 60) * 4;
            assert_eq!(&pixels[center..center + 4], &[255; 4]);
            for previous in &outputs {
                assert_ne!(&pixels, previous);
            }
            outputs.push(pixels);
        }
        for ellipse in [false, true] {
            let point = boundary_towards(shape.bounds, point(px(80.), px(20.)), ellipse);
            assert_eq!(point, gpui_kit::point(px(28.), px(20.)));
        }
    }

    #[test]
    fn center_dot_uses_a_solid_shaft_and_a_filled_source_marker() {
        for scale in [1., 1.25, 2.] {
            let mut shape = mark();
            shape.move_lens(point(px(40.), px(0.)));
            let w = (100. * scale) as u32;
            let h = (80. * scale) as u32;
            let original = [255; 4].repeat((w * h) as usize);
            let mut samples = Vec::new();
            for connector in [LensConnector::Center, LensConnector::CenterDot] {
                if let ShapeParameters::Magnifier(lens) = &mut shape.parameters {
                    lens.connector = connector;
                }
                let mut pixels = original.clone();
                rasterize(
                    &shape,
                    &mut pixels,
                    &original,
                    w,
                    h,
                    point(px(0.), px(0.)),
                    scale,
                );
                for x in ((30. * scale) as u32)..((43. * scale) as u32) {
                    let at = (((20. * scale) as u32 * w + x) * 4) as usize;
                    assert_eq!(
                        &pixels[at..at + 4],
                        &[255, 0, 0, 255],
                        "solid connector at {x}"
                    );
                }
                let at = (((22. * scale) as u32 * w + (20. * scale) as u32) * 4) as usize;
                samples.push(pixels[at..at + 4].to_vec());
            }
            assert_eq!(samples[0], [255; 4]);
            // The circular marker's edge is antialiased at fractional scales.
            assert_eq!(samples[1][0], 255);
            assert!(samples[1][1] < 32 && samples[1][2] < 32);
            assert_eq!(samples[1][3], 255);
        }
    }

    #[test]
    fn connector_edits_preserve_other_options_and_history() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Magnifier);
        marks.set_lens_connector(LensConnector::Border);
        let area = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        marks.begin(point(px(20.), px(20.)), area, false);
        marks.drag_to(point(px(40.), px(40.)), area, false);
        marks.end();
        marks.select_index(0);
        let before = marks.shapes[0].clone();
        marks.set_lens_connector(LensConnector::CenterDot);
        assert_eq!(
            marks.lens_options(),
            (false, true, true, LensConnector::CenterDot)
        );
        marks.undo();
        assert_eq!(marks.shapes[0], before);
        marks.redo();
        assert_eq!(marks.lens_options().3, LensConnector::CenterDot);
    }

    #[test]
    fn drawing_places_a_separate_image_and_preserves_it_through_history() {
        let selection = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        for x in [20., 250.] {
            let mut annotations = Annotations::default();
            annotations.toggle(ShapeKind::Magnifier);
            annotations.begin(point(px(x), px(50.)), selection, false);
            annotations.drag_to(point(px(x + 20.), px(70.)), selection, false);
            annotations.end();
            let shape = annotations.committed()[0].clone();
            let destination = shape.magnified_bounds().unwrap();
            assert!(
                destination.right() <= shape.bounds.left()
                    || destination.left() >= shape.bounds.right()
                    || destination.bottom() <= shape.bounds.top()
                    || destination.top() >= shape.bounds.bottom()
            );
            assert!(selection.contains(&destination.origin));
            assert!(destination.right() <= selection.right());
            assert!(destination.bottom() <= selection.bottom());
            let original = [40, 70, 100, 255].repeat(300 * 200);
            let mut pixels = original.clone();
            annotations.rasterize(&mut pixels, 300, 200, selection.origin, 1.);
            // The separate image remains filled with sampled pixels, not an empty outline.
            let center = destination.center();
            let at = ((f32::from(center.y) as usize * 300) + f32::from(center.x) as usize) * 4;
            assert_eq!(&pixels[at..at + 4], &[40, 70, 100, 255]);
            assert_ne!(pixels, original);
            annotations.undo();
            assert!(annotations.committed().is_empty());
            annotations.redo();
            assert_eq!(annotations.committed()[0], shape);
        }
    }

    #[test]
    fn lens_samples_immutable_source_and_keeps_outside_pixels() {
        let mark = mark();
        let original: Vec<u8> = (0..64 * 64)
            .flat_map(|i| [(i % 64) as u8, (i / 64) as u8, 80, 255])
            .collect();
        let mut output = original.clone();
        rasterize(
            &mark,
            &mut output,
            &original,
            64,
            64,
            point(px(0.), px(0.)),
            1.,
        );
        let at = (20 * 64 + 28) * 4;
        assert_eq!(&output[at..at + 4], &[24, 20, 80, 255]);
        assert_eq!(&output[..4], &original[..4]);
        assert_eq!(original[(20 * 64 + 28) * 4], 28);
    }

    #[test]
    fn lens_samples_explicit_layer_and_moves_without_changing_source() {
        let mut mark = mark();
        let original = [20, 40, 60, 255].repeat(64 * 64);
        let mut annotated = [100, 120, 140, 255].repeat(64 * 64);
        let center = (20 * 64 + 20) * 4;
        let marked = annotated.clone();
        rasterize(
            &mark,
            &mut annotated,
            &marked,
            64,
            64,
            point(px(0.), px(0.)),
            1.,
        );
        assert_eq!(&annotated[center..center + 4], &[100, 120, 140, 255]);
        if let ShapeParameters::Magnifier(lens) = &mut mark.parameters {
            lens.annotations = false;
        }
        rasterize(
            &mark,
            &mut annotated,
            &original,
            64,
            64,
            point(px(0.), px(0.)),
            1.,
        );
        assert_eq!(&annotated[center..center + 4], &[20, 40, 60, 255]);
        let source = mark.bounds;
        let destination = mark.magnified_bounds().unwrap();
        mark.move_lens(point(px(10.), px(0.)));
        assert_eq!(mark.bounds, source);
        assert_eq!(
            mark.magnified_bounds().unwrap().origin,
            destination.origin + point(px(10.), px(0.))
        );
    }

    #[test]
    fn lens_samples_later_marks_independent_of_creation_order() {
        let mut lens = mark();
        lens.move_lens(point(px(40.), px(0.)));
        let mut ink = mark();
        ink.kind = ShapeKind::Rectangle;
        ink.parameters = ShapeParameters::Geometry {
            width: 2.,
            fill: true,
            radius: 0.,
            transform: Default::default(),
            style: super::super::LineStyle::Solid,
        };
        ink.color = 0x2468acff;
        let original = [255; 4].repeat(100 * 80);
        let render = |shapes: &[&Shape]| {
            let mut pixels = original.clone();
            Annotations::rasterize_shapes(
                shapes.iter().copied(),
                &mut pixels,
                100,
                80,
                point(px(0.), px(0.)),
                1.,
            );
            pixels
        };
        let later = render(&[&lens, &ink]);
        let earlier = render(&[&ink, &lens]);
        // The source border follows creation order; only the sampled image is invariant.
        for y in 8..32 {
            for x in 48..72 {
                let at = (y * 100 + x) * 4;
                assert_eq!(&later[at..at + 4], &earlier[at..at + 4], "{x},{y}");
            }
        }
        let at = (20 * 100 + 60) * 4;
        assert_eq!(&later[at..at + 4], &[36, 104, 172, 255]);
        assert_eq!(&render(&[&lens])[at..at + 4], &[255; 4]);
        if let ShapeParameters::Magnifier(options) = &mut lens.parameters {
            options.annotations = false;
        }
        assert_eq!(&render(&[&lens, &ink])[at..at + 4], &[255; 4]);
    }

    #[test]
    fn later_marks_cover_the_enlarged_image_and_undo_restores_it() {
        let mut lens = mark();
        lens.move_lens(point(px(40.), px(0.)));
        let mut ink = mark();
        ink.kind = ShapeKind::Rectangle;
        ink.bounds.origin += point(px(40.), px(0.));
        ink.parameters = ShapeParameters::Geometry {
            width: 2.,
            fill: true,
            radius: 0.,
            transform: Default::default(),
            style: super::super::LineStyle::Solid,
        };
        ink.color = 0x2468acff;
        let mut annotations = Annotations::default();
        annotations.record_add(lens);
        annotations.record_add(ink);
        let render = |annotations: &Annotations| {
            let mut pixels = vec![255; 100 * 80 * 4];
            Annotations::rasterize_shapes(
                annotations.committed().iter(),
                &mut pixels,
                100,
                80,
                point(px(0.), px(0.)),
                1.,
            );
            pixels
        };
        let at = (20 * 100 + 60) * 4;
        assert_eq!(&render(&annotations)[at..at + 4], &[36, 104, 172, 255]);
        annotations.undo();
        assert_eq!(&render(&annotations)[at..at + 4], &[255; 4]);
        annotations.redo();
        assert_eq!(&render(&annotations)[at..at + 4], &[36, 104, 172, 255]);
    }

    #[test]
    fn lens_clips_at_fractional_scale_without_filling_transparent_gaps() {
        let mut mark = mark();
        mark.move_lens(point(px(-24.), px(-24.)));
        for scale in [1., 1.25, 2.] {
            let mut pixels = [80, 90, 100, 255].repeat(64 * 64);
            pixels[..4].fill(0);
            let before = pixels.clone();
            rasterize(
                &mark,
                &mut pixels,
                &before,
                64,
                64,
                point(px(-2.), px(-3.)),
                scale,
            );
            assert_eq!(&pixels[..4], &[0; 4]);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(before.as_chunks::<4>().0.iter())
                    .all(|(a, b)| a[3] == b[3])
            );
        }
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn wheel_changes_zoom_only_over_source_or_destination() {
        for ellipse in [false, true] {
            let area = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.)));
            let mut marks = Annotations::default();
            marks.toggle(ShapeKind::Magnifier);
            marks.set_lens_option(LensOption::Ellipse, ellipse);
            marks.begin(point(px(10.), px(30.)), area, false);
            marks.drag_to(point(px(40.), px(60.)), area, false);
            marks.end();
            marks.select_index(0);
            let destination = marks.shapes[0].magnified_bounds().unwrap();
            assert!(marks.step_size_at(destination.center(), true));
            assert_eq!(marks.shapes[0].size(), 3.);
            assert!(!marks.step_size_at(point(px(390.), px(290.)), true));
            assert_eq!(marks.shapes[0].size(), 3.);
            assert_eq!(marks.shapes[0].lens().unwrap().border, 2.);
            let source = marks.shapes[0].bounds.center();
            assert!(marks.step_size_at(source, false));
            assert_eq!(marks.shapes[0].size(), 2.);
            marks.undo();
            assert_eq!(marks.shapes[0].size(), 3.);
            marks.undo();
            assert_eq!(marks.shapes[0].size(), 2.);
            assert_eq!(marks.shapes[0].lens().unwrap().border, 2.);
        }
    }

    #[test]
    fn shadow_edits_preserve_zoom_and_are_reversible() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(220.), px(160.)));
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Magnifier);
        marks.begin(point(px(10.), px(30.)), area, false);
        marks.drag_to(point(px(40.), px(60.)), area, false);
        marks.end();
        marks.select_index(0);
        let mut plain = vec![255; 220 * 160 * 4];
        marks.rasterize(&mut plain, 220, 160, area.origin, 1.);
        marks.set_lens_option(LensOption::Shadow, true);
        assert_eq!(marks.selected_index(), Some(0));
        let mut shadow = vec![255; 220 * 160 * 4];
        marks.rasterize(&mut shadow, 220, 160, area.origin, 1.);
        assert_ne!(plain, shadow);
        let destination = marks.shapes[0].magnified_bounds().unwrap();
        for (i, (a, b)) in plain
            .as_chunks::<4>()
            .0
            .iter()
            .zip(shadow.as_chunks::<4>().0)
            .enumerate()
        {
            if a != b {
                assert!(destination.contains(&point(
                    px((i % 220) as f32 + 0.5),
                    px((i / 220) as f32 + 0.5)
                )));
            }
        }
        marks.undo();
        assert!(!marks.shapes[0].lens().unwrap().shadow);
    }
}
