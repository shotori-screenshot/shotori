//! Editable erasure masks restore captured pixels without deleting entire marks.

use super::{Annotations, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point};

/// Pointer state for an in-flight local erase mask.
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

    /// Accumulate a continuous mask without modifying the editable source marks.
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

    pub(super) fn finish_erase(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let shape = draft.shape;
        let rect = shape.kind == ShapeKind::EraserRect;
        let meaningful = if rect {
            shape.bounds.size.width > gpui_kit::px(0.)
                && shape.bounds.size.height > gpui_kit::px(0.)
                && self.shapes.iter().any(|s| {
                    let i = s.ink_bounds().intersect(&shape.bounds);
                    i.size.width > gpui_kit::px(0.) && i.size.height > gpui_kit::px(0.)
                })
        } else {
            shape.points.iter().any(|&p| {
                self.shapes
                    .iter()
                    .any(|s| super::select::shape_erased(s, p, shape.size() / 2.))
            })
        };
        if meaningful {
            self.record_add(shape);
            if !rect {
                self.selected.clear();
            }
        }
    }
}

/// Restore captured pixels at the mask's layer; later marks remain above the erasure.
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

    #[test]
    fn rectangle_mask_remains_editable_without_destroying_original_marks() {
        let mut marks = scene();
        let original = image(&marks, 1.);
        marks.toggle(ShapeKind::EraserRect);
        marks.begin(point(px(70.), px(50.)), area(), false);
        marks.drag_to(point(px(90.), px(70.)), area(), false);
        marks.end();
        assert_eq!(marks.committed().len(), 2);
        assert_eq!(pixel(&image(&marks, 1.), 1., 80., 60.), &[24, 36, 48, 255]);
        let mask = marks.committed()[1].clone();
        marks.place_shape(1, &mask, point(px(0.), px(30.)));
        marks.commit_move(1, mask);
        assert_eq!(image(&marks, 1.), original);
        marks.undo();
        assert_eq!(pixel(&image(&marks, 1.), 1., 80., 60.), &[24, 36, 48, 255]);
        marks.undo();
        assert_eq!(image(&marks, 1.), original);
    }
}
