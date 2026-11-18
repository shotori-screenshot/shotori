//! Filled and patterned geometry shares polygons across preview, export and picking.
use super::parameters::GeometryFill;
use super::{Annotations, LineStyle, Shape, ShapeKind, ShapeParameters};
use gpui_kit::{Path, Pixels, Point, point, px};

/// Ellipse angles use clockwise radians in the mark's unrotated coordinate system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GeometryTransform {
    pub(super) rotation: f32,
    pub(super) start: f32,
    pub(super) sweep: f32,
    pub(super) inner: f32,
}

impl Default for GeometryTransform {
    fn default() -> Self {
        Self {
            rotation: 0.,
            start: 0.,
            sweep: std::f32::consts::TAU,
            inner: 0.,
        }
    }
}

impl Shape {
    pub(super) fn geometry_transform(&self) -> GeometryTransform {
        match self.parameters {
            ShapeParameters::Geometry { transform, .. } => transform,
            _ => GeometryTransform::default(),
        }
    }

    pub(crate) fn rotation(&self) -> f32 {
        match self.parameters {
            ShapeParameters::Geometry { transform, .. } => transform.rotation,
            _ if self.kind == ShapeKind::Text => self.text_style.rotation,
            _ => 0.,
        }
    }

    pub(super) fn set_rotation(&mut self, rotation: f32) {
        match &mut self.parameters {
            ShapeParameters::Geometry { transform, .. } => transform.rotation = rotation,
            _ if self.kind == ShapeKind::Text => self.text_style.rotation = rotation,
            _ => {}
        }
    }

