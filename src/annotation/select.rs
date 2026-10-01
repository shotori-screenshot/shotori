//! Selecting placed shapes: hit-testing and the selection state.
//!
//! The hit region of every kind is its visible stroke or region —
//! "what you see is what you can click" (issue #5). This module owns
//! the geometry probes and the `selected` slot on [`Annotations`];
//! the pointer-level state machine (click vs. draw-through) lives in
//! the session, which drives these APIs.
use super::{Annotations, HistoryEntry, Shape, ShapeKind, line};
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

/// Pointing forgiveness for hairline geometry — the hit region is the
/// visible stroke itself; this only covers the antialiased fringe so
/// an edge-pointing click still lands.
const HIT_TOLERANCE: f32 = 0.5;

/// What a press at the hovered spot would do to a shape — the input
/// to the cursor affordance. A hand cursor implies "I'm holding
/// something", which is wrong before anything is grabbed: the
/// pre-selection hover advertises "click to pick" instead (the
/// canvas-app convention, issue #17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShapeHover {
    /// Over an unselected shape: press selects it.
    Pick,
    /// Over the selected shape's body: press starts a move drag.
    Move,
}

impl Annotations {
    /// Hit probe without side effects: the topmost shape index under
    /// the point, if any.
    pub(crate) fn hit_test(&self, p: Point<Pixels>) -> Option<usize> {
        self.shapes.iter().rposition(|s| shape_hit(s, p))
    }

    /// Classify the hover at `p` for the cursor affordance. The
    /// topmost hit decides — the same shape a press would act on
    /// (`pointer_down` parks its click on the topmost hit, selected
    /// or not) — so a selected shape buried under a newer one still
    /// reads as Pick at the overlap. Tools that never park a click
    /// (polyline, the erasers) advertise no shape affordance at all:
    /// their press means something else.
    pub(crate) fn shape_hover(&self, p: Point<Pixels>) -> Option<ShapeHover> {
        if !self.parks_click_select() {
            return None;
        }
        let ix = self.hit_test(p)?;
        Some(if self.selected_index() == Some(ix) {
            ShapeHover::Move
        } else {
            ShapeHover::Pick
        })
    }

    /// Select a shape by index (the click-select path); no-op when the
    /// index no longer exists.
    pub(crate) fn select_index(&mut self, ix: usize) -> bool {
        if self.shapes.get(ix).is_some() {
            self.selected = Some(ix);
            true
        } else {
            false
        }
    }

    pub(crate) fn deselect(&mut self) {
        self.selected = None;
    }

    /// Remove the selected shape (the Delete/Backspace path). Records
    /// a Remove entry so undo re-inserts at the same index; drops the
    /// selection because indices shift after a mid-sequence removal.
    pub(crate) fn delete_selected(&mut self) -> bool {
        let Some(ix) = self.selected_index() else {
            return false;
        };
        if ix >= self.shapes.len() {
            return false;
        }
        let shape = self.shapes.remove(ix);
        self.history.push(HistoryEntry::Remove { ix, shape });
        self.redo.clear();
        self.selected = None;
        true
    }

    pub(crate) fn selected(&self) -> Option<&Shape> {
        self.selected.and_then(|ix| self.shapes.get(ix))
    }

