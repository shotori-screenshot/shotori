//! The object eraser (issue #14): whole-shape deletion by brush sweep
//! or rectangle. Unlike every other tool it carries no draft shape —
//! the gesture state is the sweep itself, and its only output is
//! removals folded into one history entry (see [`super::history`]).

use super::{Annotations, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point};

/// An eraser gesture in flight (issue #14). The eraser deletes whole
/// shapes, so unlike every other tool it carries NO draft shape — the
/// gesture state is the sweep itself, and its only output is
/// removals from `shapes`.
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

    /// One brush sample: delete every shape whose ink the circle at
    /// `p` touches, topmost first. Removals are live (the canvas
    /// updates as the brush sweeps) and accumulate into ONE history
    /// entry per gesture — created on the first removal, extended
    /// afterwards (the `apply_size` merging pattern), so history
    /// never diverges from `shapes` even if undo lands mid-gesture.
    pub(super) fn erase_sample(&mut self, p: Point<Pixels>) -> bool {
        let radius = self.erase_radius();
        let mut removed = Vec::new();
        // Descending indices stay valid as the list shrinks; each
        // pair records the index valid at the moment its shape left.
        let mut ix = self.shapes.len();
        while ix > 0 {
            ix -= 1;
            if super::select::shape_erased(&self.shapes[ix], p, radius) {
                removed.push((ix, self.shapes.remove(ix)));
            }
        }
        self.commit_removals(removed)
    }

    /// The area eraser's release: delete every shape whose bounds
    /// intersect the dragged rect — deliberately coarse ("clear this
    /// area"), in contrast with the brush's ink-touching precision.
    pub(super) fn erase_rect(&mut self, area: Bounds<Pixels>) -> bool {
        let hits = |b: &Bounds<Pixels>| {
            let i = b.intersect(&area);
            f32::from(i.size.width) > 0. && f32::from(i.size.height) > 0.
        };
        let mut removed = Vec::new();
        let mut ix = self.shapes.len();
        while ix > 0 {
            ix -= 1;
            if hits(&self.shapes[ix].bounds) {
                removed.push((ix, self.shapes.remove(ix)));
            }
        }
        self.commit_removals(removed)
    }
}
