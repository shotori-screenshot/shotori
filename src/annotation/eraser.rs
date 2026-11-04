//! The eraser edits the marks it crosses (issue #30): a finished
//! gesture appends its footprint to every touched shape's `erasures`
//! instead of committing an independent mask shape. The raster path
//! restores the pre-mark pixels under each footprint, so the hole is
//! part of the mark and travels with it; bounds and picking treat the
//! footprint as missing ink.

use super::{Annotations, Erasure, Shape, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point};

/// Pointer state for an in-flight erase gesture.
#[derive(Clone, Copy)]
pub(super) enum EraserGesture {
    /// Brush sweep. `last` is the previous sample; the segment to the
    /// next one is interpolated so a fast flick cannot jump over thin
    /// ink between two pointer events.
    Stroke { last: Point<Pixels> },
    /// Area sweep: the dragged rectangle, committed on release (the
    /// bounds are only known at the end).
    Rect {
        start: Point<Pixels>,
        end: Point<Pixels>,
    },
}

impl Annotations {
    /// remembered size; the overlay's ring chrome renders the same
    /// circle, so what you see is what erases.
    pub(crate) fn erase_radius(&self) -> f32 {
        self.size_of(ShapeKind::Eraser) / 2.
    }

    /// The area eraser's in-flight rect in selection-global
    /// coordinates (the dashed chrome outline). None outside the
    /// gesture.
    pub(crate) fn eraser_rect_bounds(&self) -> Option<Bounds<Pixels>> {
        match self.eraser {
            Some(EraserGesture::Rect { start, end }) => Some(Bounds::from_corners(
                point(start.x.min(end.x), start.y.min(end.y)),
                point(start.x.max(end.x), start.y.max(end.y)),
            )),
            _ => None,
        }
    }

    /// Accumulate the sweep samples without touching the marks; the
    /// footprint is applied to the touched shapes on release.
    pub(super) fn erase_sample(&mut self, p: Point<Pixels>) -> bool {
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        if draft.shape.points.last().copied() == Some(p) {
            return false;
        }
        draft.shape.points.push(p);
        true
    }

    pub(super) fn erase_rect(&mut self, area: Bounds<Pixels>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.shape.bounds = area;
        }
    }

    /// Commit the gesture: append the footprint to every shape whose
    /// ink the sweep crossed, as ONE history step (Edit/EditMany) so a
    /// single undo restores every mark's pre-erase state. Nothing is
    /// recorded when no ink was crossed — a sweep through empty canvas
    /// must not pollute history.
    pub(super) fn finish_erase(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let shape = draft.shape;
        let radius = shape.size() / 2.;
        let touched = |s: &Shape| match shape.kind {
            ShapeKind::EraserRect => {
                let i = s.ink_bounds().intersect(&shape.bounds);
                i.size.width > gpui_kit::px(0.) && i.size.height > gpui_kit::px(0.)
            }
            _ => shape
                .points
                .iter()
                .any(|&p| super::select::shape_erased(s, p, radius)),
        };
        let before: Vec<(usize, Shape)> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| touched(s))
            .map(|(ix, s)| (ix, s.clone()))
            .collect();
        let erasure = if shape.kind == ShapeKind::EraserRect {
            Erasure::Rect(shape.bounds)
        } else {
            Erasure::Brush {
                points: shape.points.clone(),
                diameter: shape.size(),
            }
        };
        for (ix, _) in &before {
            if let Some(mark) = self.shapes.get_mut(*ix) {
                mark.erasures.push(erasure.clone());
            }
        }
        self.commit_moves(&before);
    }
}

impl Shape {
    /// Whether `p` falls inside one of this mark's erasure footprints
    /// (the hole's ≥50%-coverage interior, minus the pointing
    /// tolerance so the antialiased rim of the hole stays clickable,
    /// mirroring `HIT_TOLERANCE` on ink). The inverse of
    /// [`super::select::shape_hit`]'s ink test.
    pub(crate) fn erased_at(&self, p: Point<Pixels>, tolerance: f32) -> bool {
        self.erasures.iter().any(|erasure| match erasure {
            Erasure::Brush { points, diameter } => {
                let radius = diameter / 2. - tolerance;
                radius > 0.
                    && points
                        .iter()
                        .zip(points.iter().skip(1).chain(points.first()))
                        .any(|(a, b)| super::select::point_segment_distance(p, *a, *b) <= radius)
            }
            Erasure::Rect(bounds) => {
                super::select::inflate(bounds, gpui_kit::px(-tolerance)).contains(&p)
            }
        })
    }