    /// The selected shape's index (valid or None).
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected.filter(|ix| self.shapes.get(*ix).is_some())
    }

    /// The size the wheel/slider edit right now: the selected shape's
    /// when one is live, else the active tool's preset — one truth for
    /// the slider's position, its readout and the write paths.
    pub(crate) fn current_edit_size(&self) -> f32 {
        if let Some(shape) = self.editing_text() {
            return shape.width;
        }
        match self.selected() {
            Some(shape) if shape.kind == ShapeKind::Number => f32::from(shape.bounds.size.width),
            Some(shape) => shape.width,
            None => self.tool_size(),
        }
    }

    /// The kind whose spec governs the current edit target (selected
    /// shape first, else the active tool).
    pub(crate) fn edit_kind(&self) -> Option<ShapeKind> {
        self.editing_text()
            .map(|shape| shape.kind)
            .or_else(|| self.selected().map(|shape| shape.kind))
            .or(self.tool)
    }

    /// The slider's write path: apply `v` to the selected shape (one
    /// merged history entry per drag — a whole drag undoes as one
    /// step) or to the tool preset when nothing is selected.
    pub(crate) fn apply_size(&mut self, v: f32) {
        if self.editing_text().is_some() {
            self.set_size_of(ShapeKind::Text, v);
        }
        if let Some(shape) = self.editing_text_mut() {
            let spec = super::size_spec(ShapeKind::Text);
            shape.width = v.clamp(spec.min, spec.max);
            return;
        }
        let Some(ix) = self.selected_index() else {
            self.set_tool_size(v);
            return;
        };
        let kind = self.shapes[ix].kind;
        let spec = super::size_spec(kind);
        let v = v.clamp(spec.min, spec.max);
        self.set_size_of(kind, v); // remember for the next stroke
        let write = |shapes: &mut Vec<Shape>| {
            if let Some(shape) = shapes.get_mut(ix) {
                if kind == ShapeKind::Number {
                    let b = shape.bounds;
                    let c = point(b.left() + b.size.width / 2., b.top() + b.size.height / 2.);
                    shape.bounds = Bounds::new(
                        point(c.x - px(v / 2.), c.y - px(v / 2.)),
                        size(px(v), px(v)),
                    );
                } else {
                    shape.width = v;
                }
            }
        };
        // merge into the previous entry while the same drag continues
        // (same shape); any interruption — selection change, undo, a
        // new stroke — breaks the chain check and starts a fresh entry
        let mergeable = self.size_drag_active
            && matches!(self.history.last(), Some(HistoryEntry::Edit { ix: e, .. }) if *e == ix);
        if mergeable {
            write(&mut self.shapes);
            if let Some(HistoryEntry::Edit { after, .. }) = self.history.last_mut() {
                *after = self.shapes[ix].clone();
            }
        } else {
            let before = self.shapes[ix].clone();
            write(&mut self.shapes);
            let after = self.shapes[ix].clone();
            if after != before {
                self.history.push(HistoryEntry::Edit { ix, before, after });
                self.redo.clear();
            }
            self.size_drag_active = true;
        }
    }

    /// Close the slider drag: the next `apply_size` starts a new
    /// history entry. Called on `SliderEvent::Release`.
    pub(crate) fn end_size_drag(&mut self) {
        self.size_drag_active = false;
    }

    /// Re-place shape `ix` as the press-time snapshot translated by
    /// `delta`. Snapshot re-derivation means move events cannot
    /// accumulate float error, and a zero delta restores the snapshot
    /// (the Escape path).
    pub(crate) fn place_shape(&mut self, ix: usize, before: &Shape, delta: Point<Pixels>) {
        if let Some(shape) = self.shapes.get_mut(ix) {
            shape.bounds = Bounds::new(before.bounds.origin + delta, before.bounds.size);
            shape.points = before.points.iter().map(|p| *p + delta).collect();
        }
    }

    /// Re-derive shape `ix` from the press snapshot with handle
    /// `anchor` placed at `p` — snapshot semantics like [`place_shape`].
    pub(crate) fn place_handle(
        &mut self,
        ix: usize,
        anchor: usize,
        before: &Shape,
        p: Point<Pixels>,
    ) {
        if let Some(shape) = self.shapes.get_mut(ix) {
            *shape = before.clone();
            shape.set_handle(anchor, p);
        }
    }

    /// Finish an in-place edit drag (move or handle): record the Edit
    /// (before → current) so undo restores the pre-drag state. No
    /// entry when nothing changed.
    pub(crate) fn commit_move(&mut self, ix: usize, before: Shape) {
        let Some(after) = self.shapes.get(ix) else {
            return;
        };
        if after != &before {
            self.history.push(HistoryEntry::Edit {
                ix,
                before,
                after: after.clone(),
            });
            self.redo.clear();
        }
    }

    /// Step the selected badge's VALUE ±1 — the wheel over a selected
    /// number (issue #2's quick tune); the size slider keeps owning the
    /// diameter. Floors at 1 (0 is not a badge); one reversible Edit
    /// entry per notch.
    pub(crate) fn step_selected_number(&mut self, ix: usize, up: bool) -> bool {
        let Some(current) = self.shapes.get(ix).and_then(|s| s.number) else {
            return false;
        };
        let next = match (up, current) {
            (true, _) => current.saturating_add(1),
            (false, 0 | 1) => return false,
            (false, _) => current - 1,
        };
        if next == current {
            return false;
        }
        let before = self.shapes[ix].clone();
        self.shapes[ix].number = Some(next);
        let after = self.shapes[ix].clone();
        self.history.push(HistoryEntry::Edit { ix, before, after });
        self.redo.clear();
        true
    }

    /// Write a value with NO history entry — the double-click editor's
    /// live preview. The caller holds the pre-edit shape; commit goes
    /// through `commit_move` (before-snapshot → current), cancel
    /// re-previews the original. The NumberCache keys on `shape.number`,
    /// so rendering follows along.
    pub(crate) fn preview_number(&mut self, ix: usize, value: u32) {
        if let Some(shape) = self.shapes.get_mut(ix) {
            shape.number = Some(value);
        }
    }

    /// Whether a press right now should park a click-select pending
    /// resolution (release = select, drag past the slop = draw
    /// through). Polyline never parks: its clicks PLACE VERTICES, and
    /// an intercepting hit would break polygons mid-drawing — its
    /// shapes stay selectable from any other tool. The eraser never
    /// parks either: its press on a shape must erase it (issue #14),
    /// not promise a selection.
    pub(crate) fn parks_click_select(&self) -> bool {
        !matches!(
            self.tool,
            Some(ShapeKind::Polyline | ShapeKind::Eraser | ShapeKind::EraserRect)
        )
    }

    /// Step the selected shape's size through its OWN preset ladder —
    /// the same rungs the toolbar exposes per tool. Number badges grow
    /// their diameter (bounds re-centered); every other kind steps its
    /// width field. One reversible Edit entry per notch.
    pub(crate) fn step_selected_size(&mut self, ix: usize, up: bool) -> bool {
        let kind = self.shapes[ix].kind;
        let spec = super::size_spec(kind);
        let current = if kind == ShapeKind::Number {
            f32::from(self.shapes[ix].bounds.size.width)
        } else {
            self.shapes[ix].width
        };
        let next = if up { current + 1. } else { current - 1. };
        if next == current || next < spec.min || next > spec.max {
            return false;
        }
        self.set_size_of(kind, next); // remember for the next stroke
        let before = self.shapes[ix].clone();
        if kind == ShapeKind::Number {
            // grow the badge around its center
            let b = self.shapes[ix].bounds;
            let c = point(b.left() + b.size.width / 2., b.top() + b.size.height / 2.);
            self.shapes[ix].bounds = Bounds::new(
                point(c.x - px(next / 2.), c.y - px(next / 2.)),
                size(px(next), px(next)),
            );
        } else {
            self.shapes[ix].width = next;
        }
        let after = self.shapes[ix].clone();
        self.history.push(HistoryEntry::Edit { ix, before, after });
        self.redo.clear();
        true
    }
}

