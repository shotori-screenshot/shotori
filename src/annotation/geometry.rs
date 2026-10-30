//! Filled and patterned geometry shares polygons across preview, export and picking.
use super::{Annotations, HistoryEntry, LineStyle, Shape, ShapeKind, ShapeParameters};
use gpui_kit::{Pixels, Point, point, px};

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

    pub(crate) fn rotated_point(&self, p: Point<Pixels>, inverse: bool) -> Point<Pixels> {
        let center = self.bounds.center();
        let angle = self.geometry_transform().rotation * if inverse { -1. } else { 1. };
        let (sin, cos) = angle.sin_cos();
        let d = p - center;
        center + point(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
    }

    pub(crate) fn can_rotate(&self) -> bool {
        matches!(self.kind, ShapeKind::Rectangle | ShapeKind::Ellipse)
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

    pub(crate) fn geometry_fill(&self) -> bool {
        matches!(
            self.parameters,
            ShapeParameters::Geometry { fill: true, .. }
        )
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
        let from = before.geometry_transform().rotation;
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
            if let ShapeParameters::Geometry { transform, .. } = &mut shape.parameters {
                transform.rotation = from + (target - from) * fraction;
            }
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

    pub(crate) fn geometry_fill(&self) -> bool {
        self.selected()
            .map(Shape::geometry_fill)
            .unwrap_or(self.geometry_fill)
    }

    pub(crate) fn geometry_radius(&self) -> f32 {
        self.selected()
            .map(Shape::geometry_radius)
            .unwrap_or(self.geometry_radius)
    }

    pub(crate) fn set_geometry_fill(&mut self, value: bool) {
        self.geometry_fill = value;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Geometry { fill, .. } = &mut self.shapes[ix].parameters {
                *fill = value;
            }
            self.commit_move(ix, before);
        }
    }

    /// Adjust rectangle corners independently of stroke width, merging one slider gesture.
    pub(crate) fn set_geometry_radius(&mut self, value: f32) {
        if self.edit_kind() != Some(ShapeKind::Rectangle) || !value.is_finite() {
            return;
        }
        let value = value.clamp(0., Self::MAX_CORNER_RADIUS);
        self.geometry_radius = value;
        let Some(ix) = self.selected_index() else {
            return;
        };
        let before = self.shapes[ix].clone();
        if let ShapeParameters::Geometry { radius, .. } = &mut self.shapes[ix].parameters {
            *radius = value;
        }
        if self.shapes[ix] == before {
            return;
        }
        if self.size_drag_active
            && let Some(HistoryEntry::Edit {
                ix: previous,
                after,
                ..
            }) = self.history.last_mut()
            && *previous == ix
        {
            *after = self.shapes[ix].clone();
        } else {
            self.commit_move(ix, before);
        }
        self.redo.clear();
        self.size_drag_active = true;
    }

    pub(crate) const MAX_CORNER_RADIUS: f32 = 128.;
}

pub(super) fn polygons(shape: &Shape) -> Vec<Vec<Point<Pixels>>> {
    let width = f32::from(shape.bounds.size.width).max(0.);
    let height = f32::from(shape.bounds.size.height).max(0.);
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let filled = shape.geometry_fill();
    let stroke = shape.size().min(width).min(height);
    let inset = if filled { 0. } else { stroke / 2. };
    let x = f32::from(shape.bounds.left()) + inset;
    let y = f32::from(shape.bounds.top()) + inset;
    let w = (width - inset * 2.).max(0.);
    let h = (height - inset * 2.).max(0.);
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
            for i in (0..=steps).rev() {
                let (sin, cos) = (t.start + i as f32 * t.sweep / steps as f32).sin_cos();
                contour.push(point(
                    px(x + w / 2. * (1. + cos * t.inner)),
                    px(y + h / 2. * (1. + sin * t.inner)),
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

fn positive_sweep(angle: f32) -> f32 {
    let sweep = angle.rem_euclid(std::f32::consts::TAU);
    if sweep < 0.001 {
        std::f32::consts::TAU
    } else {
        sweep
    }
}

/// Triangles keep concave sectors and ring holes valid for both coverage engines.
fn tessellate_fill(contour: &[Point<Pixels>]) -> Vec<Vec<Point<Pixels>>> {
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
}
