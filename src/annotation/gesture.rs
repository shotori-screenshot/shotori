//! The drawing gesture state machine: press → drag → release for
//! every tool (freehand sampling with its smoothing constants, line
//! constraints, number placement, polyline continuation), plus the
//! escape hatches (cancel, polyline finish). The committed history
//! entries land through [`super::history`]; this module is the
//! pointer-to-shape geometry in between.

use super::eraser::EraserGesture;
use super::{Annotations, Draft, INK_RESAMPLE_STEP, INK_SMOOTHING_RADIUS, Shape, ShapeKind};
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

impl Annotations {
    pub(crate) fn begin(&mut self, p: Point<Pixels>, selection: Bounds<Pixels>, same_number: bool) {
        if !self.enabled() || self.tool == Some(ShapeKind::Text) || !selection.contains(&p) {
            return;
        }
        // starting a new stroke gives up the selection
        self.selected = None;
        if self.tool == Some(ShapeKind::Number)
            && (selection.size.width < px(16.) || selection.size.height < px(16.))
        {
            return;
        }
        self.pressed = true;
        if matches!(self.tool, Some(ShapeKind::Eraser | ShapeKind::EraserRect)) {
            // Object eraser (issue #14): no draft, no pixel math —
            // the press itself already deletes what it lands on
            self.eraser_entry_open = false; // a new gesture never
            // merges into the previous one's history entry
            if self.tool == Some(ShapeKind::EraserRect) {
                self.eraser = Some(EraserGesture::Rect { start: p, end: p });
            } else {
                self.eraser = Some(EraserGesture::Stroke { last: p });
                self.erase_sample(p);
            }
            return;
        }
        if self.tool == Some(ShapeKind::Polyline) && self.draft.is_some() {
            return;
        }
        self.draft_generation += 1;
        self.draft = Some(Draft {
            start: p,
            ink: p,
            shape: Shape {
                kind: self.tool.expect("active annotation tool"),
                text: None,
                number: (self.tool == Some(ShapeKind::Number))
                    .then(|| self.placement_number(same_number)),
                bounds: if self.tool == Some(ShapeKind::Number) {
                    number_bounds(p, selection, self.number_size())
                } else {
                    Bounds::new(p, size(px(0.), px(0.)))
                },
                color: if self.tool == Some(ShapeKind::Highlighter) {
                    (self.color().0 & 0xffffff00) | 96
                } else {
                    self.color().0
                },
                width: self.width(),
                points: if matches!(
                    self.tool,
                    Some(ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline)
                ) {
                    vec![p, p]
                } else if matches!(self.tool, Some(ShapeKind::Pencil | ShapeKind::Highlighter)) {
                    vec![p]
                } else {
                    Vec::new()
                },
            },
        });
    }

