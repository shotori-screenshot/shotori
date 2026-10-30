//! Selecting placed shapes: hit-testing and the selection state.
//!
//! Picks prefer visible ink and allow a small margin around thin strokes.
//! Unselected hollow interiors stay transparent; selected frames own their interior.
//! This module owns
//! the geometry probes and the `selected` slot on [`Annotations`];
//! the pointer-level state machine (click vs. draw-through) lives in
//! the session, which drives these APIs.
use super::history::ShapeEdit;
use super::{Annotations, HistoryEntry, Shape, ShapeKind, line};
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

/// Pointing forgiveness for hairline geometry — the hit region is the
/// visible stroke itself; this only covers the antialiased fringe so
/// an edge-pointing click still lands.
const HIT_TOLERANCE: f32 = 0.5;
const PICK_TOLERANCE: f32 = 4.;

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
    /// Prefer visible ink, then allow a small pointing margin around it.
    /// Both passes preserve topmost priority without filling hollow shapes.
    pub(crate) fn hit_test(&self, p: Point<Pixels>) -> Option<usize> {
        self.shapes
            .iter()
            .rposition(|s| shape_hit(s, p))
            .or_else(|| {
                self.shapes.iter().rposition(|s| {
                    matches!(
                        s.kind,
                        ShapeKind::Rectangle
                            | ShapeKind::Ellipse
                            | ShapeKind::Line
                            | ShapeKind::Arrow
                            | ShapeKind::Polyline
                            | ShapeKind::Pencil
                            | ShapeKind::Highlighter
                    ) && shape_erased(s, p, PICK_TOLERANCE)
                })
            })
    }

    /// Drawing only picks the current selection; idle mode also picks other marks.
    /// Text keeps existing text selectable for double-click editing.
    pub(crate) fn interaction_hit_test(&self, p: Point<Pixels>) -> Option<usize> {
        self.selected
            .iter()
            .rev()
            .copied()
            .find(|&ix| {
                self.shapes
                    .get(ix)
                    .is_some_and(|shape| shape.selection_box().contains(&p))
            })
            .or_else(|| match self.tool {
                None | Some(ShapeKind::Select) => self.hit_test(p),
                Some(ShapeKind::Text) => self
                    .hit_test(p)
                    .filter(|&ix| self.shapes[ix].kind == ShapeKind::Text),
                _ => None,
            })
    }

    /// Match the move cursor to the selected frame, suppressing unrelated drawing hover.
    pub(crate) fn shape_hover(&self, p: Point<Pixels>) -> Option<ShapeHover> {
        if !self.parks_click_select() {
            return None;
        }
        let ix = self.interaction_hit_test(p)?;
        if !matches!(self.tool, None | Some(ShapeKind::Select)) && !self.is_selected(ix) {
            return None;
        }
        Some(if self.is_selected(ix) {
            ShapeHover::Move
        } else {
            ShapeHover::Pick
        })
    }

    /// Every shape whose selection frame intersects `rect` — the
    /// marquee's batch pick, in list order (bottom to top), so the
    /// last entry handed to [`select_indices`] is the topmost shape
    /// and takes primary — the same priority a click has.
    pub(crate) fn shapes_in(&self, rect: Bounds<Pixels>) -> Vec<usize> {
        self.shapes
            .iter()
            .enumerate()
            .filter(|(_, shape)| {
                let b = shape.selection_box();
                b.origin.x < rect.right()
                    && b.right() > rect.left()
                    && b.origin.y < rect.bottom()
                    && b.bottom() > rect.top()
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Select a shape by index (the plain click-select path): replaces
    /// the whole selection. No-op when the index no longer exists.
    pub(crate) fn select_index(&mut self, ix: usize) -> bool {
        if self.shapes.get(ix).is_some() {
            self.selected.clear();
            self.selected.push(ix);
            true
        } else {
            false
        }
    }

    /// Shift+click: add `ix` to the selection, or drop it when it is
    /// already in (the conventional toggle). Dropping the primary
    /// hands primary to whatever was selected before it, so a
    /// multi-selection keeps a meaningful edit target.
    pub(crate) fn toggle_index(&mut self, ix: usize) -> bool {
        if self.shapes.get(ix).is_none() {
            return false;
        }
        match self.selected.iter().position(|sel| *sel == ix) {
            Some(pos) => {
                self.selected.remove(pos);
            }
            None => self.selected.push(ix),
        }
        true
    }

    /// Replace the selection with `ixs` (the marquee path). Unknown
    /// indices are dropped and duplicates collapse onto their first
    /// occurrence; the LAST survivor becomes primary, so passing the
    /// hit order through hands the primary to the topmost shape.
    pub(crate) fn select_indices(&mut self, ixs: impl IntoIterator<Item = usize>) {
        let mut kept: Vec<usize> = Vec::new();
        for ix in ixs {
            if self.shapes.get(ix).is_some() && !kept.contains(&ix) {
                kept.push(ix);
            }
        }
        self.selected = kept;
    }

    pub(crate) fn deselect(&mut self) {
        self.selected.clear();
    }

    /// Remove every selected shape (the Delete/Backspace path). One
    /// history entry restores the whole batch — the deletion records
    /// each shape's index at the moment it left, so undo re-inserts
    /// them in reverse and redo re-takes them in order. The selection
    /// drops because indices shift after mid-sequence removals.
    pub(crate) fn delete_selected(&mut self) -> bool {
        self.delete_shapes(self.selected.clone())
    }

    pub(crate) fn selected(&self) -> Option<&Shape> {
        self.selected_index().and_then(|ix| self.shapes.get(ix))
    }

    /// The primary selected shape's index (valid or None): the last
    /// entry of the selection order, which owns the style editors.
    /// Handles are exposed only when it is the sole selected shape.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
            .last()
            .copied()
            .filter(|ix| self.shapes.get(*ix).is_some())
    }

    /// Every selected index, primary last.
    pub(crate) fn selected_indices(&self) -> &[usize] {
        &self.selected
    }

    /// The selected shapes in selection order (primary last), skipping
    /// indices whose shape has gone away.
    pub(crate) fn selected_shapes(&self) -> Vec<&Shape> {
        self.selected
            .iter()
            .filter_map(|ix| self.shapes.get(*ix))
            .collect()
    }

    pub(crate) fn is_selected(&self, ix: usize) -> bool {
        self.selected.contains(&ix)
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// The size the wheel/slider edit right now: the selected shape's
    /// when one is live, else the active tool's preset — one truth for
    /// the slider's position, its readout and the write paths.
    pub(crate) fn current_edit_size(&self) -> f32 {
        if self.editing_filter_brush_size()
            && let super::FilterArea::Brush { diameter } = self.filter_area()
        {
            return diameter;
        }
        if let Some(shape) = self.editing_text() {
            return shape.size();
        }
        match self.selected() {
            Some(shape) if shape.kind == ShapeKind::Number => f32::from(shape.bounds.size.width),
            Some(shape) => shape.size(),
            None => self.tool_size(),
        }
    }

    /// The kind whose spec governs the current edit target (selected
    /// shape first, else the active tool). The select tool is
    /// filtered out — it has no size semantics of its own, so with it
    /// active and nothing selected the settings row and slider hide.
    pub(crate) fn edit_kind(&self) -> Option<ShapeKind> {
        self.editing_text()
            .map(|shape| shape.kind)
            .or_else(|| self.selected().map(|shape| shape.kind))
            .or(self.tool.filter(|k| *k != ShapeKind::Select))
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
            shape.parameters.set_size(v.clamp(spec.min, spec.max));
            return;
        }
        let brush_size = self.editing_filter_brush_size();
        let spec = self.current_size_spec();
        let v = v.clamp(spec.min, spec.max);
        let v = self.bounded_size(v, brush_size);
        if brush_size {
            self.filter_brush_size = v;
            self.filter_area = super::FilterArea::Brush { diameter: v };
        }
        let Some(ix) = self.selected_index() else {
            if !brush_size {
                self.set_tool_size(v);
            }
            return;
        };
        let kind = self.shapes[ix].kind;
        let v = v.clamp(spec.min, spec.max);
        if !brush_size {
            self.set_size_of(kind, v);
        }
        let write = |shapes: &mut Vec<Shape>| {
            if let Some(shape) = shapes.get_mut(ix) {
                if brush_size {
                    if let super::ShapeParameters::Filter { area, .. } = &mut shape.parameters {
                        *area = super::FilterArea::Brush { diameter: v };
                    }
                } else {
                    resize_shape(shape, v);
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
                self.size_drag_active = true;
            }
        }
    }

    fn bounded_size(&self, requested: f32, brush: bool) -> f32 {
        let (Some(before), Some(area)) = (self.selected(), self.resize_bounds) else {
            return requested;
        };
        let current = self.current_edit_size();
        if requested <= current || before.kind == ShapeKind::Spotlight {
            return requested;
        }
        // Preserve already clipped marks after a crop change without allowing
        // a subsequent size edit to extend their clipping.
        let limit = area.union(&before.ink_bounds());
        let fits = |value| {
            let mut candidate = before.clone();
            if brush {
                if let super::ShapeParameters::Filter { area, .. } = &mut candidate.parameters {
                    *area = super::FilterArea::Brush { diameter: value };
                }
            } else {
                resize_shape(&mut candidate, value);
            }
            let bounds = candidate.ink_bounds();
            bounds.left() >= limit.left()
                && bounds.top() >= limit.top()
                && bounds.right() <= limit.right()
                && bounds.bottom() <= limit.bottom()
        };
        if fits(requested) {
            return requested;
        }
        let (mut low, mut high) = (current, requested);
        for _ in 0..24 {
            let mid = (low + high) / 2.;
            if fits(mid) {
                low = mid;
            } else {
                high = mid;
            }
        }
        // Integral controls must not accumulate subpixel growth at a blocked edge.
        if before.kind == ShapeKind::Magnifier {
            low
        } else {
            low.floor().max(current)
        }
    }

    /// Scope size edits to the capture before recording history or clearing redo.
    pub(crate) fn edit_within(&mut self, area: Bounds<Pixels>, edit: impl FnOnce(&mut Self)) {
        let previous = self.resize_bounds.replace(area);
        edit(self);
        self.resize_bounds = previous;
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
            *shape = before.clone();
            shape.translate(delta);
        }
    }

    /// The whole-selection variant of [`place_shape`]: every shape
    /// follows the same delta, each re-derived from its own press
    /// snapshot.
    pub(crate) fn place_shapes(&mut self, before: &[(usize, Shape)], delta: Point<Pixels>) {
        for (ix, shape) in before {
            self.place_shape(*ix, shape, delta);
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
        area: Bounds<Pixels>,
    ) {
        let Some(start) = before.handle_points().get(anchor).copied() else {
            return;
        };
        // A curve may overshoot its handles. Constrain the resulting outline,
        // including selection padding, rather than just the pointer position.
        // Existing clipping after crop changes must not make a drag jump.
        let limit = area.union(&before.selection_box());
        let fits = |shape: &Shape| {
            let b = shape.selection_box();
            b.left() >= limit.left()
                && b.right() <= limit.right()
                && b.top() >= limit.top()
                && b.bottom() <= limit.bottom()
        };
        if let Some(shape) = self.shapes.get_mut(ix) {
            *shape = before.clone();
            shape.set_handle(anchor, p);
            if fits(shape) {
                return;
            }
            let (mut low, mut high) = (0., 1.);
            for _ in 0..20 {
                let fraction = (low + high) / 2.;
                *shape = before.clone();
                shape.set_handle(anchor, start + (p - start) * fraction);
                if fits(shape) {
                    low = fraction;
                } else {
                    high = fraction;
                }
            }
            *shape = before.clone();
            shape.set_handle(anchor, start + (p - start) * low);
        }
    }

    /// Finish an in-place edit drag (move or handle): record the Edit
    /// (before → current) so undo restores the pre-drag state. No
    /// entry when nothing changed.
    pub(crate) fn commit_move(&mut self, ix: usize, before: Shape) {
        self.commit_moves(&[(ix, before)]);
    }

    /// Commit a whole selection's drag as ONE history step: a single
    /// `Edit` when only one shape moved, `EditMany` otherwise.
    pub(crate) fn commit_moves(&mut self, before: &[(usize, Shape)]) {
        let mut edits: Vec<ShapeEdit> = Vec::new();
        for (ix, before) in before {
            let Some(after) = self.shapes.get(*ix) else {
                continue;
            };
            if after != before {
                edits.push((*ix, before.clone(), after.clone()));
            }
        }
        match edits.len() {
            0 => return,
            1 => {
                let (ix, before, after) = edits.remove(0);
                self.history.push(HistoryEntry::Edit { ix, before, after });
            }
            _ => self.history.push(HistoryEntry::EditMany { edits }),
        }
        self.redo.clear();
    }

    /// Step the selected badge value independently of wheel/slider diameter. Floors at 1 (0 is not a badge); one reversible Edit
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

    /// Idle interaction selects marks; a newly placed selection stays editable.
    /// Explicitly activating a drawing tool clears selection and draws over ink.
    pub(crate) fn parks_click_select(&self) -> bool {
        matches!(self.tool, None | Some(ShapeKind::Select)) || !self.selected.is_empty()
    }

    /// Step the selected shape's size through its OWN preset ladder —
    /// the same rungs the toolbar exposes per tool. Number badges grow
    /// their diameter around its center; text scales its bounds from the origin.
    /// Other kinds change stroke width. One reversible Edit entry per notch.
    pub(crate) fn step_selected_size(&mut self, ix: usize, up: bool) -> bool {
        let kind = self.shapes[ix].kind;
        let spec = super::size_spec(kind);
        let current = if kind == ShapeKind::Number {
            f32::from(self.shapes[ix].bounds.size.width)
        } else {
            self.shapes[ix].size()
        };
        let next = if up { current + 1. } else { current - 1. };
        if next == current || next < spec.min || next > spec.max {
            return false;
        }
        let before = self.shapes[ix].clone();
        self.end_size_drag();
        self.apply_size(next);
        self.end_size_drag();
        self.shapes[ix] != before
    }
}

/// Keep size-dependent geometry in the same history snapshot as its style.
fn resize_shape(shape: &mut Shape, value: f32) {
    match shape.kind {
        ShapeKind::Number => {
            shape.parameters.set_size(value);
            let b = shape.bounds;
            let center = point(b.left() + b.size.width / 2., b.top() + b.size.height / 2.);
            shape.bounds = Bounds::new(
                point(center.x - px(value / 2.), center.y - px(value / 2.)),
                size(px(value), px(value)),
            );
        }
        ShapeKind::Text => {
            // Scale the wrapping width and height with the glyphs, preserving
            // the text origin and line layout instead of clipping to the old box.
            let ratio = value / shape.size();
            let inset = px(shape.text_style.inset() * 2.);
            shape.bounds.size = size(
                (shape.bounds.size.width - inset).max(px(1.)) * ratio + inset,
                (shape.bounds.size.height - inset).max(px(1.)) * ratio + inset,
            );
            shape.parameters.set_size(value);
        }
        _ => shape.parameters.set_size(value),
    }
}

/// Whether a point lands on a selectable shape. Every kind's region is
/// its visible stroke or body; see [`line::geometry`] for the shared
/// visual outline.
pub(super) fn shape_hit(shape: &Shape, p: Point<Pixels>) -> bool {
    if shape.has_custom_geometry() {
        return line::shape_geometry(shape)
            .iter()
            .any(|poly| polygon_touched(p, poly, HIT_TOLERANCE));
    }
    if let Some(super::FilterArea::Brush { diameter }) = shape.filter_area() {
        return line::geometry(&shape.points, diameter, false)
            .iter()
            .any(|poly| point_in_polygon(p, poly));
    }
    let (x, y) = (f32::from(p.x), f32::from(p.y));
    match shape.kind {
        // stroke band: any of the four edge rectangles, inflated by the
        // antialiased fringe
        ShapeKind::Magnifier => super::magnifier::contains(shape, p),
        ShapeKind::Spotlight => super::spotlight::contains(shape, p),
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
            let inner_rx = (rx - shape.size() - HIT_TOLERANCE).max(0.);
            let inner_ry = (ry - shape.size() - HIT_TOLERANCE).max(0.);
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
            line::shape_geometry(shape)
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
            let band = line::geometry(&shape.points, shape.size(), false)
                .iter()
                .any(|poly| point_in_polygon(p, poly));
            band || ring_is_closed(shape) && point_in_polygon(p, &shape.points)
        }
        // freehand families share the same visual-polygon outline
        ShapeKind::Pencil | ShapeKind::Highlighter | ShapeKind::Eraser => {
            line::shape_geometry(shape)
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
                || shape
                    .number_arrow()
                    .is_some_and(|arrow| shape_erased(&arrow, p, PICK_TOLERANCE))
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
    if shape.has_custom_geometry() {
        return line::shape_geometry(shape)
            .iter()
            .any(|poly| polygon_touched(p, poly, radius));
    }
    if let Some(super::FilterArea::Brush { diameter }) = shape.filter_area() {
        return line::geometry(&shape.points, diameter, false)
            .iter()
            .any(|poly| polygon_touched(p, poly, radius));
    }
    match shape.kind {
        // Stroke families: the exact visual polygons (capsules,
        // arrowheads — the same geometry `shape_hit` trusts), each
        // dilated by the brush radius.
        ShapeKind::Line
        | ShapeKind::Arrow
        | ShapeKind::Polyline
        | ShapeKind::Pencil
        | ShapeKind::Highlighter
        | ShapeKind::Eraser => line::shape_geometry(shape)
            .iter()
            .any(|poly| polygon_touched(p, poly, radius)),
        ShapeKind::Magnifier => super::magnifier::contains(shape, p),
        ShapeKind::Spotlight => super::spotlight::contains(shape, p),
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
                    || shape
                        .number_arrow()
                        .is_some_and(|arrow| shape_erased(&arrow, p, radius))
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
    let inner_rx = rx - shape.size();
    let inner_ry = ry - shape.size();
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
    // Fewer than 3 vertices cannot enclose area, and `j = len - 1`
    // below would wrap on an empty slice. Today's callers only feed
    // real polygons; the guard keeps this generic utility safe for
    // future ones.
    if poly.len() < 3 {
        return false;
    }
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
            super::distance(*first, *last) <= CLOSURE_SLOP + shape.size()
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

    #[test]
    fn free_curve_handles_keep_the_entire_outline_inside_the_crop() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.)));
        use crate::annotation::ArrowStyle;
        for (kind, arrow) in [
            (ShapeKind::Line, ArrowStyle::Open),
            (ShapeKind::Arrow, ArrowStyle::Open),
            (ShapeKind::Arrow, ArrowStyle::DoubleOpen),
            (ShapeKind::Arrow, ArrowStyle::Outline),
            (ShapeKind::Arrow, ArrowStyle::Filled),
            (ShapeKind::Arrow, ArrowStyle::Dimension),
            (ShapeKind::Arrow, ArrowStyle::Triangle),
            (ShapeKind::Arrow, ArrowStyle::DoubleTriangle),
        ] {
            let mut a = Annotations::default();
            a.toggle(kind);
            a.set_arrow_style(arrow);
            a.begin(point(px(100.), px(150.)), area, false);
            a.drag_to(point(px(300.), px(150.)), area, false);
            a.end();
            let before = a.shapes[0].clone();
            // Near either endpoint, the handle follows both pointer coordinates.
            for target in [point(px(101.), px(140.)), point(px(299.), px(160.))] {
                a.place_handle(0, 2, &before, target, area);
                assert_eq!(a.shapes[0].handle_points()[2], target);
            }
            for anchor in 0..3 {
                for target in [
                    point(px(-200.), px(150.)),
                    point(px(600.), px(150.)),
                    point(px(200.), px(-200.)),
                    point(px(200.), px(500.)),
                    point(px(1.), px(1.)),
                    point(px(399.), px(299.)),
                ] {
                    a.place_handle(0, anchor, &before, target, area);
                    let bounds = a.shapes[0].selection_box();
                    assert!(bounds.left() >= area.left());
                    assert!(bounds.right() <= area.right());
                    assert!(bounds.top() >= area.top());
                    assert!(bounds.bottom() <= area.bottom());
                }
            }
            let after = a.shapes[0].clone();
            a.commit_move(0, before.clone());
            a.undo();
            assert_eq!(a.shapes[0], before);
            a.redo();
            assert_eq!(a.shapes[0], after);
        }
    }

    #[test]
    fn resizing_placed_text_updates_bounds_and_history() {
        for wheel in [false, true] {
            let mut a = Annotations::default();
            a.add_text(
                Bounds::new(point(px(40.), px(50.)), size(px(220.), px(64.8))),
                "Canvas note\nSecond row".into(),
            );
            a.select_index(0);
            let original = a.selected().unwrap().clone();
            if wheel {
                assert!(a.step_selected_size(0, true));
            } else {
                a.apply_size(36.);
                a.apply_size(48.);
                a.end_size_drag();
            }
            let enlarged = a.selected().unwrap().clone();
            let ratio = enlarged.size() / original.size();
            assert_eq!(enlarged.bounds.origin, original.bounds.origin);
            assert_eq!(
                enlarged.bounds.size,
                size(
                    original.bounds.size.width * ratio,
                    original.bounds.size.height * ratio
                )
            );
            assert_eq!(enlarged.text, original.text);
            a.undo();
            assert_eq!(a.shapes[0], original);
            a.redo();
            assert_eq!(a.shapes[0], enlarged);
            a.select_index(0);
            if wheel {
                assert!(a.step_selected_size(0, false));
            } else {
                a.apply_size(24.);
                a.end_size_drag();
            }
            let restored = &a.shapes[0];
            assert_eq!(restored.size(), original.size());
            assert!(
                (f32::from(restored.bounds.size.width - original.bounds.size.width)).abs() < 0.001
            );
            assert!(
                (f32::from(restored.bounds.size.height - original.bounds.size.height)).abs()
                    < 0.001
            );
        }
    }

    #[test]
    fn resized_text_raster_keeps_every_line_at_fractional_scales() {
        super::super::text::with_test_font(|| {
            let mut a = Annotations::default();
            a.add_text(
                Bounds::new(point(px(20.2), px(30.3)), size(px(220.), px(64.8))),
                "Canvas note\nSecond row".into(),
            );
            a.select_index(0);
            for font_size in [72., 12., 36., 24.] {
                a.apply_size(font_size);
                let shape = a.selected().unwrap();
                for scale in [1_f32, 1.25, 1.5, 1.75, 2.] {
                    let w = (800. * scale).ceil() as u32;
                    let h = (400. * scale).ceil() as u32;
                    let mut actual = vec![255; (w * h * 4) as usize];
                    super::super::text::rasterize(
                        shape,
                        &mut actual,
                        w,
                        h,
                        point(px(0.), px(0.)),
                        scale,
                    );
                    // A taller reference uses identical wrapping and glyphs but
                    // cannot hide a clipped last line behind the shape's mask.
                    let mut reference_shape = shape.clone();
                    reference_shape.bounds.size.height = px(350.);
                    let mut expected = vec![255; actual.len()];
                    super::super::text::rasterize(
                        &reference_shape,
                        &mut expected,
                        w,
                        h,
                        point(px(0.), px(0.)),
                        scale,
                    );
                    assert!(actual.iter().any(|channel| *channel != 255));
                    assert!(
                        actual == expected,
                        "size {font_size}, scale {scale}: clipped text"
                    );
                }
            }
        });
    }

    fn polyline(points: &[(f32, f32)], width: f32) -> Shape {
        Shape {
            kind: ShapeKind::Polyline,
            number: None,
            text: None,
            text_style: Default::default(),
            bounds: Bounds::default(),
            color: 0xffd43b60,
            parameters: crate::annotation::ShapeParameters::new(ShapeKind::Polyline, width),
            points: points.iter().map(|&(x, y)| point(px(x), px(y))).collect(),
        }
    }

    fn hits(shape: &Shape, x: f32, y: f32) -> bool {
        shape_hit(shape, point(px(x), px(y)))
    }

    #[test]
    fn thin_stroke_picking_has_margin_without_widening_the_eraser() {
        for kind in [
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Polyline,
            ShapeKind::Pencil,
        ] {
            let mut shape = polyline(&[(20., 30.), (100., 30.)], 1.);
            shape.kind = kind;
            let mut a = Annotations::default();
            a.record_add(shape.clone());
            let near = point(px(60.), px(34.));
            assert_eq!(a.hit_test(near), Some(0), "{kind:?}");
            assert!(!shape_erased(&shape, near, 0.5), "{kind:?}");
            assert_eq!(a.hit_test(point(px(60.), px(50.))), None);
        }
        for kind in [ShapeKind::Rectangle, ShapeKind::Ellipse] {
            let shape = Shape {
                kind,
                bounds: Bounds::new(point(px(20.), px(20.)), size(px(100.), px(80.))),
                ..polyline(&[], 1.)
            };
            let mut a = Annotations::default();
            a.record_add(shape);
            assert_eq!(a.hit_test(point(px(70.), px(16.))), Some(0), "{kind:?}");
            assert_eq!(a.hit_test(point(px(70.), px(60.))), None, "{kind:?}");
        }
    }

    #[test]
    fn exact_ink_wins_over_a_newer_shapes_pointing_margin() {
        let mut a = Annotations::default();
        a.record_add(polyline(&[(20., 30.), (100., 30.)], 1.));
        a.record_add(polyline(&[(20., 34.), (100., 34.)], 1.));
        assert_eq!(a.hit_test(point(px(60.), px(30.))), Some(0));
        assert_eq!(a.hit_test(point(px(60.), px(32.))), Some(1));
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
        // the cursor never lags behind what a press would do. Only
        // the select tool advertises anything: a draw tool's press
        // draws (the polyline's places a vertex), and no tool at all
        // is region editing — neither may promise a selection.
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
        // Placing no longer selects (the 2026-10-06 flip), and while
        // the owning tool runs the hover advertises nothing.
        assert_eq!(a.selected_index(), None);
        assert_eq!(a.shape_hover(point(px(35.), px(35.))), None);
        a.toggle(ShapeKind::Polyline); // off
        assert_eq!(
            a.shape_hover(point(px(35.), px(35.))),
            Some(ShapeHover::Pick)
        );
        a.toggle(ShapeKind::Select);
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