/// Whether a point lands on a selectable shape. Every kind's region is
/// its visible stroke or body; see [`line::geometry`] for the shared
/// visual outline.
pub(super) fn shape_hit(shape: &Shape, p: Point<Pixels>) -> bool {
    let (x, y) = (f32::from(p.x), f32::from(p.y));
    match shape.kind {
        // stroke band: any of the four edge rectangles, inflated by the
        // antialiased fringe
        ShapeKind::Rectangle => shape
            .strokes()
            .iter()
            .any(|s| inflate(s, px(HIT_TOLERANCE)).contains(&p)),
        ShapeKind::Ellipse => {
            let rx = f32::from(shape.bounds.size.width) / 2.;
            let ry = f32::from(shape.bounds.size.height) / 2.;
            if rx <= 0. || ry <= 0. {
                return false;
            }
            let cx = f32::from(shape.bounds.origin.x) + rx;
            let cy = f32::from(shape.bounds.origin.y) + ry;
            let outer = ((x - cx) / (rx + HIT_TOLERANCE)).powi(2)
                + ((y - cy) / (ry + HIT_TOLERANCE)).powi(2);
            let inner_rx = (rx - shape.width - HIT_TOLERANCE).max(0.);
            let inner_ry = (ry - shape.width - HIT_TOLERANCE).max(0.);
            // a band thinner than the tolerance means even the center
            // is within reach — the whole disc hits
            let inner_clear = inner_rx <= 0.
                || inner_ry <= 0.
                || ((x - cx) / inner_rx).powi(2) + ((y - cy) / inner_ry).powi(2) >= 1.;
            outer <= 1. && inner_clear
        }
        ShapeKind::Line | ShapeKind::Arrow => {
            // the exact visual geometry (capsule / arrowhead polygon):
            // what you see is what you can click
            line::geometry(&shape.points, shape.width, shape.kind == ShapeKind::Arrow)
                .iter()
                .any(|poly| point_in_polygon(p, poly))
        }
        // A placed polyline is a stroke band like its freehand
        // cousins, plus the interior when the ring reads as closed:
        // the shape carries no closed flag, so a sealed polygon
        // means the final click landed back on the first vertex
        // (issue #16). Ray casting over the vertex ring stays exact
        // for concave outlines a bounding-box test would misjudge.
        ShapeKind::Polyline => {
            let band = line::geometry(&shape.points, shape.width, false)
                .iter()
                .any(|poly| point_in_polygon(p, poly));
            band || ring_is_closed(shape) && point_in_polygon(p, &shape.points)
        }
        // freehand families share the same visual-polygon outline
        ShapeKind::Pencil | ShapeKind::Highlighter => {
            line::geometry(&shape.points, shape.width, false)
                .iter()
                .any(|poly| point_in_polygon(p, poly))
        }
        // the badge is a circle inscribed in its bounds
        ShapeKind::Number => {
            let r = f32::from(shape.bounds.size.width) / 2.;
            if r <= 0. {
                return false;
            }
            let c = shape.bounds.origin + point(px(r), px(r));
            (f32::from(p.x - c.x)).hypot(f32::from(p.y - c.y)) <= r + 3.0
        }
        // solid regions: anywhere inside the bounds
        ShapeKind::Text | ShapeKind::Mosaic | ShapeKind::Blur => {
            inflate(&shape.bounds, px(HIT_TOLERANCE)).contains(&p)
        }
        _ => false,
    }
}