    pub(crate) fn drag_to(
        &mut self,
        p: Point<Pixels>,
        selection: Bounds<Pixels>,
        square: bool,
    ) -> bool {
        // Copy the gesture out (it is `Copy`) — the sampling below
        // needs `&mut self` for the removals themselves.
        if let Some(EraserGesture::Stroke { last }) = self.eraser {
            // Brush sweep: interpolate along the segment since the
            // last sample — two pointer events far apart must still
            // erase every piece of ink between them.
            let end = line_endpoint(last, p, selection, false);
            let radius = self.erase_radius();
            let step = radius.max(2.);
            let (dx, dy) = (f32::from(end.x - last.x), f32::from(end.y - last.y));
            let span = dx.hypot(dy);
            let steps = (span / step).ceil().max(1.) as usize;
            let mut changed = false;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let sample = point(
                    px(f32::from(last.x) + dx * t),
                    px(f32::from(last.y) + dy * t),
                );
                changed |= self.erase_sample(sample);
            }
            self.eraser = Some(EraserGesture::Stroke { last: end });
            return changed;
        }
        if let Some(EraserGesture::Rect { start, end: cur }) = self.eraser {
            // Area sweep: track the rect; deletion happens on release.
            let end = point(
                p.x.clamp(selection.left(), selection.right()),
                p.y.clamp(selection.top(), selection.bottom()),
            );
            let changed = cur != end;
            self.eraser = Some(EraserGesture::Rect { start, end });
            return changed;
        }
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        if matches!(draft.shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter) {
            let end = line_endpoint(draft.start, p, selection, false);
            // Distance-adaptive exponential smoothing (see Draft::ink):
            // micro tremor is damped hard, fast flicks pass through
            // nearly raw, and the trail behind the pointer stays ≈
            // INK_SMOOTHING_RADIUS at any speed. Recording waits for
            // INK_RESAMPLE_STEP of cursor travel, not raw event density.
            let ink = draft.ink;
            let (dx, dy) = (f32::from(end.x - ink.x), f32::from(end.y - ink.y));
            let dist = dx.hypot(dy);
            let alpha = dist / (dist + INK_SMOOTHING_RADIUS);
            let next = point(ink.x + px(dx * alpha), ink.y + px(dy * alpha));
            draft.ink = next;
            let changed = draft.shape.points.last().is_some_and(|last| {
                f32::from(next.x - last.x).hypot(f32::from(next.y - last.y)) >= INK_RESAMPLE_STEP
            });
            if changed {
                draft.shape.points.push(next);
            }
            return changed;
        }
        if draft.shape.kind == ShapeKind::Number {
            let bounds = number_bounds(p, selection, f32::from(draft.shape.bounds.size.width));
            let changed = bounds != draft.shape.bounds;
            draft.shape.bounds = bounds;
            return changed;
        }
        if matches!(
            draft.shape.kind,
            ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline
        ) {
            let last = draft.shape.points.len() - 1;
            let start = draft.shape.points[last - 1];
            let end = line_endpoint(start, p, selection, square);
            let changed = draft.shape.points[last] != end;
            draft.shape.points[last] = end;
            return changed;
        }
        let start = draft.start;
        let mut end = point(
            p.x.clamp(selection.left(), selection.right()),
            p.y.clamp(selection.top(), selection.bottom()),
        );
        if square {
            let dx = f32::from(end.x - start.x);
            let dy = f32::from(end.y - start.y);
            let available_x = if dx < 0. {
                start.x - selection.left()
            } else {
                selection.right() - start.x
            };
            let available_y = if dy < 0. {
                start.y - selection.top()
            } else {
                selection.bottom() - start.y
            };
            let side = dx
                .abs()
                .max(dy.abs())
                .min(f32::from(available_x))
                .min(f32::from(available_y));
            end = point(
                start.x + px(if dx < 0. { -side } else { side }),
                start.y + px(if dy < 0. { -side } else { side }),
            );
        }
        let bounds = Bounds::from_corners(
            point(start.x.min(end.x), start.y.min(end.y)),
            point(start.x.max(end.x), start.y.max(end.y)),
        );
        let changed = draft.shape.bounds != bounds;
        draft.shape.bounds = bounds;
        changed
    }

    pub(crate) fn end(&mut self) {
        let pressed = std::mem::take(&mut self.pressed);
        // The eraser gesture closes regardless of `pressed` — an
        // undo mid-sweep already cleared the flag, and a stale
        // gesture must never leak into the next tool's drags.
        if let Some(gesture) = self.eraser.take() {
            self.eraser_entry_open = false;
            if pressed && let EraserGesture::Rect { start, end } = gesture {
                // The area eraser deletes on release — the bounds are
                // only final now
                let area = Bounds::from_corners(
                    point(start.x.min(end.x), start.y.min(end.y)),
                    point(start.x.max(end.x), start.y.max(end.y)),
                );
                self.erase_rect(area);
            }
            return;
        }
        if !pressed {
            return;
        }
        if let Some(draft) = self.draft.as_mut()
            && draft.shape.kind == ShapeKind::Polyline
        {
            let last = draft.shape.points.len() - 1;
            let end = draft.shape.points[last];
            if distance(draft.shape.points[last - 1], end) >= 2. {
                draft.shape.points.push(end);
            } else {
                draft.shape.points[last] = draft.shape.points[last - 1];
            }
            return;
        }
        if let Some(draft) = self.draft.take() {
            let valid = if matches!(draft.shape.kind, ShapeKind::Line | ShapeKind::Arrow) {
                distance(draft.shape.points[0], draft.shape.points[1]) >= 2.
            } else if matches!(draft.shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter) {
                true
            } else {
                draft.shape.bounds.size.width >= px(2.) && draft.shape.bounds.size.height >= px(2.)
            };
            if valid {
                self.record_add(draft.shape);
            }
        }
    }

    pub(crate) fn is_pressed(&self) -> bool {
        self.pressed
    }

    pub(crate) fn is_drawing_polyline(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.shape.kind == ShapeKind::Polyline)
    }

    /// Commit confirmed vertices, never the floating cursor preview.
    pub(crate) fn finish_polyline(&mut self) {
        if !self.is_drawing_polyline() {
            return;
        }
        self.pressed = false;
        let mut shape = self.draft.take().unwrap().shape;
        shape.points.pop();
        if shape.points.len() >= 2 {
            self.record_add(shape);
        }
    }

    /// Return to the crop with no active tool, preserving completed marks.
    /// Returns whether an unfinished drawing gesture was interrupted.
    pub(crate) fn finish_interaction(&mut self) -> bool {
        let active = self.pressed || self.draft.is_some() || self.eraser.is_some();
        self.pressed = false;
        self.draft = None;
        self.eraser = None;
        self.eraser_entry_open = false;
        self.selected = None;
        self.tool = None;
        self.size_drag_active = false;
        active
    }

    /// Cancel one annotation state for internal editing commands.
    /// Completed eraser deletions remain in history for Undo.
    /// User-facing Escape clears all interaction state via `finish_interaction`.
    pub(crate) fn cancel(&mut self) -> bool {
        self.pressed = false;
        self.eraser = None;
        self.eraser_entry_open = false;
        if self.draft.take().is_some() {
            return true;
        }
        if self.selected.take().is_some() {
            return true;
        }
        // any lit tool button goes dark — the select tool included
        // (it is not `enabled()`, but Esc must still exit it)
        if self.tool.take().is_some() {
            return true;
        }
        false
    }
}