    /// Cut this mark's erasure footprints into freshly painted pixels:
    /// blend every footprint-covered pixel back to the snapshot taken
    /// BEFORE this mark was painted, which reveals the earlier marks
    /// and capture underneath — the per-mark equivalent of the old
    /// whole-layer mask, but scoped to this mark's own ink.
    pub(super) fn apply_erasures(
        &self,
        rgba: &mut [u8],
        snapshot: &[u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        for erasure in &self.erasures {
            match erasure {
                Erasure::Rect(bounds) => {
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
                    for row in y(bounds.top())..y(bounds.bottom()) {
                        for col in x(bounds.left())..x(bounds.right()) {
                            let offset = (row * w as usize + col) * 4;
                            if rgba.len() >= offset + 4 {
                                rgba[offset..offset + 4]
                                    .copy_from_slice(&snapshot[offset..offset + 4]);
                            }
                        }
                    }
                }
                Erasure::Brush { points, diameter } => {
                    // Reuse the stroke coverage engine with a synthetic
                    // brush shape, exactly like the old mask rasterizer.
                    let brush = Shape {
                        kind: ShapeKind::Eraser,
                        number: None,
                        text: None,
                        text_style: Default::default(),
                        bounds: Bounds::default(),
                        color: 0,
                        parameters: super::ShapeParameters::new(ShapeKind::Eraser, *diameter),
                        points: points.clone(),
                        erasures: Vec::new(),
                    };
                    super::line::coverage(&brush, w, h, origin, scale, |offset, coverage| {
                        if rgba.len() >= offset + 4 {
                            for c in 0..4 {
                                rgba[offset + c] = (rgba[offset + c] as f32 * (1. - coverage)
                                    + snapshot[offset + c] as f32 * coverage)
                                    .round()
                                    as u8;
                            }
                        }
                    });
                }
            }
        }
    }
}

/// Live-preview the in-flight sweep as a whole-layer mask over the
/// capture (the draft eraser shape in `visible()`). Commit replaces
/// this with per-mark footprints; the pixels agree wherever the sweep
/// crosses ink, which is the only place feedback matters.
pub(super) fn rasterize(
    shape: &super::Shape,
    rgba: &mut [u8],
    original: &[u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    super::line::coverage(shape, w, h, origin, scale, |offset, coverage| {
        for c in 0..4 {
            rgba[offset + c] = (rgba[offset + c] as f32 * (1. - coverage)
                + original[offset + c] as f32 * coverage)
                .round() as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{px, size};

    fn area() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(160.), px(120.)))
    }

    fn scene() -> Annotations {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Line);
        marks.set_tool_size(4.);
        marks.set_color(0xff0000ff);
        marks.begin(point(px(20.), px(60.)), area(), false);
        marks.drag_to(point(px(140.), px(60.)), area(), false);
        marks.end();
        marks
    }

    fn image(marks: &Annotations, scale: f32) -> Vec<u8> {
        let w = (160. * scale).round() as u32;
        let h = (120. * scale).round() as u32;
        let mut pixels = [24, 36, 48, 255].repeat((w * h) as usize);
        marks.rasterize(&mut pixels, w, h, point(px(0.), px(0.)), scale);
        pixels
    }

    fn pixel(pixels: &[u8], scale: f32, x: f32, y: f32) -> &[u8] {
        let offset =
            (((y * scale) as usize * (160. * scale).round() as usize) + (x * scale) as usize) * 4;
        &pixels[offset..offset + 4]
    }

    #[test]
    fn brush_only_erases_crossed_ink_and_preserves_newer_layers_and_history() {
        for scale in [1., 1.25, 1.73, 2.] {
            let mut marks = scene();
            let before = image(&marks, scale);
            marks.toggle(ShapeKind::Eraser);
            marks.set_tool_size(16.);
            marks.begin(point(px(80.), px(20.)), area(), false);
            marks.drag_to(point(px(80.), px(100.)), area(), false);
            assert_eq!(marks.committed().len(), 1);
            marks.end();
            // the erasure edits the stroke in place — no mask shape is added
            assert_eq!(marks.committed().len(), 1);
            let erased = image(&marks, scale);
            assert_eq!(pixel(&erased, scale, 80., 60.), &[24, 36, 48, 255]);
            assert_eq!(
                pixel(&erased, scale, 30., 60.),
                pixel(&before, scale, 30., 60.)
            );
            assert_eq!(
                pixel(&erased, scale, 130., 60.),
                pixel(&before, scale, 130., 60.)
            );
            marks.undo();
            assert_eq!(image(&marks, scale), before);
            marks.redo();
            assert_eq!(image(&marks, scale), erased);
            marks.toggle(ShapeKind::Line);
            marks.begin(point(px(60.), px(60.)), area(), false);
            marks.drag_to(point(px(100.), px(60.)), area(), false);
            marks.end();
            assert_eq!(
                pixel(&image(&marks, scale), scale, 80., 60.),
                pixel(&before, scale, 80., 60.)
            );
        }
    }

    #[test]
    fn near_misses_empty_rectangles_and_cancelled_sweeps_leave_history_intact() {
        let mut marks = scene();
        let before = marks.committed().to_vec();
        marks.toggle(ShapeKind::Eraser);
        marks.set_tool_size(16.);
        marks.begin(point(px(80.), px(75.)), area(), false);
        marks.end();
        assert_eq!(marks.committed(), before);
        assert!(marks.committed()[0].erasures.is_empty());
        marks.begin(point(px(80.), px(60.)), area(), false);
        marks.undo();
        marks.drag_to(point(px(40.), px(60.)), area(), false);
        marks.end();
        assert_eq!(marks.committed(), before);
        marks.toggle(ShapeKind::EraserRect);
        marks.begin(point(px(80.), px(60.)), area(), false);
        marks.end();
        assert_eq!(marks.committed(), before);
        marks.undo();
        assert!(marks.committed().is_empty());
    }

    /// The issue #30 repro: erase a chunk, then drag the stroke — the
    /// gap must travel with the ink, not stay behind as a mask hole.
    #[test]
    fn erasure_travels_with_the_mark_when_moved() {
        let mut marks = scene();
        marks.toggle(ShapeKind::Eraser);
        marks.set_tool_size(16.);
        marks.begin(point(px(80.), px(20.)), area(), false);
        marks.drag_to(point(px(80.), px(100.)), area(), false);
        marks.end();
        marks.toggle(ShapeKind::Select);
        let before = marks.committed()[0].clone();
        marks.place_shape(0, &before, point(px(0.), px(30.)));
        marks.commit_move(0, before);
        let moved = image(&marks, 1.);
        // the gap moved down with the stroke…
        assert_eq!(pixel(&moved, 1., 80., 90.), &[24, 36, 48, 255]);
        // …the ink reads through where the stroke used to be…
        assert_eq!(pixel(&moved, 1., 30., 90.), pixel(&moved, 1., 130., 90.));
        assert_eq!(pixel(&moved, 1., 80., 60.), &[24, 36, 48, 255]);
        // …and undo/redo of the move keeps the gap glued to the ink
        marks.undo();
        assert_eq!(pixel(&image(&marks, 1.), 1., 80., 60.), &[24, 36, 48, 255]);
        marks.redo();
        assert_eq!(pixel(&image(&marks, 1.), 1., 80., 90.), &[24, 36, 48, 255]);
    }

    /// Erasing an end of the stroke shrinks the mark's ink bounds to
    /// the surviving ink, so selection chrome and marquee match what
    /// is actually visible (issue #30's AABB requirement).
    #[test]
    fn erasures_shrink_ink_bounds_to_the_surviving_ink() {
        let mut marks = scene();
        let full = marks.committed()[0].ink_bounds();
        marks.toggle(ShapeKind::Eraser);
        marks.set_tool_size(24.);
        marks.begin(point(px(132.), px(20.)), area(), false);
        marks.drag_to(point(px(132.), px(100.)), area(), false);
        marks.end();
        let shrunken = marks.committed()[0].ink_bounds();
        assert_eq!(shrunken.left(), full.left());
        assert_eq!(shrunken.top(), full.top());
        assert_eq!(shrunken.bottom(), full.bottom());
        assert!(shrunken.right() <= px(120.), "right edge {shrunken:?}");
        assert!(shrunken.right() < full.right());
        // a hole in the middle cannot shrink the extents
        marks.undo();
        marks.set_tool_size(16.);
        marks.begin(point(px(80.), px(20.)), area(), false);
        marks.drag_to(point(px(80.), px(100.)), area(), false);
        marks.end();
        let holed = marks.committed()[0].ink_bounds();
        assert_eq!(holed.left(), full.left());
        assert_eq!(holed.right(), full.right());
        // fully erased: nothing pickable survives
        marks.undo();
        marks.set_tool_size(96.);
        marks.begin(point(px(80.), px(60.)), area(), false);
        marks.drag_to(point(px(80.), px(60.)), area(), false);
        marks.end();
        assert_eq!(marks.committed().len(), 1);
        let fully_erased = marks.committed()[0].clone();
        assert!(!crate::annotation::select::shape_hit(
            &fully_erased,
            point(px(80.), px(60.))
        ));
        assert_eq!(marks.hit_test(point(px(80.), px(60.))), None);
    }
}
