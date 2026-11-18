//! Selection chrome geometry: the selection frame and the handle
//! anchors a selected shape paints. Kept next to the shape data (not
//! in the overlay) so both preview paths render identical chrome and
//! future geometry editing (issue #5 phase C) grows on the same
//! anchors. Pure geometry — no state.
use gpui_kit::{Bounds, PathBuilder, Pixels, Point, point, px, size};

use super::{Shape, ShapeKind};

/// Clearance between the ink and the selection frame (logical px).
/// Keeps the dashes off the colored edge they frame (they must stay
/// legible against the same palette that hid the old on-ink outline)
/// and gives degenerate ink — a horizontal line, a single tap — a
/// frame with visible height/width.
const FRAME_PAD: f32 = 3.;

/// Minimum side length (logical px) before a region's 1D edge-midpoint
/// stretch handle appears along that axis. Thin shapes keep only their
/// four corners so midpoint dots do not crowd the corners (issue #33).
const MIN_EDGE_HANDLE_SPAN: f32 = 48.;

/// Minimum side length (logical px) before a rectangle's inset corner-radius
/// handles are active; smaller boxes adjust radius via the toolbar slider so
/// the 12 px inset dots never collide with the center move zone.
const MIN_RADIUS_HANDLE_SPAN: f32 = 36.;

/// Pointer proximity (logical px) to a rectangle corner or its inset radius
/// anchor that reveals that single corner's radius handle (issue #33).
const RADIUS_REVEAL_DISTANCE: f32 = 36.;

/// Visual hierarchy tier for an editable annotation handle (issue #33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandleRole {
    /// Primary geometry anchor: region corners, line/arrow endpoints,
    /// polyline vertices, and sequence leader-arrow tips.
    Primary,
    /// 1D edge-stretch midpoint on region shapes.
    Edge,
    /// Shape-parameter handle: rectangle corner radius, ellipse sweep/inner
    /// ring, or line/arrow curve bend point.
    Modifier,
}

impl Shape {
    /// Visible ink bounds, including stroke caps and arrowhead wings.
    pub(crate) fn ink_bounds(&self) -> Bounds<Pixels> {
        if let Some(super::FilterArea::Brush { diameter }) = self.filter_area() {
            let mut stroke = self.clone();
            stroke.kind = ShapeKind::Pencil;
            stroke.parameters = super::ShapeParameters::new(ShapeKind::Pencil, diameter);
            return ink_aabb(&stroke);
        }
        if let Some(arrow) = self.number_arrow() {
            return self.bounds.union(&arrow.ink_bounds());
        }
        if let Some(bounds) = self.magnified_bounds() {
            return bounds.union(&self.bounds);
        }
        if self.has_custom_geometry() {
            return ink_aabb(self);
        }
        if self.kind == ShapeKind::Text && self.rotation() != 0. {
            let b = self.bounds;
            let corners = [
                b.origin,
                point(b.right(), b.top()),
                b.bottom_right(),
                point(b.left(), b.bottom()),
            ]
            .map(|p| self.rotated_point(p, false));
            let mut min = corners[0];
            let mut max = corners[0];
            for c in &corners[1..] {
                min = point(min.x.min(c.x), min.y.min(c.y));
                max = point(max.x.max(c.x), max.y.max(c.y));
            }
            return Bounds::from_corners(min, max);
        }
        match self.kind {
            ShapeKind::Pencil
            | ShapeKind::Highlighter
            | ShapeKind::Polyline
            | ShapeKind::Line
            | ShapeKind::Arrow => ink_aabb(self),
            _ => self.bounds,
        }
    }

    /// The selection frame encloses visible ink with space for a dashed outline.
    pub(crate) fn selection_box(&self) -> Bounds<Pixels> {
        super::select::inflate(&self.ink_bounds(), px(FRAME_PAD))
    }