pub(super) fn number_bounds(
    p: Point<Pixels>,
    selection: Bounds<Pixels>,
    diameter: f32,
) -> Bounds<Pixels> {
    let diameter = px(diameter)
        .min(selection.size.width)
        .min(selection.size.height);
    let radius = diameter / 2.;
    let center = point(
        p.x.clamp(selection.left() + radius, selection.right() - radius),
        p.y.clamp(selection.top() + radius, selection.bottom() - radius),
    );
    Bounds::new(center - point(radius, radius), size(diameter, diameter))
}

pub(super) fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    f32::from(b.x - a.x).hypot(f32::from(b.y - a.y))
}

pub(super) fn line_endpoint(
    start: Point<Pixels>,
    p: Point<Pixels>,
    bounds: Bounds<Pixels>,
    constrain: bool,
) -> Point<Pixels> {
    let end = point(
        p.x.clamp(bounds.left(), bounds.right()),
        p.y.clamp(bounds.top(), bounds.bottom()),
    );
    if !constrain {
        return end;
    }
    let dx = f32::from(end.x - start.x);
    let dy = f32::from(end.y - start.y);
    let direction =
        ((dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize;
    let (x, y): (f32, f32) = [
        (1., 0.),
        (1., 1.),
        (0., 1.),
        (-1., 1.),
        (-1., 0.),
        (-1., -1.),
        (0., -1.),
        (1., -1.),
    ][direction];
    let mut length = dx.hypot(dy) / x.hypot(y);
    for (direction, available) in [
        (
            x,
            if x < 0. {
                start.x - bounds.left()
            } else {
                bounds.right() - start.x
            },
        ),
        (
            y,
            if y < 0. {
                start.y - bounds.top()
            } else {
                bounds.bottom() - start.y
            },
        ),
    ] {
        if direction != 0. {
            length = length.min(f32::from(available));
        }
    }
    point(
        (start.x + px(x * length)).clamp(bounds.left(), bounds.right()),
        (start.y + px(y * length)).clamp(bounds.top(), bounds.bottom()),
    )
}
