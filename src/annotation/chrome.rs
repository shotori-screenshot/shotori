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

impl Shape {
    /// Selection frame: the axis-aligned bounding box of the shape's
    /// VISIBLE ink, inflated by [`FRAME_PAD`] — the universal
    /// "this object is selected" affordance (Figma/PowerPoint
    /// language), replacing the 1 px accent stroke that traced the
    /// shape's own outline. That stroke sat ON the ink, in a hue the
    /// annotation palette all but swallowed, so a selected shape read
    /// as unselected (user report, 2026-10-06). Callers paint this as
    /// a dashed accent rectangle (`ui::hud::annotation_chrome`).
    pub(crate) fn selection_box(&self) -> Bounds<Pixels> {
        super::select::inflate(
            &match self.kind {
                // Point-carried shapes: the ink is the union of capsules
                // and arrowheads from the shared geometry source ("what
                // you see is what you box"). Neither `points` alone nor
                // `bounds` will do: a capsule's radius is half the
                // stroke width, an arrowhead's wings reach ~1.8× the
                // stroke width past the segment, and freehand drags grow
                // `points` while never touching `bounds` — which stays
                // the 0×0 box the gesture started from.
                ShapeKind::Pencil
                | ShapeKind::Highlighter
                | ShapeKind::Polyline
                | ShapeKind::Line
                | ShapeKind::Arrow => ink_aabb(self),
                // Bounds-carried shapes: `bounds` already encloses the
                // ink — rect strokes and the ellipse ring are drawn
                // inward from it, the badge circle inscribes it, and
                // text/mosaic/blur fill exactly it.
                _ => self.bounds,
            },
            px(FRAME_PAD),
        )
    }

    /// Handle anchors for the selection chrome — the points where a
    /// grab makes sense for geometry editing: the two endpoints of a
    /// line/arrow, every vertex of a polyline, the four corners of
    /// rectangles/ellipses (TL, TR, BR, BL). Freehand strokes and
    /// content shapes (badges, text, filters) have no per-point editing
    /// semantics and get the frame alone.
    pub(crate) fn handle_points(&self) -> Vec<Point<Pixels>> {
        match self.kind {
            ShapeKind::Line | ShapeKind::Arrow => self.points.iter().take(2).cloned().collect(),
            ShapeKind::Polyline => self.points.clone(),
            ShapeKind::Rectangle | ShapeKind::Ellipse => vec![
                point(self.bounds.left(), self.bounds.top()),
                point(self.bounds.right(), self.bounds.top()),
                point(self.bounds.right(), self.bounds.bottom()),
                point(self.bounds.left(), self.bounds.bottom()),
            ],
            _ => Vec::new(),
        }
    }

    /// Whether `p` grabs one of the handles; returns its anchor index
    /// (same indexing as [`Shape::handle_points`]). The grab radius
    /// comfortably exceeds the painted dot's radius (`HANDLE_VIS / 2`,
    /// see `ui::hud::paint_handle_dot`), so the small dot stays easy to
    /// catch — what you see and what you can grab are separate budgets.
    pub(crate) fn handle_at(&self, p: Point<Pixels>) -> Option<usize> {
        const GRAB_RADIUS: f32 = 7.;
        self.handle_points()
            .iter()
            .position(|h| (f32::from(p.x - h.x)).hypot(f32::from(p.y - h.y)) <= GRAB_RADIUS)
    }

    /// Re-derive the geometry with handle `anchor` placed at `p` —
    /// the phase-C edit. Endpoints/vertices move in place; corners
    /// re-normalize the bounds around the opposite corner, so dragging
    /// through the anchor flips the rectangle like drawing did. No 45°
    /// snapping here (yet): the handle follows the pointer exactly.
    pub(crate) fn set_handle(&mut self, anchor: usize, p: Point<Pixels>) {
        match self.kind {
            ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline => {
                if let Some(q) = self.points.get_mut(anchor) {
                    *q = p;
                }
            }
            ShapeKind::Rectangle | ShapeKind::Ellipse => {
                let b = self.bounds;
                // opposite corner stays fixed (TL, TR, BR, BL order);
                // dragging through it must re-normalize like drawing
                let opposite = match anchor {
                    0 => b.bottom_right(),
                    1 => point(b.left(), b.bottom()),
                    2 => b.origin,
                    _ => point(b.right(), b.top()),
                };
                let (x0, x1) = (opposite.x.min(p.x), opposite.x.max(p.x));
                let (y0, y1) = (opposite.y.min(p.y), opposite.y.max(p.y));
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
    for poly in super::line::geometry(&shape.points, shape.width, shape.kind == ShapeKind::Arrow) {
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
            bounds: Bounds::default(),
            color: 0xffd43b60,
            width: 20.,
            points: points.iter().map(|&(x, y)| point(px(x), px(y))).collect(),
        }
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
        let b = shape(ShapeKind::Arrow, &[(10., 50.), (110., 50.)]).selection_box();
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
}