/// Whether an eraser brush circle of `radius` centered at `p` touches
/// the shape's VISIBLE INK (issue #14). Deliberately narrower than
/// [`shape_hit`]'s region: a closed polygon's interior is clickable
/// (issue #16) but not erasable — a brush sweeping inside an
/// enclosing ring must leave the ring alone, or nothing inside it
/// could ever be erased individually. Hollow rectangle/ellipse
/// outlines likewise erase only from their stroke band.
pub(super) fn shape_erased(shape: &Shape, p: Point<Pixels>, radius: f32) -> bool {
    match shape.kind {
        // Stroke families: the exact visual polygons (capsules,
        // arrowheads — the same geometry `shape_hit` trusts), each
        // dilated by the brush radius.
        ShapeKind::Line
        | ShapeKind::Arrow
        | ShapeKind::Polyline
        | ShapeKind::Pencil
        | ShapeKind::Highlighter => {
            line::geometry(&shape.points, shape.width, shape.kind == ShapeKind::Arrow)
                .iter()
                .any(|poly| polygon_touched(p, poly, radius))
        }
        ShapeKind::Rectangle => shape
            .strokes()
            .iter()
            .any(|s| inflate(s, px(radius)).contains(&p)),
        ShapeKind::Ellipse => ellipse_ring_touched(shape, p, radius),
        // The badge is solid ink: its inscribed circle plus the brush.
        ShapeKind::Number => {
            let r = f32::from(shape.bounds.size.width) / 2.;
            r > 0. && {
                let c = shape.bounds.origin + point(px(r), px(r));
                (f32::from(p.x - c.x)).hypot(f32::from(p.y - c.y)) <= r + radius
            }
        }
        // Solid regions: the whole bounds is ink.
        ShapeKind::Text | ShapeKind::Mosaic | ShapeKind::Blur => {
            inflate(&shape.bounds, px(radius)).contains(&p)
        }
        _ => false,
    }
}