    /// Handle anchors for the selection chrome — the points where a
    /// grab makes sense for geometry editing: endpoints and bend point of a
    /// line/arrow, every vertex of a polyline, the four corners of
    /// region marks (TL, TR, BR, BL), and an attached sequence arrow tip.
    /// Freehand strokes, plain badges, text and filters get the frame alone.
    pub(crate) fn handle_points(&self) -> Vec<Point<Pixels>> {
        match self.kind {
            ShapeKind::Number => self.number_tip().into_iter().collect(),
            ShapeKind::Line | ShapeKind::Arrow => {
                let mut handles: Vec<_> = self.points.iter().take(2).copied().collect();
                if handles.len() == 2 {
                    handles.push(
                        self.points
                            .get(2)
                            .copied()
                            .unwrap_or((handles[0] + handles[1]) / 2.),
                    );
                }
                handles
            }
            ShapeKind::Polyline => self.points.clone(),
            ShapeKind::Rectangle | ShapeKind::Ellipse => self.geometry_handles(),
            ShapeKind::Magnifier | ShapeKind::Spotlight => vec![
                point(self.bounds.left(), self.bounds.top()),
                point(self.bounds.right(), self.bounds.top()),
                point(self.bounds.right(), self.bounds.bottom()),
                point(self.bounds.left(), self.bounds.bottom()),
            ],
            ShapeKind::Mosaic | ShapeKind::Blur
                if matches!(
                    self.filter_area(),
                    Some(super::FilterArea::Rectangle | super::FilterArea::Ellipse)
                ) =>
            {
                let b = self.bounds;
                let c = b.center();
                vec![
                    b.origin,
                    point(b.right(), b.top()),
                    b.bottom_right(),
                    point(b.left(), b.bottom()),
                    point(c.x, b.top()),
                    point(b.right(), c.y),
                    point(c.x, b.bottom()),
                    point(b.left(), c.y),
                ]
            }
            ShapeKind::Highlighter if self.highlighter_options().rectangle => {
                let b = self.bounds;
                let c = b.center();
                vec![
                    b.origin,
                    point(b.right(), b.top()),
                    b.bottom_right(),
                    point(b.left(), b.bottom()),
                    point(c.x, b.top()),
                    point(b.right(), c.y),
                    point(c.x, b.bottom()),
                    point(b.left(), c.y),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// Role of handle `anchor` in [`Shape::handle_points`].
    pub(crate) fn handle_role(&self, anchor: usize) -> HandleRole {
        match self.kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => match anchor {
                0..=3 => HandleRole::Primary,
                4..=7 => HandleRole::Edge,
                _ => HandleRole::Modifier,
            },
            ShapeKind::Mosaic | ShapeKind::Blur | ShapeKind::Highlighter => match anchor {
                0..=3 => HandleRole::Primary,
                _ => HandleRole::Edge,
            },
            ShapeKind::Line | ShapeKind::Arrow if anchor == 2 => HandleRole::Modifier,
            _ => HandleRole::Primary,
        }
    }

    fn handle_enabled(&self, anchor: usize, active_anchor: Option<usize>) -> bool {
        if active_anchor == Some(anchor) {
            return true;
        }
        match self.kind {
            ShapeKind::Rectangle
            | ShapeKind::Ellipse
            | ShapeKind::Mosaic
            | ShapeKind::Blur
            | ShapeKind::Highlighter => match anchor {
                4 | 6 => self.bounds.size.width >= px(MIN_EDGE_HANDLE_SPAN),
                5 | 7 => self.bounds.size.height >= px(MIN_EDGE_HANDLE_SPAN),
                8..=11 if self.kind == ShapeKind::Rectangle => {
                    self.bounds.size.width >= px(MIN_RADIUS_HANDLE_SPAN)
                        && self.bounds.size.height >= px(MIN_RADIUS_HANDLE_SPAN)
                }
                _ => true,
            },
            _ => true,
        }
    }

    /// Which inset corner-radius handle (`8..=11`) on a selected rectangle
    /// should be revealed for `pointer` (in the same coordinate space as
    /// `self`) or an in-flight `active_anchor` drag. At rest (`None`), all
    /// four radius dots stay hidden so only primary/edge handles show.
    pub(crate) fn revealed_radius_anchor(
        &self,
        pointer: Option<Point<Pixels>>,
        active_anchor: Option<usize>,
    ) -> Option<usize> {
        if self.kind != ShapeKind::Rectangle {
            return None;
        }
        if let Some(anchor) = active_anchor.filter(|a| (8..=11).contains(a)) {
            return Some(anchor);
        }
        if active_anchor.is_some()
            || self.bounds.size.width < px(MIN_RADIUS_HANDLE_SPAN)
            || self.bounds.size.height < px(MIN_RADIUS_HANDLE_SPAN)
        {
            return None;
        }
        let ptr = pointer?;
        let pts = self.handle_points();
        if pts.len() < 12 {
            return None;
        }
        (0..4usize)
            .map(|corner| {
                let d_corner =
                    f32::from(ptr.x - pts[corner].x).hypot(f32::from(ptr.y - pts[corner].y));
                let d_radius = f32::from(ptr.x - pts[8 + corner].x)
                    .hypot(f32::from(ptr.y - pts[8 + corner].y));
                (8 + corner, d_corner.min(d_radius))
            })
            .filter(|(_, dist)| *dist <= RADIUS_REVEAL_DISTANCE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(anchor, _)| anchor)
    }

    /// Handles to paint in the selection HUD for `pointer` (same coordinate
    /// space as `self`) and any active handle drag `active_anchor`.
    pub(crate) fn visible_handles(
        &self,
        pointer: Option<Point<Pixels>>,
        active_anchor: Option<usize>,
    ) -> Vec<(Point<Pixels>, HandleRole)> {
        let pts = self.handle_points();
        let revealed_radius = self.revealed_radius_anchor(pointer, active_anchor);
        pts.into_iter()
            .enumerate()
            .filter(|&(ix, _)| {
                self.handle_enabled(ix, active_anchor)
                    && !(self.kind == ShapeKind::Rectangle
                        && (8..=11).contains(&ix)
                        && revealed_radius != Some(ix))
            })
            .map(|(ix, p)| (p, self.handle_role(ix)))
            .collect()
    }

    /// Whether `p` grabs one of the handles; returns its anchor index
    /// (same indexing as [`Shape::handle_points`]). The grab radius
    /// comfortably exceeds the painted dot's radius (`HANDLE_VIS / 2`,
    /// see `ui::hud::paint_handle_dot`), so the small dot stays easy to
    /// catch — what you see and what you can grab are separate budgets.
    pub(crate) fn handle_at(&self, p: Point<Pixels>) -> Option<usize> {
        const GRAB_RADIUS: f32 = 10.;
        const RADIUS_HANDLE_GRAB: f32 = 7.5;
        const CORNER_PRIORITY_RADIUS: f32 = 8.;
        let pts = self.handle_points();
        if self.kind == ShapeKind::Rectangle
            && let Some((ix, _)) = pts
                .iter()
                .take(4)
                .enumerate()
                .map(|(ix, h)| (ix, f32::from(p.x - h.x).hypot(f32::from(p.y - h.y))))
                .filter(|(_, d)| *d <= CORNER_PRIORITY_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            return Some(ix);
        }
        pts.iter()
            .enumerate()
            .filter(|&(ix, _)| self.handle_enabled(ix, None))
            .map(|(ix, h)| {
                let limit = if self.kind == ShapeKind::Rectangle && (8..=11).contains(&ix) {
                    RADIUS_HANDLE_GRAB
                } else {
                    GRAB_RADIUS
                };
                (ix, f32::from(p.x - h.x).hypot(f32::from(p.y - h.y)), limit)
            })
            .filter(|(_, distance, limit)| distance <= limit)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(ix, _, _)| ix)
            .or_else(|| self.rotation_hit(p).then_some(super::ROTATION_HANDLE))
    }

    /// Re-derive the geometry with handle `anchor` placed at `p` —
    /// the phase-C edit. Endpoints/vertices move in place; corners
    /// re-normalize the bounds around the opposite corner, so dragging
    /// through the anchor flips the rectangle like drawing did. The bend handle
    /// is an interpolated point on the curve and can move independently of endpoints.
    pub(crate) fn set_handle(&mut self, anchor: usize, p: Point<Pixels>) {
        match self.kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => self.set_geometry_handle(anchor, p),
            ShapeKind::Number if anchor == 0 => self.set_number_tip(Some(p)),
            ShapeKind::Line | ShapeKind::Arrow if self.points.len() >= 2 => {
                if anchor == 2 {
                    if self.points.len() == 2 {
                        self.points.push(p);
                    } else {
                        self.points[2] = p;
                    }
                } else if let Some(handle) = self.points.get_mut(anchor) {
                    *handle = p;
                }
            }
            ShapeKind::Polyline => {
                if let Some(q) = self.points.get_mut(anchor) {
                    *q = p;
                }
            }
            ShapeKind::Magnifier
            | ShapeKind::Spotlight
            | ShapeKind::Mosaic
            | ShapeKind::Blur
            | ShapeKind::Highlighter => {
                let b = self.bounds;
                if anchor >= 8 {
                    return;
                }
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
                let mut x0 = opposite.x.min(p.x);
                let mut x1 = opposite.x.max(p.x);
                let mut y0 = opposite.y.min(p.y);
                let mut y1 = opposite.y.max(p.y);
                if anchor == 4 || anchor == 6 {
                    x0 = b.left();
                    x1 = b.right();
                }
                if anchor == 5 || anchor == 7 {
                    y0 = b.top();
                    y1 = b.bottom();
                }
                self.bounds = Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0));
            }
            _ => {}
        }
    }
}