    pub(crate) fn rotated_point(&self, p: Point<Pixels>, inverse: bool) -> Point<Pixels> {
        let rotation = self.rotation();
        if rotation == 0. {
            return p;
        }
        let center = self.bounds.center();
        let angle = rotation * if inverse { -1. } else { 1. };
        let (sin, cos) = angle.sin_cos();
        let d = p - center;
        center + point(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
    }

    pub(crate) fn can_rotate(&self) -> bool {
        matches!(
            self.kind,
            ShapeKind::Rectangle | ShapeKind::Ellipse | ShapeKind::Text
        )
    }

    pub(crate) fn frame_corners(&self) -> [Point<Pixels>; 4] {
        let b = if self.can_rotate() {
            super::select::inflate(&self.bounds, px(3.))
        } else {
            self.selection_box()
        };
        [
            b.origin,
            point(b.right(), b.top()),
            b.bottom_right(),
            point(b.left(), b.bottom()),
        ]
        .map(|p| self.rotated_point(p, false))
    }

    pub(super) fn rotation_hit(&self, p: Point<Pixels>) -> bool {
        if !self.can_rotate() {
            return false;
        }
        let p = self.rotated_point(p, true);
        let frame = super::select::inflate(&self.bounds, px(3.));
        super::select::inflate(&frame, px(6.)).contains(&p)
            && !super::select::inflate(&frame, px(-3.)).contains(&p)
    }

    pub(super) fn geometry_handles(&self) -> Vec<Point<Pixels>> {
        let b = self.bounds;
        let center = b.center();
        let mut handles = vec![
            b.origin,
            point(b.right(), b.top()),
            b.bottom_right(),
            point(b.left(), b.bottom()),
            point(center.x, b.top()),
            point(b.right(), center.y),
            point(center.x, b.bottom()),
            point(b.left(), center.y),
        ];
        if self.kind == ShapeKind::Rectangle {
            if b.size.width >= px(16.) && b.size.height >= px(16.) {
                let r = px(self
                    .geometry_radius()
                    .max(12.)
                    .min(f32::from(b.size.width.min(b.size.height)) / 2.));
                handles.extend([
                    b.origin + point(r, r),
                    point(b.right() - r, b.top() + r),
                    b.bottom_right() - point(r, r),
                    point(b.left() + r, b.bottom() - r),
                ]);
            }
        } else {
            let t = self.geometry_transform();
            let at = |angle: f32, radius: f32| {
                let (sin, cos) = angle.sin_cos();
                center
                    + point(
                        b.size.width * (cos * radius / 2.),
                        b.size.height * (sin * radius / 2.),
                    )
            };
            handles.push(at(t.start, 0.6));
            if t.sweep < std::f32::consts::TAU - 0.001 {
                handles.push(at(t.start + t.sweep, 0.6));
                handles.push(at(t.start + t.sweep / 2., t.inner));
            }
        }
        handles
            .into_iter()
            .map(|p| self.rotated_point(p, false))
            .collect()
    }

    pub(super) fn set_geometry_handle(&mut self, anchor: usize, p: Point<Pixels>) {
        let p = self.rotated_point(p, true);
        let b = self.bounds;
        if anchor < 8 {
            let opposite = match anchor {
                0 => b.bottom_right(),
                1 => point(b.left(), b.bottom()),
                2 => b.origin,
                3 => point(b.right(), b.top()),
                4 => point(b.center().x, b.bottom()),
                5 => point(b.left(), b.center().y),
                6 => point(b.center().x, b.top()),
                _ => point(b.right(), b.center().y),
            };
            let mut min = point(p.x.min(opposite.x), p.y.min(opposite.y));
            let mut max = point(p.x.max(opposite.x), p.y.max(opposite.y));
            if anchor == 4 || anchor == 6 {
                min.x = b.left();
                max.x = b.right();
            }
            if anchor == 5 || anchor == 7 {
                min.y = b.top();
                max.y = b.bottom();
            }
            let local_center = (min + max) / 2.;
            let world_center = self.rotated_point(local_center, false);
            self.bounds = gpui_kit::Bounds::from_corners(min, max);
            self.bounds.origin += world_center - local_center;
            return;
        }
        if let ShapeParameters::Geometry {
            radius, transform, ..
        } = &mut self.parameters
        {
            if self.kind == ShapeKind::Rectangle {
                let corner = match anchor {
                    8 => b.origin,
                    9 => point(b.right(), b.top()),
                    10 => b.bottom_right(),
                    _ => point(b.left(), b.bottom()),
                };
                *radius = ((f32::from(p.x - corner.x).abs() + f32::from(p.y - corner.y).abs())
                    / 2.)
                    .min(f32::from(b.size.width.min(b.size.height)) / 2.);
            } else {
                let d = p - b.center();
                let x = f32::from(d.x) / (f32::from(b.size.width) / 2.).max(0.001);
                let y = f32::from(d.y) / (f32::from(b.size.height) / 2.).max(0.001);
                let angle = y.atan2(x).rem_euclid(std::f32::consts::TAU);
                if anchor == 10 {
                    transform.inner = x.hypot(y).clamp(0., 0.95);
                } else if anchor == 8 {
                    let end = transform.start + transform.sweep;
                    transform.start = angle;
                    transform.sweep = positive_sweep(end - angle);
                } else {
                    transform.sweep = positive_sweep(angle - transform.start);
                }
            }
        }
    }

    pub(crate) fn geometry_fill_mode(&self) -> GeometryFill {
        match self.parameters {
            ShapeParameters::Geometry { fill, .. } => fill,
            _ => GeometryFill::None,
        }
    }

    pub(crate) fn geometry_fill(&self) -> bool {
        self.geometry_fill_mode() != GeometryFill::None
    }

    pub(crate) fn geometry_radius(&self) -> f32 {
        match self.parameters {
            ShapeParameters::Geometry { radius, .. } => radius,
            _ => 0.,
        }
    }

    pub(crate) fn has_custom_geometry(&self) -> bool {
        matches!(self.kind, ShapeKind::Rectangle | ShapeKind::Ellipse)
            && (self.geometry_fill()
                || self.geometry_radius() > 0.
                || self.line_style() != LineStyle::Solid
                || self.geometry_transform() != GeometryTransform::default())
    }
}

impl Annotations {
    pub(crate) fn rotate_shape(
        &mut self,
        ix: usize,
        before: &Shape,
        press: Point<Pixels>,
        p: Point<Pixels>,
        area: gpui_kit::Bounds<Pixels>,
        snap: bool,
    ) {
        if !before.can_rotate() {
            return;
        }
        let center = before.bounds.center();
        let angle_at =
            |p: Point<Pixels>| f32::from(p.y - center.y).atan2(f32::from(p.x - center.x));
        let from = before.rotation();
        let delta = (angle_at(p) - angle_at(press) + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let target = if snap {
            ((from + delta) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4
        } else {
            from + delta
        };
        let limit = area.union(&before.selection_box());
        let Some(shape) = self.shapes.get_mut(ix) else {
            return;
        };
        let mut apply = |fraction| {
            *shape = before.clone();
            shape.set_rotation(from + (target - from) * fraction);
            let b = shape.selection_box();
            b.left() >= limit.left()
                && b.right() <= limit.right()
                && b.top() >= limit.top()
                && b.bottom() <= limit.bottom()
        };
        if apply(1.) {
            return;
        }
        let (mut low, mut high) = (0., 1.);
        for _ in 0..20 {
            let fraction = (low + high) / 2.;
            if apply(fraction) {
                low = fraction;
            } else {
                high = fraction;
            }
        }
        apply(low);
    }

    pub(crate) fn geometry_fill_mode(&self) -> GeometryFill {
        self.selected()
            .filter(|s| matches!(s.kind, ShapeKind::Rectangle | ShapeKind::Ellipse))
            .map(Shape::geometry_fill_mode)
            .unwrap_or(self.geometry_fill)
    }

    pub(crate) fn geometry_radius(&self) -> f32 {
        self.selected()
            .map(Shape::geometry_radius)
            .unwrap_or(self.geometry_radius)
    }

    #[cfg(test)]
    pub(crate) fn set_geometry_fill(&mut self, value: bool) {
        self.set_geometry_fill_mode(if value {
            GeometryFill::Solid
        } else {
            GeometryFill::None
        });
    }

    pub(crate) fn set_geometry_fill_mode(&mut self, value: GeometryFill) {
        self.geometry_fill = value;
        self.edit_selected(|shape| {
            if let ShapeParameters::Geometry { fill, .. } = &mut shape.parameters {
                *fill = value;
            }
        });
    }

    /// Adjust rectangle corners independently of stroke width, merging one slider gesture.
    pub(crate) fn set_geometry_radius(&mut self, value: f32) {
        if self.edit_kind() != Some(ShapeKind::Rectangle) || !value.is_finite() {
            return;
        }
        let value = value.clamp(0., Self::MAX_CORNER_RADIUS);
        self.geometry_radius = value;
        self.edit_selected_drag(|before| {
            let mut next = before.clone();
            if next.kind == ShapeKind::Rectangle
                && let ShapeParameters::Geometry { radius, .. } = &mut next.parameters
            {
                *radius = value;
            }
            next
        });
    }

    pub(crate) const MAX_CORNER_RADIUS: f32 = 128.;
}

pub(super) fn polygons(shape: &Shape) -> Vec<Vec<Point<Pixels>>> {
    polygons_with_mode(shape, shape.geometry_fill_mode() == GeometryFill::Solid, 0.)
}

/// Interior wash polygons for `GeometryFill::Translucent`, inset inside solid
/// borders so the stroke and wash never double-blend along their shared edge.
pub(super) fn translucent_fill_polygons(shape: &Shape) -> Vec<Vec<Point<Pixels>>> {
    let inset = if shape.line_style() == LineStyle::Solid {
        shape
            .size()
            .min(f32::from(shape.bounds.size.width))
            .min(f32::from(shape.bounds.size.height))
    } else {
        0.
    };
    polygons_with_mode(shape, true, inset)
}

/// Outline stroke polygons for `GeometryFill::Translucent`.
pub(super) fn outline_polygons(shape: &Shape) -> Vec<Vec<Point<Pixels>>> {
    polygons_with_mode(shape, false, 0.)
}

fn polygons_with_mode(shape: &Shape, filled: bool, extra_inset: f32) -> Vec<Vec<Point<Pixels>>> {
    let width = f32::from(shape.bounds.size.width).max(0.);
    let height = f32::from(shape.bounds.size.height).max(0.);
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let stroke = shape.size().min(width).min(height);
    let inset = if filled { extra_inset } else { stroke / 2. };
    let x = f32::from(shape.bounds.left()) + inset;
    let y = f32::from(shape.bounds.top()) + inset;
    let w = (width - inset * 2.).max(0.);
    let h = (height - inset * 2.).max(0.);
    if w <= 0. || h <= 0. {
        return Vec::new();
    }
    let mut contour = Vec::new();
    if shape.kind == ShapeKind::Ellipse {
        let steps = ((w + h) * std::f32::consts::PI / 2.)
            .ceil()
            .clamp(32., 512.) as usize;
        let t = shape.geometry_transform();
        for i in 0..=steps {
            let (sin, cos) = (t.start + i as f32 * t.sweep / steps as f32).sin_cos();
            contour.push(point(
                px(x + w / 2. * (1. + cos)),
                px(y + h / 2. * (1. + sin)),
            ));
        }
        if t.inner > 0. {
            let inner_rx = (w / 2. * t.inner + if filled { extra_inset } else { 0. }).min(w / 2.);
            let inner_ry = (h / 2. * t.inner + if filled { extra_inset } else { 0. }).min(h / 2.);
            for i in (0..=steps).rev() {
                let (sin, cos) = (t.start + i as f32 * t.sweep / steps as f32).sin_cos();
                contour.push(point(
                    px(x + w / 2. + cos * inner_rx),
                    px(y + h / 2. + sin * inner_ry),
                ));
            }
        } else if t.sweep < std::f32::consts::TAU - 0.001 {
            contour.push(point(px(x + w / 2.), px(y + h / 2.)));
        }
    } else {
        let radius = (shape.geometry_radius() - inset)
            .max(0.)
            .min(w / 2.)
            .min(h / 2.);
        for (cx, cy, angle) in [
            (x + w - radius, y + radius, -std::f32::consts::FRAC_PI_2),
            (x + w - radius, y + h - radius, 0.),
            (x + radius, y + h - radius, std::f32::consts::FRAC_PI_2),
            (x + radius, y + radius, std::f32::consts::PI),
        ] {
            for i in 0..=16 {
                let (sin, cos) = (angle + i as f32 * std::f32::consts::FRAC_PI_2 / 16.).sin_cos();
                contour.push(point(px(cx + cos * radius), px(cy + sin * radius)));
            }
        }
    }
    let mut contour: Vec<_> = contour
        .into_iter()
        .map(|p| shape.rotated_point(p, false))
        .collect();
    if filled {
        return tessellate_fill(&contour);
    }
    if let Some(first) = contour.first().copied() {
        contour.push(first);
    }
    super::line::patterned_geometry(&contour, stroke, shape.line_style())
}

/// Paint the fill mesh together so internal edges share one antialiasing pass.
pub(super) fn filled_path(shape: &Shape, offset: Point<Pixels>) -> Option<Path<Pixels>> {
    let triangles = polygons(shape);
    let start = triangles.first()?.first()?;
    let mut path = Path::new(*start + offset);
    for triangle in triangles {
        if let [a, b, c] = triangle.as_slice() {
            path.push_triangle(
                (*a + offset, *b + offset, *c + offset),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
    }
    Some(path)
}

fn positive_sweep(angle: f32) -> f32 {
    let sweep = angle.rem_euclid(std::f32::consts::TAU);
    if sweep < 0.001 {
        std::f32::consts::TAU
    } else {
        sweep
    }
}

/// Triangles keep concave sectors and ring holes valid for both coverage engines.
pub(super) fn tessellate_fill(contour: &[Point<Pixels>]) -> Vec<Vec<Point<Pixels>>> {
    use lyon_tessellation::path::{Path, math::point as vertex};
    use lyon_tessellation::{
        BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers,
    };
    let Some(first) = contour.first() else {
        return Vec::new();
    };
    let mut path = Path::builder();
    path.begin(vertex(f32::from(first.x), f32::from(first.y)));
    for p in &contour[1..] {
        path.line_to(vertex(f32::from(p.x), f32::from(p.y)));
    }
    path.close();
    let mut mesh: VertexBuffers<Point<Pixels>, u32> = VertexBuffers::new();
    let result = FillTessellator::new().tessellate_path(
        &path.build(),
        &FillOptions::default(),
        &mut BuffersBuilder::new(&mut mesh, |v: FillVertex<'_>| {
            point(px(v.position().x), px(v.position().y))
        }),
    );
    if let Err(error) = result {
        eprintln!("Annotation fill tessellation failed: {error:?}");
        return Vec::new();
    }
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| tri.iter().map(|&i| mesh.vertices[i as usize]).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Bounds, size};

    fn filled_geometries() -> Vec<Shape> {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(180.)));
        let mut shapes = Vec::new();
        for (kind, sweep, inner) in [
            (ShapeKind::Rectangle, std::f32::consts::TAU, 0.),
            (ShapeKind::Ellipse, std::f32::consts::TAU, 0.),
            (ShapeKind::Ellipse, std::f32::consts::TAU, 0.5),
            (ShapeKind::Ellipse, 2., 0.),
            (ShapeKind::Ellipse, 4.5, 0.5),
        ] {
            for rotation in [0., 0.37] {
                let mut marks = Annotations::default();
                marks.toggle(kind);
                marks.set_geometry_fill(true);
                marks.begin(point(px(40.25), px(44.5)), area, false);
                marks.drag_to(point(px(160.25), px(124.5)), area, false);
                marks.end();
                let mut shape = marks.shapes.remove(0);
                shape.color = 0xD91C26FF;
                if let ShapeParameters::Geometry {
                    radius, transform, ..
                } = &mut shape.parameters
                {
                    *radius = 18.;
                    *transform = GeometryTransform {
                        rotation,
                        start: 0.,
                        sweep,
                        inner,
                    };
                }
                shapes.push(shape);
            }
        }
        shapes
    }