/// Whether `p` lies inside the polygon or within `radius` of its
/// outline — the polygon "dilated" by the brush, without building
/// the dilated polygon.
fn polygon_touched(p: Point<Pixels>, poly: &[Point<Pixels>], radius: f32) -> bool {
    point_in_polygon(p, poly)
        || poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .any(|(a, b)| point_segment_distance(p, *a, *b) <= radius)
}

/// Euclidean distance from `p` to the segment `ab`.
fn point_segment_distance(p: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let (dx, dy) = (f32::from(b.x - a.x), f32::from(b.y - a.y));
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f32::EPSILON {
        return (f32::from(p.x - a.x)).hypot(f32::from(p.y - a.y));
    }
    let t = ((f32::from(p.x - a.x) * dx + f32::from(p.y - a.y) * dy) / length_sq).clamp(0., 1.);
    let (qx, qy) = (f32::from(a.x) + t * dx, f32::from(a.y) + t * dy);
    (f32::from(p.x) - qx).hypot(f32::from(p.y) - qy)
}

/// Ring-band touch test for ellipse outlines. Exact point-to-ellipse
/// distance has no closed form; sampling both contours (the hole is
/// the inner ring, matching the export path) into chords is within
/// half a chord of the truth — far below eraser tolerances at 64
/// samples per contour.
fn ellipse_ring_touched(shape: &Shape, p: Point<Pixels>, radius: f32) -> bool {
    let rx = f32::from(shape.bounds.size.width) / 2.;
    let ry = f32::from(shape.bounds.size.height) / 2.;
    if rx <= 0. || ry <= 0. {
        return false;
    }
    let cx = f32::from(shape.bounds.origin.x) + rx;
    let cy = f32::from(shape.bounds.origin.y) + ry;
    let point_at =
        |rx: f32, ry: f32, theta: f32| point(px(cx + rx * theta.cos()), px(cy + ry * theta.sin()));
    let inner_rx = rx - shape.width;
    let inner_ry = ry - shape.width;
    for contour in [(rx, ry, 0.), (inner_rx, inner_ry, std::f32::consts::PI)] {
        if contour.0 <= 0. || contour.1 <= 0. {
            continue; // band thinner than the stroke: no hole
        }
        let steps = 64;
        let mut prev = point_at(contour.0, contour.1, contour.2);
        for i in 1..=steps {
            let theta = contour.2 + i as f32 * std::f32::consts::TAU / steps as f32;
            let next = point_at(contour.0, contour.1, theta);
            if point_segment_distance(p, prev, next) <= radius {
                return true;
            }
            prev = next;
        }
    }
    false
}