/// The AABB of everything `line::geometry` would paint for a
/// point-carried shape. Absorbs the raw points first so the result is
/// never empty even when geometry degenerates to nothing (a
/// zero-length line/arrow), then the polygons' vertices for the true
/// ink extent.
fn ink_aabb(shape: &Shape) -> Bounds<Pixels> {
    let mut aabb: Option<Bounds<Pixels>> = None;
    let mut absorb = |p: Point<Pixels>| {
        let dot = Bounds::new(p, size(px(0.), px(0.)));
        aabb = Some(match aabb {
            Some(b) => b.union(&dot),
            None => dot,
        });
    };
    for p in &shape.points {
        absorb(*p);
    }
    for poly in super::line::shape_geometry(shape) {
        for p in poly {
            absorb(p);
        }
    }
    aabb.unwrap_or(shape.bounds)
}

/// Append one eight-arc cubic ellipse contour to a builder (either
/// fill or stroke mode). `direction` flips the winding to cut holes.
/// Shared by the export ring ([`Shape::ellipse_path`]).
pub(super) fn ellipse_contour(
    builder: &mut PathBuilder,
    center: Point<Pixels>,
    rx: f32,
    ry: f32,
    direction: f32,
) {
    let step = direction * std::f32::consts::TAU / 8.;
    let k = 4. / 3. * (step / 4.).tan();
    let at = |x, y| center + point(px(rx * x), px(ry * y));
    builder.move_to(at(1., 0.));
    for i in 0..8 {
        let (s0, c0) = (i as f32 * step).sin_cos();
        let (s1, c1) = ((i + 1) as f32 * step).sin_cos();
        builder.cubic_bezier_to(
            at(c1, s1),
            at(c0 - k * s0, s0 + k * c0),
            at(c1 + k * s1, s1 - k * c1),
        );
    }
    builder.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(kind: ShapeKind, points: &[(f32, f32)]) -> Shape {
        Shape {
            kind,
            number: None,
            text: None,
            text_style: Default::default(),
            bounds: Bounds::default(),
            color: 0xffd43b60,
            parameters: crate::annotation::ShapeParameters::new(kind, 20.),
            points: points.iter().map(|&(x, y)| point(px(x), px(y))).collect(),
        }
    }

    #[test]
    fn larger_handles_pick_the_nearest_node_when_hit_regions_overlap() {
        let mark = shape(ShapeKind::Line, &[(20., 20.), (100., 20.), (26., 24.)]);
        assert_eq!(mark.handle_at(point(px(28.), px(25.))), Some(2));
        assert_eq!(mark.handle_at(point(px(12.), px(20.))), Some(0));
        assert_eq!(mark.handle_at(point(px(9.), px(20.))), None);
    }

    /// Containment with slack: the sampled n-gon rims hit axis-aligned
    /// extremes exactly for horizontal/vertical strokes but undershoot
    /// them by up to ~0.5% of the radius on diagonals, so "covers"
    /// tolerates a tenth of a pixel.
    fn covers(b: &Bounds<Pixels>, x: f32, y: f32) -> bool {
        const S: f32 = 0.1;
        b.left() <= px(x + S)
            && b.right() >= px(x - S)
            && b.top() <= px(y + S)
            && b.bottom() >= px(y - S)
    }

    #[test]
    fn arrow_frame_covers_the_head_wings_not_just_the_endpoints() {
        // (10,50)→(110,50), width 20 → head 60, wings at base (50,50)
        // reaching ±27 perpendicular. An endpoints-plus-half-width box
        // would stop at y=40/60 and clip the wings; the ink box must
        // not.
        let mut mark = shape(ShapeKind::Arrow, &[(10., 50.), (110., 50.)]);
        if let crate::annotation::ShapeParameters::Stroke { arrow, .. } = &mut mark.parameters {
            *arrow = crate::annotation::ArrowStyle::Triangle;
        }
        let b = mark.selection_box();
        for (x, y) in [(10., 50.), (110., 50.), (50., 23.), (50., 77.)] {
            assert!(covers(&b, x, y), "ink point ({x},{y}) outside frame {b:?}");
        }
        // …and it must not be absurdly larger than the ink either
        // (the whole extent is x∈[0,110], y∈[23,77]).
        assert!(b.left() >= px(-4.) && b.right() <= px(114.));
        assert!(b.top() >= px(19.) && b.bottom() <= px(81.));
    }

    #[test]
    fn horizontal_line_frame_has_visible_height() {
        // Degenerate ink: the AABB of the endpoints is zero-height;
        // the capsule radius (10) plus pad keeps the frame a band, not
        // a line.
        let b = shape(ShapeKind::Line, &[(30., 30.), (90., 30.)]).selection_box();
        assert!(covers(&b, 20., 30.) && covers(&b, 100., 30.));
        assert!(b.top() <= px(17.) && b.bottom() >= px(43.));
        assert!(b.size.height >= px(26.));
    }

    #[test]
    fn single_point_tap_frames_its_dot() {
        let b = shape(ShapeKind::Pencil, &[(30., 30.)]).selection_box();
        assert!(covers(&b, 20., 30.) && covers(&b, 40., 30.));
    }

    #[test]
    fn freehand_frame_spans_every_point_even_though_bounds_stays_stale() {
        // Freehand drags grow `points` and never update `bounds` (it
        // stays the 0×0 start box) — the frame must come from the
        // points' ink, proving `bounds` is unused on this path.
        let b = shape(
            ShapeKind::Pencil,
            &[(10., 30.), (40., 10.), (70., 30.), (90., 50.)],
        )
        .selection_box();
        for (x, y) in [(0., 20.), (100., 40.), (40., 0.), (70., 60.)] {
            assert!(covers(&b, x, y), "ink point ({x},{y}) outside frame {b:?}");
        }
    }

    #[test]
    fn bounds_carried_shapes_frame_their_bounds_plus_pad() {
        let mut s = shape(ShapeKind::Rectangle, &[]);
        s.bounds = Bounds::new(point(px(10.), px(20.)), size(px(80.), px(60.)));
        assert_eq!(
            s.selection_box(),
            Bounds::new(point(px(7.), px(17.)), size(px(86.), px(66.)))
        );
    }

    #[test]
    fn rectangle_handles_hide_radius_at_rest_reveal_single_corner_and_drop_thin_midpoints() {
        let mut rect = shape(ShapeKind::Rectangle, &[]);
        rect.bounds = Bounds::new(point(px(100.), px(100.)), size(px(120.), px(80.)));

        // At rest (no pointer near a corner): 4 primary corners + 4 edge midpoints, 0 radius dots.
        let idle = rect.visible_handles(None, None);
        assert_eq!(idle.len(), 8);
        assert_eq!(
            idle.iter()
                .filter(|(_, r)| *r == HandleRole::Primary)
                .count(),
            4
        );
        assert_eq!(
            idle.iter().filter(|(_, r)| *r == HandleRole::Edge).count(),
            4
        );
        assert_eq!(
            idle.iter()
                .filter(|(_, r)| *r == HandleRole::Modifier)
                .count(),
            0
        );

        // Hovering near top-left corner reveals ONLY top-left's radius handle (anchor 8).
        assert_eq!(
            rect.revealed_radius_anchor(Some(point(px(108.), px(108.))), None),
            Some(8)
        );
        let hovered = rect.visible_handles(Some(point(px(108.), px(108.))), None);
        assert_eq!(hovered.len(), 9);
        assert_eq!(
            hovered
                .iter()
                .filter(|(_, r)| *r == HandleRole::Modifier)
                .count(),
            1
        );

        // Corner click priority: aiming near (105, 105) grabs corner 0, while
        // aiming at the inset radius dot (112, 112) grabs radius handle 8.
        assert_eq!(rect.handle_at(point(px(105.), px(105.))), Some(0));
        assert_eq!(rect.handle_at(point(px(112.), px(112.))), Some(8));

        // Active radius drag keeps that handle visible even when the pointer moves far away.
        let dragging = rect.visible_handles(Some(point(px(300.), px(300.))), Some(10));
        assert_eq!(
            dragging
                .iter()
                .filter(|(_, r)| *r == HandleRole::Modifier)
                .count(),
            1
        );

        // Thin horizontal banner (120×28): vertical edge midpoints (left/right) and
        // inset radius handles drop out so corners and body move stay uncluttered.
        rect.bounds = Bounds::new(point(px(100.), px(100.)), size(px(120.), px(28.)));
        let thin = rect.visible_handles(Some(point(px(105.), px(105.))), None);
        assert_eq!(thin.len(), 6); // 4 corners + top/bottom midpoints only
        assert_eq!(rect.handle_at(point(px(100.), px(114.))), None);
        assert_eq!(rect.handle_at(point(px(112.), px(112.))), None);
    }
}