    #[test]
    fn filled_preview_batches_the_export_mesh_into_one_path() {
        let offset = point(px(17.25), px(-3.5));
        for shape in filled_geometries() {
            let paths = super::super::line::paths(&shape, offset);
            assert_eq!(paths.len(), 1, "{:?}", shape.parameters);
            let expected: Vec<_> = polygons(&shape).into_iter().flatten().collect();
            assert!(expected.len() > 3);
            assert_eq!(paths[0].vertices.len(), expected.len());
            for (vertex, expected) in paths[0].vertices.iter().zip(expected) {
                assert_eq!(vertex.xy_position, expected + offset);
                assert_eq!(vertex.st_position, point(0., 1.));
            }
        }
    }

    #[test]
    fn filled_export_has_solid_interiors_and_preserves_holes_at_fractional_scales() {
        for shape in filled_geometries() {
            for scale in [1., 1.25, 2.] {
                let w = (200. * scale) as u32;
                let h = (180. * scale) as u32;
                let mut pixels = vec![255; (w * h * 4) as usize];
                super::super::line::rasterize(
                    &shape,
                    &mut pixels,
                    w,
                    h,
                    point(px(0.), px(0.)),
                    scale,
                );
                for y in 0..h {
                    for x in 0..w {
                        let local = shape.rotated_point(
                            point(px((x as f32 + 0.5) / scale), px((y as f32 + 0.5) / scale)),
                            true,
                        ) - shape.bounds.center();
                        let dx = f32::from(local.x);
                        let dy = f32::from(local.y);
                        let inside = if shape.kind == ShapeKind::Rectangle {
                            let qx = dx.abs() - 42.;
                            let qy = dy.abs() - 22.;
                            let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - 18.;
                            if distance.abs() < 2. {
                                continue;
                            }
                            distance < 0.
                        } else {
                            let radius = (dx / 60.).hypot(dy / 40.);
                            let t = shape.geometry_transform();
                            if (radius - 1.).abs() < 0.1 || (radius - t.inner).abs() < 0.1 {
                                continue;
                            }
                            let angle =
                                (dy / 40.).atan2(dx / 60.).rem_euclid(std::f32::consts::TAU);
                            if t.sweep < std::f32::consts::TAU
                                && (angle < 0.15
                                    || std::f32::consts::TAU - angle < 0.15
                                    || (angle - t.sweep).abs() < 0.15)
                            {
                                continue;
                            }
                            radius < 1. && radius > t.inner && angle < t.sweep
                        };
                        let expected = if inside {
                            [0xD9, 0x1C, 0x26, 255]
                        } else {
                            [255; 4]
                        };
                        let offset = ((y * w + x) * 4) as usize;
                        assert_eq!(
                            pixels[offset..offset + 4],
                            expected,
                            "{:?}, scale {scale}, ({x}, {y})",
                            shape.parameters,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn corner_radius_never_changes_the_stroke_slider_or_wheel_target() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(240.), px(200.)));
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Rectangle);
        marks.apply_size(4.);
        marks.set_geometry_radius(12.);
        assert_eq!(marks.current_edit_size(), 4.);
        let spec = marks.current_size_spec();
        let width_spec = super::super::size_spec(ShapeKind::Rectangle);
        assert_eq!((spec.min, spec.max), (width_spec.min, width_spec.max));
        marks.begin(point(px(20.), px(20.)), area, false);
        marks.drag_to(point(px(180.), px(140.)), area, false);
        marks.end();
        marks.select_index(0);
        let original = marks.shapes[0].clone();
        marks.set_geometry_radius(18.);
        marks.set_geometry_radius(24.);
        marks.end_size_drag();
        assert_eq!(marks.current_edit_size(), 4.);
        assert!(marks.step_size(true));
        assert_eq!(marks.current_edit_size(), 5.);
        assert_eq!(marks.geometry_radius(), 24.);
        marks.undo();
        assert_eq!(marks.shapes[0].size(), 4.);
        assert_eq!(marks.shapes[0].geometry_radius(), 24.);
        marks.undo();
        assert_eq!(marks.shapes[0], original);
        marks.redo();
        marks.redo();
        assert_eq!(marks.shapes[0].size(), 5.);
        assert_eq!(marks.shapes[0].geometry_radius(), 24.);
        marks.toggle(ShapeKind::Ellipse);
        marks.apply_size(7.);
        marks.set_geometry_radius(80.);
        assert_eq!(marks.current_edit_size(), 7.);
        marks.toggle(ShapeKind::Rectangle);
        assert_eq!(marks.current_edit_size(), 5.);
        assert_eq!(marks.geometry_radius(), 24.);
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            marks.set_geometry_radius(invalid);
            assert_eq!(marks.geometry_radius(), 24.);
        }
    }

    #[test]
    fn fills_patterns_and_corners_are_reversible_and_match_pick_regions() {
        for kind in [ShapeKind::Rectangle, ShapeKind::Ellipse] {
            let mut marks = Annotations::default();
            marks.toggle(kind);
            let area = Bounds::new(point(px(0.), px(0.)), size(px(180.), px(140.)));
            marks.begin(point(px(20.), px(20.)), area, false);
            marks.drag_to(point(px(160.), px(120.)), area, false);
            marks.end();
            marks.select_index(0);
            let original = marks.shapes[0].clone();
            marks.set_geometry_fill(true);
            let center = point(px(90.), px(70.));
            assert!(super::super::select::shape_hit(&marks.shapes[0], center));
            assert!(super::super::select::shape_erased(
                &marks.shapes[0],
                center,
                1.
            ));
            for scale in [1., 1.25, 2.] {
                let w = (180. * scale) as u32;
                let h = (140. * scale) as u32;
                let mut pixels = vec![255; (w * h * 4) as usize];
                marks.rasterize(&mut pixels, w, h, area.origin, scale);
                let offset = (((70. * scale) as u32 * w + (90. * scale) as u32) * 4) as usize;
                assert_ne!(&pixels[offset..offset + 3], &[255; 3]);
            }
            marks.undo();
            assert_eq!(marks.shapes[0], original);
            marks.redo();
            marks.select_index(0);
            marks.set_geometry_fill(false);
            if kind == ShapeKind::Rectangle {
                marks.set_geometry_radius(18.);
                marks.set_geometry_radius(24.);
                marks.end_size_drag();
                assert_eq!(marks.shapes[0].geometry_radius(), 24.);
                assert_eq!(marks.shapes[0].size(), original.size());
                marks.undo();
                assert_eq!(marks.shapes[0].geometry_radius(), 0.);
                marks.redo();
            }
            marks.select_index(0);
            marks.set_line_style(LineStyle::DashDotDot);
            let mark = &marks.shapes[0];
            assert!(!super::super::select::shape_hit(mark, center));
            for polygon in polygons(mark) {
                for p in polygon {
                    assert!(
                        p.x >= mark.bounds.left() - px(0.01)
                            && p.x <= mark.bounds.right() + px(0.01)
                    );
                    assert!(
                        p.y >= mark.bounds.top() - px(0.01)
                            && p.y <= mark.bounds.bottom() + px(0.01)
                    );
                }
            }
        }
    }

    #[test]
    fn translucent_fill_tints_interior_with_full_strength_border() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(160.), px(120.)));
        for kind in [ShapeKind::Rectangle, ShapeKind::Ellipse] {
            let mut marks = Annotations::default();
            marks.toggle(kind);
            marks.apply_size(4.);
            marks.set_geometry_fill_mode(GeometryFill::Translucent);
            marks.begin(point(px(20.), px(20.)), area, false);
            marks.drag_to(point(px(140.), px(100.)), area, false);
            marks.end();
            let mut pixels = vec![255; 160 * 120 * 4];
            marks.rasterize(&mut pixels, 160, 120, area.origin, 1.);
            let center = (60 * 160 + 80) * 4;
            // Interior is tinted, neither untouched white nor fully opaque ink.
            assert!(
                pixels[center + 1] < 255 && pixels[center + 1] > 150,
                "{kind:?}"
            );
            // Border stroke reaches full ink strength without double-blending.
            let edge = (21 * 160 + 80) * 4;
            assert!(pixels[edge + 1] < 80, "{kind:?}");
        }
    }

    #[test]
    fn rotated_text_updates_ink_bounds_picking_and_upright_editing() {
        crate::annotation::text::with_test_font(|| {
            let area = Bounds::new(point(px(0.), px(0.)), size(px(320.), px(260.)));
            let mut marks = Annotations::default();
            marks.add_text(
                Bounds::new(point(px(90.), px(100.)), size(px(140.), px(36.))),
                "Rotated label".into(),
            );
            marks.select_index(0);
            let before = marks.shapes[0].clone();
            assert!(before.can_rotate());
            marks.rotate_shape(
                0,
                &before,
                point(px(230.), px(118.)),
                point(px(160.), px(188.)),
                area,
                true,
            );
            marks.commit_move(0, before.clone());
            let rotated = marks.shapes[0].clone();
            assert!((rotated.rotation() - std::f32::consts::FRAC_PI_2).abs() < 0.01);
            assert!(rotated.ink_bounds().size.height > before.ink_bounds().size.height);
            // Unrotated left tail is outside the 90°-rotated oriented box.
            assert!(!super::super::select::shape_hit(
                &rotated,
                point(px(95.), px(118.))
            ));
            assert!(super::super::select::shape_hit(
                &rotated,
                rotated.bounds.center()
            ));

            let mut pixels = vec![255; 320 * 260 * 4];
            marks.rasterize(&mut pixels, 320, 260, area.origin, 1.);
            // Ink extends vertically past the unrotated 36px box height without clipping
            // (check channel +1 because palette swatch 0 is #ff3b30 with R == 255).
            let has_vertical_ink =
                (50..88).any(|y| (140..180).any(|x| pixels[(y * 320 + x) * 4 + 1] < 200));
            assert!(
                has_vertical_ink,
                "rotated text top half must not be clipped"
            );

            // Inline editing straightens upright while typing, then restores rotation.
            assert!(marks.begin_text_edit(0));
            assert_eq!(marks.shapes[0].rotation(), 0.);
            marks.preview_text(before.bounds, "Edited rotated".into());
            marks.finish_text_edit(true);
            assert!((marks.shapes[0].rotation() - rotated.rotation()).abs() < 0.001);
            marks.undo();
            assert_eq!(marks.shapes[0], rotated);
        });
    }
}
