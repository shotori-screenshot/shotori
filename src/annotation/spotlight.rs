//! Spotlight masks preserve the union of highlighted regions.
use super::{Annotations, HistoryEntry, Shape, ShapeKind, ShapeParameters};
use gpui_kit::{Bounds, Pixels, Point, point, px, size};

/// The outline is painted inside the highlighted region, never outside the crop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SpotlightOutline {
    pub(super) enabled: bool,
    pub(super) width: f32,
}

impl Default for SpotlightOutline {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 2.,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SpotlightAdjustment {
    Opacity,
    Region,
    Outline,
}

impl Annotations {
    pub(crate) fn spotlight_outline(&self) -> (bool, f32) {
        let outline = match self.selected().map(|s| s.parameters) {
            Some(ShapeParameters::Spotlight { outline, .. }) => outline,
            _ => self.spotlight_outline,
        };
        (outline.enabled, outline.width)
    }

    pub(crate) fn set_spotlight_outline(&mut self, enabled: bool) {
        self.end_size_drag();
        self.spotlight_outline.enabled = enabled;
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Spotlight { outline, .. } = &mut self.shapes[ix].parameters {
                outline.enabled = enabled;
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn set_spotlight_outline_width(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(1., 20.);
        self.spotlight_outline.width = value;
        let Some(ix) = self.selected_index() else {
            return;
        };
        let before = self.shapes[ix].clone();
        let ShapeParameters::Spotlight { outline, .. } = &mut self.shapes[ix].parameters else {
            return;
        };
        outline.width = value;
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

    /// Wheel modifiers target independent spotlight properties; each notch is reversible.
    pub(crate) fn step_spotlight(&mut self, adjustment: SpotlightAdjustment, up: bool) -> bool {
        if self.edit_kind() != Some(ShapeKind::Spotlight) {
            return false;
        }
        self.end_size_drag();
        match adjustment {
            SpotlightAdjustment::Opacity => self.step_size(up),
            SpotlightAdjustment::Outline => {
                let (enabled, width) = self.spotlight_outline();
                if !enabled {
                    return false;
                }
                self.set_spotlight_outline_width(width + if up { 1. } else { -1. });
                self.end_size_drag();
                self.spotlight_outline().1 != width
            }
            SpotlightAdjustment::Region => {
                let Some(ix) = self.selected_index() else {
                    return false;
                };
                let before = self.shapes[ix].clone();
                let b = before.bounds;
                let center = b.center();
                let min_size = px(4.);
                let factor: f32 = if up { 1.05 } else { 1. / 1.05 };
                let min_factor = (min_size / b.size.width).max(min_size / b.size.height);
                let max_factor = self.resize_bounds.map_or(f32::INFINITY, |area| {
                    let limit = area.union(&b);
                    let width = (center.x - limit.left()).min(limit.right() - center.x) * 2.;
                    let height = (center.y - limit.top()).min(limit.bottom() - center.y) * 2.;
                    (width / b.size.width).min(height / b.size.height)
                });
                let factor = factor.max(min_factor.min(1.)).min(max_factor.max(1.));
                let dimensions = size(b.size.width * factor, b.size.height * factor);
                self.shapes[ix].bounds = Bounds::new(
                    center - point(dimensions.width / 2., dimensions.height / 2.),
                    dimensions,
                );
                let changed = self.shapes[ix] != before;
                self.commit_move(ix, before);
                changed
            }
        }
    }
}

pub(super) fn rasterize_outline(
    shape: &Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let ShapeParameters::Spotlight {
        ellipse, outline, ..
    } = shape.parameters
    else {
        return;
    };
    if !outline.enabled {
        return;
    }
    let width = outline.width.min(f32::from(
        shape.bounds.size.width.min(shape.bounds.size.height),
    ));
    if width <= 0. {
        return;
    }
    let mut border = shape.clone();
    border.kind = if ellipse {
        ShapeKind::Ellipse
    } else {
        ShapeKind::Rectangle
    };
    border.parameters = ShapeParameters::new(border.kind, width);
    Annotations::rasterize_ordinary(std::iter::once(&border), rgba, None, w, h, origin, scale);
}

pub(super) fn rasterize<'a>(
    shapes: impl Iterator<Item = &'a Shape>,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let shapes: Vec<_> = shapes.collect();
    if shapes.is_empty() {
        return;
    }
    let opacity = shapes.iter().map(|s| s.size()).fold(0_f32, f32::max) / 100.;
    for y in 0..h {
        for x in 0..w {
            let p = origin + point(px((x as f32 + 0.5) / scale), px((y as f32 + 0.5) / scale));
            if shapes.iter().any(|s| contains(s, p)) {
                continue;
            }
            let at = ((y * w + x) * 4) as usize;
            if rgba[at + 3] == 0 {
                continue;
            }
            for c in 0..3 {
                rgba[at + c] = (rgba[at + c] as f32 * (1. - opacity)).round() as u8;
            }
        }
    }
}

pub(super) fn contains(shape: &Shape, p: Point<Pixels>) -> bool {
    let ShapeParameters::Spotlight { ellipse, .. } = shape.parameters else {
        return false;
    };
    if !ellipse {
        return shape.bounds.contains(&p);
    }
    let half = shape.bounds.size;
    if half.width <= px(0.) || half.height <= px(0.) {
        return false;
    }
    let delta = p - shape.bounds.center();
    (f32::from(delta.x) / f32::from(half.width / 2.)).powi(2)
        + (f32::from(delta.y) / f32::from(half.height / 2.)).powi(2)
        <= 1.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{Annotations, ShapeKind};
    use gpui_kit::{Bounds, size};

    #[test]
    fn regions_share_one_mask_and_preserve_alpha() {
        let selection = Bounds::new(point(px(0.), px(0.)), size(px(32.), px(32.)));
        let mut annotations = Annotations::default();
        annotations.toggle(ShapeKind::Spotlight);
        annotations.apply_size(50.);
        for x in [4., 16.] {
            annotations.begin(point(px(x), px(4.)), selection, false);
            annotations.drag_to(point(px(x + 8.), px(12.)), selection, false);
            annotations.end();
        }
        let original = [100, 160, 200, 128].repeat(32 * 32);
        let mut pixels = original.clone();
        pixels[..4].fill(0);
        annotations.rasterize(&mut pixels, 32, 32, selection.origin, 1.);
        assert_eq!(&pixels[..4], &[0; 4]);
        for x in [6, 18] {
            let at = (6 * 32 + x) * 4;
            assert_eq!(&pixels[at..at + 4], &[100, 160, 200, 128]);
        }
        assert_eq!(&pixels[4..8], &[50, 80, 100, 128]);
        annotations.undo();
        let mut undone = original;
        annotations.rasterize(&mut undone, 32, 32, selection.origin, 1.);
        let at = (6 * 32 + 18) * 4;
        assert_eq!(&undone[at..at + 4], &[50, 80, 100, 128]);
        annotations.redo();
        assert_eq!(annotations.committed().len(), 2);
    }

    #[test]
    fn spotlight_opacity_dims_only_the_background() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(32.), px(32.)));
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Spotlight);
        a.begin(point(px(8.), px(8.)), area, false);
        a.drag_to(point(px(24.), px(24.)), area, false);
        a.end();
        a.select_index(0);
        for opacity in [0., 25., 75., 100.] {
            a.apply_size(opacity);
            a.end_size_drag();
            let mut pixels = [200, 120, 80, 255].repeat(32 * 32);
            a.rasterize(&mut pixels, 32, 32, area.origin, 1.);
            assert_eq!(pixels[0], (200. * (1. - opacity / 100.)) as u8);
            let inside = (16 * 32 + 16) * 4;
            assert_eq!(&pixels[inside..inside + 4], &[200, 120, 80, 255]);
        }
    }

    #[test]
    fn spotlight_outline_stays_inside_the_region_at_fractional_scale() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(32.), px(32.)));
        for ellipse in [false, true] {
            let mut a = Annotations::default();
            a.toggle(ShapeKind::Spotlight);
            a.apply_size(0.);
            a.set_spotlight_ellipse(ellipse);
            a.set_spotlight_outline(true);
            a.set_spotlight_outline_width(4.);
            a.begin(point(px(8.), px(8.)), area, false);
            a.drag_to(point(px(24.), px(24.)), area, false);
            a.end();
            let mut pixels = [255, 255, 255, 255].repeat(40 * 40);
            a.rasterize(&mut pixels, 40, 40, area.origin, 1.25);
            assert!(pixels.as_chunks::<4>().0.iter().any(|p| *p != [255; 4]));
            for y in 0..40 {
                for x in 0..40 {
                    if !(10..30).contains(&x) || !(10..30).contains(&y) {
                        let at = (y * 40 + x) * 4;
                        assert_eq!(&pixels[at..at + 4], &[255; 4], "{ellipse}: {x}, {y}");
                    }
                }
            }
        }
    }

    #[test]
    fn spotlight_outline_slider_is_one_history_step_and_keeps_other_properties() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(80.), px(80.)));
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Spotlight);
        a.begin(point(px(20.), px(20.)), area, false);
        a.drag_to(point(px(60.), px(60.)), area, false);
        a.end();
        a.select_index(0);
        a.set_spotlight_outline(true);
        let before = a.shapes[0].clone();
        for width in [4., 7., 10.] {
            a.set_spotlight_outline_width(width);
        }
        a.end_size_drag();
        let after = a.shapes[0].clone();
        assert_eq!(after.size(), before.size());
        assert_eq!(after.bounds, before.bounds);
        a.undo();
        assert_eq!(a.shapes[0], before);
        a.redo();
        assert_eq!(a.shapes[0], after);
        a.select_index(0);
        a.set_spotlight_outline(false);
        assert!(!a.step_spotlight(SpotlightAdjustment::Outline, true));
        a.set_spotlight_outline(true);
        assert_eq!(a.spotlight_outline(), (true, 10.));
    }

    #[test]
    fn spotlight_region_wheel_preserves_aspect_center_and_crop_bounds() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(80.), px(80.)));
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Spotlight);
        a.begin(point(px(10.), px(20.)), area, false);
        a.drag_to(point(px(70.), px(60.)), area, false);
        a.end();
        a.select_index(0);
        let before = a.shapes[0].clone();
        a.edit_within(area, |a| {
            assert!(a.step_spotlight(SpotlightAdjustment::Region, true))
        });
        let after = a.shapes[0].clone();
        assert_eq!(after.bounds.center(), before.bounds.center());
        assert_eq!(after.parameters, before.parameters);
        a.undo();
        assert_eq!(a.shapes[0], before);
        a.redo();
        assert_eq!(a.shapes[0], after);
        a.select_index(0);
        for _ in 0..100 {
            a.edit_within(area, |a| {
                a.step_spotlight(SpotlightAdjustment::Region, true);
            });
            let b = a.shapes[0].bounds;
            assert!(b.left() >= area.left() && b.right() <= area.right());
            assert!(b.top() >= area.top() && b.bottom() <= area.bottom());
            assert!((b.size.width / b.size.height - 1.5).abs() < 0.001);
        }
    }
}
