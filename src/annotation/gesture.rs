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
            self.draft_generation += 1;
            let kind = self.tool.unwrap_or(ShapeKind::Eraser);
            self.draft = Some(Draft {
                straight_anchor: None,
                repeat_number: false,
                ink: p,
                start: p,
                shape: Shape {
                    kind,
                    bounds: Bounds::new(p, size(px(0.), px(0.))),
                    color: 0,
                    number: None,
                    text: None,
                    text_style: Default::default(),
                    parameters: super::ShapeParameters::new(kind, self.erase_radius() * 2.),
                    points: vec![p],
                },
            });
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
        // The overlay only dispatches presses with a drawing tool active
        // (Select/None park earlier), so this lookup holds by contract —
        // but the draw path must not panic, so degrade to a no-op.
        let Some(tool) = self.tool else {
            return;
        };
        let filter_brush = matches!(tool, ShapeKind::Mosaic | ShapeKind::Blur)
            && matches!(self.filter_area, super::FilterArea::Brush { .. });
        let mut parameters = super::ShapeParameters::new(tool, self.width());
        if matches!(
            tool,
            ShapeKind::Line | ShapeKind::Polyline | ShapeKind::Arrow
        ) && let super::ShapeParameters::Stroke {
            style, arrow, ends, ..
        } = &mut parameters
        {
            *style = self.line_style;
            *arrow = self.arrow_style;
            *ends = self.line_ends;
        }
        if let super::ShapeParameters::Geometry {
            fill,
            radius,
            style,
            ..
        } = &mut parameters
        {
            *fill = self.geometry_fill;
            *radius = self.geometry_radius;
            *style = self.line_style;
        }
        if let super::ShapeParameters::Highlighter { options, .. } = &mut parameters {
            *options = self.highlighter;
        }
        if let super::ShapeParameters::Badge { format, arrow, .. } = &mut parameters {
            *format = self.number_format;
            *arrow = self.number_arrow_style;
        }
        if tool == ShapeKind::Spotlight {
            parameters = super::ShapeParameters::Spotlight {
                opacity: self.spotlight_opacity,
                ellipse: self.spotlight_ellipse,
                outline: self.spotlight_outline,
            };
        }
        if tool == ShapeKind::Magnifier {
            parameters = super::ShapeParameters::Magnifier(self.lens);
        }
        if let super::ShapeParameters::Filter { area, .. } = &mut parameters {
            *area = self.filter_area;
        }
        self.draft = Some(Draft {
            straight_anchor: None,
            repeat_number: same_number,
            start: p,
            ink: p,
            shape: Shape {
                kind: tool,
                text: None,
                text_style: Default::default(),
                number: (tool == ShapeKind::Number).then(|| self.placement_number(same_number)),
                bounds: if tool == ShapeKind::Number {
                    number_bounds(p, selection, self.number_size())
                } else {
                    Bounds::new(p, size(px(0.), px(0.)))
                },
                color: self.color().0,
                parameters,
                points: if matches!(
                    tool,
                    ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline
                ) {
                    vec![p, p]
                } else if filter_brush
                    || tool == ShapeKind::Pencil
                    || (tool == ShapeKind::Highlighter && !self.highlighter.rectangle)
                {
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
            self.erase_rect(Bounds::from_corners(
                point(start.x.min(end.x), start.y.min(end.y)),
                point(start.x.max(end.x), start.y.max(end.y)),
            ));
            return changed;
        }
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        if let Some(super::FilterArea::Brush { .. }) = draft.shape.filter_area() {
            let end = line_endpoint(draft.start, p, selection, false);
            if draft
                .shape
                .points
                .last()
                .is_some_and(|last| distance(*last, end) < INK_RESAMPLE_STEP)
            {
                return false;
            }
            draft.shape.points.push(end);
            return true;
        }
        if draft.shape.is_freehand() {
            if draft.shape.kind == ShapeKind::Pencil && square {
                let anchor = *draft
                    .straight_anchor
                    .get_or_insert(draft.shape.points.len().saturating_sub(1));
                let start = draft
                    .shape
                    .points
                    .get(anchor)
                    .copied()
                    .unwrap_or(draft.start);
                let end = angle_endpoint(start, p, selection, self.pencil_angle);
                let changed = draft.shape.points.last().copied() != Some(end);
                draft.shape.points.truncate(anchor + 1);
                draft.shape.points.push(end);
                draft.ink = end;
                return changed;
            }
            draft.straight_anchor = None;
            let end = line_endpoint(draft.start, p, selection, false);
            // Distance-adaptive exponential smoothing (see Draft::ink):
            // micro tremor is damped hard, fast flicks pass through
            // nearly raw, and the trail behind the pointer stays ≈
            // INK_SMOOTHING_RADIUS at any speed. Recording waits for
            // INK_RESAMPLE_STEP of cursor travel, not raw event density.
            let ink = draft.ink;
            let (dx, dy) = (f32::from(end.x - ink.x), f32::from(end.y - ink.y));
            let dist = dx.hypot(dy);
            let alpha = if draft.shape.kind == ShapeKind::Pencil && !self.pencil_smooth {
                1.
            } else {
                dist / (dist + INK_SMOOTHING_RADIUS)
            };
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
            let tip = line_endpoint(draft.shape.bounds.center(), p, selection, square);
            return draft.shape.drag_number_tip(tip, selection);
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
        if draft.shape.kind == ShapeKind::Magnifier {
            draft.shape.place_lens_beside_source(selection);
        }
        changed
    }

    pub(crate) fn end(&mut self) {
        let pressed = std::mem::take(&mut self.pressed);
        if self.eraser.take().is_some() {
            if pressed {
                self.finish_erase();
            } else {
                self.draft = None;
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
            } else if matches!(
                draft.shape.filter_area(),
                Some(super::FilterArea::Brush { .. })
            ) || draft.shape.is_freehand()
            {
                true
            } else {
                draft.shape.bounds.size.width >= px(2.) && draft.shape.bounds.size.height >= px(2.)
            };
            if valid {
                self.record_placement(draft.shape, draft.repeat_number);
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
        // is_drawing_polyline() above proved a draft exists; take()
        // degrades instead of unwrapping to keep the pointer path
        // panic-free by construction.
        let Some(draft) = self.draft.take() else {
            return;
        };
        let mut shape = draft.shape;
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
        self.selected = None;
        self.tool = None;
        self.size_drag_active = false;
        active
    }

    /// Cancel one annotation state for internal editing commands.
    /// Completed eraser deletions remain in history for Undo.
    /// Tool exit clears passive selection and tool state via `finish_interaction`.
    pub(crate) fn cancel(&mut self) -> bool {
        let erasing = self.eraser.is_some();
        self.pressed = false;
        self.eraser = None;
        if self.draft.take().is_some() || erasing {
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

/// Quantize a ray and clip its length, keeping the selected angle at crop edges.
fn angle_endpoint(
    start: Point<Pixels>,
    p: Point<Pixels>,
    bounds: Bounds<Pixels>,
    degrees: u16,
) -> Point<Pixels> {
    if degrees == 0 {
        return line_endpoint(start, p, bounds, false);
    }
    let delta = p - start;
    let dx = f32::from(delta.x);
    let dy = f32::from(delta.y);
    let step = (degrees as f32).to_radians();
    let angle = (dy.atan2(dx) / step).round() * step;
    let (sin, cos) = angle.sin_cos();
    let mut length = dx.hypot(dy);
    for (component, position, low, high) in [
        (cos, start.x, bounds.left(), bounds.right()),
        (sin, start.y, bounds.top(), bounds.bottom()),
    ] {
        if component.abs() > 0.00001 {
            let available = if component > 0. {
                high - position
            } else {
                position - low
            };
            length = length.min(f32::from(available).max(0.) / component.abs());
        }
    }
    start + point(px(cos * length), px(sin * length))
}

#[cfg(test)]
mod pencil_tests {
    use super::*;

    #[test]
    fn shift_replaces_only_the_live_segment_and_releases_back_to_freehand() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(220.)));
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Pencil);
        marks.set_pencil_smooth(false);
        marks.begin(point(px(20.), px(20.)), area, false);
        marks.drag_to(point(px(40.), px(40.)), area, false);
        let prefix = marks.draft_shape().unwrap().points.clone();
        marks.set_pencil_angle(45);
        marks.drag_to(point(px(110.), px(85.)), area, true);
        marks.drag_to(point(px(160.), px(150.)), area, true);
        let shape = marks.draft_shape().unwrap();
        assert_eq!(&shape.points[..prefix.len()], &prefix);
        assert_eq!(shape.points.len(), prefix.len() + 1);
        let delta = shape.points.last().copied().unwrap() - prefix[prefix.len() - 1];
        assert!((f32::from(delta.x) - f32::from(delta.y)).abs() < 0.001);
        marks.drag_to(point(px(180.), px(160.)), area, false);
        assert_eq!(
            marks.draft_shape().unwrap().points.last().copied(),
            Some(point(px(180.), px(160.)))
        );
        marks.end();
        assert_eq!(marks.committed().len(), 1);
        marks.undo();
        assert!(marks.committed().is_empty());
    }

    #[test]
    fn all_angle_constraints_clip_length_without_changing_direction() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(80.)));
        let start = point(px(50.), px(40.));
        for degrees in [5, 10, 15, 30, 45] {
            for target in [
                point(px(-200.), px(300.)),
                point(px(300.), px(-200.)),
                point(px(50.), px(40.)),
            ] {
                let end = angle_endpoint(start, target, bounds, degrees);
                assert!(end.x >= bounds.left() - px(0.001) && end.x <= bounds.right() + px(0.001));
                assert!(end.y >= bounds.top() - px(0.001) && end.y <= bounds.bottom() + px(0.001));
                if distance(start, end) > 0.001 {
                    let delta = end - start;
                    let steps =
                        f32::from(delta.y).atan2(f32::from(delta.x)).to_degrees() / degrees as f32;
                    assert!((steps - steps.round()).abs() < 0.001);
                }
            }
        }
    }
}