/// Even-odd ray casting: is the point inside the polygon?
fn point_in_polygon(p: Point<Pixels>, poly: &[Point<Pixels>]) -> bool {
    let (x, y) = (f32::from(p.x), f32::from(p.y));
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = (f32::from(poly[i].x), f32::from(poly[i].y));
        let (xj, yj) = (f32::from(poly[j].x), f32::from(poly[j].y));
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Pointing forgiveness for sealing a polyline ring: the handle
/// grab radius (7 px in `chrome.rs`) on top of the stroke's own
/// footprint, so a human aiming the final click at the first
/// vertex gets a closed polygon while endpoints that merely sit
/// nearby keep the open-stroke hit region.
const CLOSURE_SLOP: f32 = 7.;

/// Whether a placed polyline reads as a closed polygon. The shape
/// model carries no closed flag — closing is structural: the ring
/// is sealed when its final vertex landed back on the first within
/// pointing slop. Ray casting then treats the vertex list as the
/// ring (an implicit last→first edge that is near-zero here).
fn ring_is_closed(shape: &Shape) -> bool {
    match (shape.points.first(), shape.points.last()) {
        // A two-vertex "ring" encloses nothing; a degenerate sliver
        // simply never passes the ray-cast test, so 3 is the only
        // floor that matters.
        (Some(first), Some(last)) if shape.points.len() >= 3 => {
            super::distance(*first, *last) <= CLOSURE_SLOP + shape.width
        }
        _ => false,
    }
}

pub(crate) fn inflate(b: &Bounds<Pixels>, by: Pixels) -> Bounds<Pixels> {
    Bounds::new(
        point(b.origin.x - by, b.origin.y - by),
        size(b.size.width + by * 2., b.size.height + by * 2.),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polyline(points: &[(f32, f32)], width: f32) -> Shape {
        Shape {
            kind: ShapeKind::Polyline,
            number: None,
            text: None,
            bounds: Bounds::default(),
            color: 0xffd43b60,
            width,
            points: points.iter().map(|&(x, y)| point(px(x), px(y))).collect(),
        }
    }

    fn hits(shape: &Shape, x: f32, y: f32) -> bool {
        shape_hit(shape, point(px(x), px(y)))
    }

    /// An L whose ring returns to its start. The notch around (60,60)
    /// sits inside the bounding box but outside the polygon — the
    /// false positive a naive bbox test would select on.
    const CLOSED_L: &[(f32, f32)] = &[
        (10., 10.),
        (90., 10.),
        (90., 40.),
        (40., 40.),
        (40., 90.),
        (10., 90.),
        (10., 10.),
    ];

    #[test]
    fn closed_polygon_interior_selects_and_bbox_gaps_stay_clear() {
        // triangle sealed back on its start; width 4 → 2 px band
        let triangle = polyline(&[(10., 10.), (90., 10.), (50., 70.), (10., 10.)], 4.);
        assert!(hits(&triangle, 50., 30.), "convex interior must select");
        assert!(
            !hits(&triangle, 80., 60.),
            "inside the bbox, outside the ring"
        );
        assert!(hits(&triangle, 50., 10.), "outline band keeps hitting");
    }

    #[test]
    fn concave_l_polygon_hits_both_arms_and_misses_the_notch() {
        let l = polyline(CLOSED_L, 4.);
        assert!(hits(&l, 30., 70.), "left arm interior");
        assert!(hits(&l, 70., 20.), "top arm interior");
        assert!(!hits(&l, 60., 60.), "the notch is not part of the ring");
    }

    #[test]
    fn open_polyline_keeps_its_stroke_band_only_region() {
        // same L without the return click: the ring is not sealed, so
        // the interior (implicit closing chord aside) must stay clear
        let open = &CLOSED_L[..CLOSED_L.len() - 1];
        let l = polyline(open, 4.);
        assert!(!hits(&l, 30., 70.), "open stroke has no interior region");
        assert!(hits(&l, 50., 10.), "the outline band still selects");
    }

    #[test]
    fn closure_tolerates_an_imprecise_final_click_only() {
        // 5 px off the start (20,20) with width 4: within CLOSURE_SLOP
        let sealed = polyline(
            &[(20., 20.), (80., 20.), (80., 80.), (20., 80.), (25., 20.)],
            4.,
        );
        assert!(ring_is_closed(&sealed));
        assert!(hits(&sealed, 50., 50.));
        // 40 px off: endpoints that merely sit nearby stay an open stroke
        let unsealed = polyline(
            &[(20., 20.), (80., 20.), (80., 80.), (20., 80.), (60., 20.)],
            4.,
        );
        assert!(!ring_is_closed(&unsealed));
        assert!(!hits(&unsealed, 50., 50.));
        // a two-vertex "ring" encloses nothing
        assert!(!ring_is_closed(&polyline(&[(10., 10.), (90., 10.)], 4.)));
    }

    #[test]
    fn freehand_loops_stay_stroke_band_only() {
        // A pencil/highlighter loop is still a stroke visually — the
        // interior gain is the polyline polygon's alone (issue #16)
        for kind in [ShapeKind::Pencil, ShapeKind::Highlighter] {
            let mut shape = polyline(CLOSED_L, 4.);
            shape.kind = kind;
            assert!(!hits(&shape, 30., 70.), "{kind:?} interior must stay clear");
            assert!(hits(&shape, 50., 10.), "{kind:?} stroke band keeps hitting");
        }
    }

    #[test]
    fn polygon_drawn_through_the_tool_selects_by_interior_click() {
        // The placed-shape path end to end: click vertices (the last
        // back on the first), finish, then the interior probe must
        // find the shape. While the polyline tool stays active the
        // press must NOT park a click-select — its clicks place
        // vertices; selection of the placed ring happens from any
        // other tool.
        let selection = Bounds::new(point(px(-20.), px(0.)), size(px(100.), px(100.)));
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Polyline);
        for (x, y) in [(10., 10.), (60., 10.), (60., 60.), (10., 60.), (10., 10.)] {
            let p = point(px(x), px(y));
            a.begin(p, selection, false);
            a.drag_to(p, selection, false);
            a.end();
        }
        a.finish_polyline();
        assert!(!a.parks_click_select(), "vertex placement owns the clicks");
        assert_eq!(a.hit_test(point(px(35.), px(35.))), Some(0));
        assert!(a.select_index(0));
        assert_eq!(a.selected().map(|s| s.kind), Some(ShapeKind::Polyline));
    }

    #[test]
    fn polygon_interior_hover_picks_then_moves_once_selected() {
        // Integration of #16 + #17: the interior hit (#16) feeds the
        // hover classifier (#17), so a closed ring advertises Pick
        // inside before selection and Move on the selected body —
        // the cursor never lags behind what a press would do. While
        // the polyline tool is still active the hover advertises
        // NOTHING: its press places a vertex, so no shape affordance
        // may promise otherwise (the eraser's gate, issue #14, is the
        // same rule).
        let selection = Bounds::new(point(px(-20.), px(0.)), size(px(100.), px(100.)));
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Polyline);
        for (x, y) in [(10., 10.), (60., 10.), (60., 60.), (10., 60.), (10., 10.)] {
            let p = point(px(x), px(y));
            a.begin(p, selection, false);
            a.drag_to(p, selection, false);
            a.end();
        }
        a.finish_polyline();
        // Freshly placed shapes auto-select (`record_add`), but the
        // owning tool keeps the clicks — no affordance while it runs.
        assert_eq!(a.shape_hover(point(px(35.), px(35.))), None);
        a.toggle(ShapeKind::Polyline); // off (also drops the selection)
        assert_eq!(
            a.shape_hover(point(px(35.), px(35.))),
            Some(ShapeHover::Pick)
        );
        assert!(a.select_index(0));
        assert_eq!(
            a.shape_hover(point(px(35.), px(35.))),
            Some(ShapeHover::Move)
        );
    }
}
