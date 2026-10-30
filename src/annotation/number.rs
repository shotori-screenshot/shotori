//! Sequence badges rendered by ab_glyph, shared by preview and PNG export.
use super::{Annotations, NumberFormat, Shape, ShapeKind, ShapeParameters, history::HistoryEntry};
use ab_glyph::{Font, FontRef, Glyph, ScaleFont};
use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};

impl Annotations {
    pub(crate) fn number_format(&self) -> NumberFormat {
        self.selected()
            .filter(|s| s.kind == ShapeKind::Number)
            .map(Shape::number_format)
            .unwrap_or(self.number_format)
    }

    pub(crate) fn set_number_format(&mut self, value: NumberFormat) {
        self.number_format = value;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Badge { format, .. } = &mut self.shapes[ix].parameters {
                *format = value;
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn number_value(&self) -> u32 {
        self.selected()
            .filter(|s| s.kind == ShapeKind::Number)
            .and_then(|s| s.number)
            .unwrap_or_else(|| self.next_number())
    }

    /// Change the selected badge or the next placement value as one undoable edit.
    pub(crate) fn set_number_value(&mut self, value: u32) {
        if value == 0 || value == self.number_value() {
            return;
        }
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            if self.shapes[ix].kind == ShapeKind::Number {
                let before = self.shapes[ix].clone();
                self.shapes[ix].number = Some(value);
                self.commit_move(ix, before);
            }
        } else {
            self.history.push(HistoryEntry::NumberCursor {
                before: self.number_override,
                after: Some(value),
            });
            self.number_override = Some(value);
            self.redo.clear();
        }
    }

    pub(crate) fn step_number_value(&mut self, up: bool) {
        if let Some(ix) = self.selected_index() {
            self.step_selected_number(ix, up);
            return;
        }
        let current = self.number_value();
        self.set_number_value(if up {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1).max(1)
        });
    }
}

impl Shape {
    pub(crate) fn number_tip(&self) -> Option<Point<Pixels>> {
        match self.parameters {
            ShapeParameters::Badge { tip, .. } => tip,
            _ => None,
        }
    }

    pub(super) fn set_number_tip(&mut self, value: Option<Point<Pixels>>) {
        if let ShapeParameters::Badge { tip, .. } = &mut self.parameters {
            *tip = value;
        }
    }

    /// Reuse the ordinary arrow outline for preview, export, picking and crop bounds.
    pub(crate) fn number_arrow(&self) -> Option<Shape> {
        let tip = self.number_tip()?;
        let center = self.bounds.center();
        let radius = f32::from(self.bounds.size.width) / 2.;
        let delta = tip - center;
        let distance = super::gesture::distance(center, tip);
        if distance <= radius + 2. {
            return None;
        }
        let start = center + delta * (radius / distance);
        Some(Shape {
            kind: ShapeKind::Arrow,
            bounds: Bounds::from_corners(
                point(start.x.min(tip.x), start.y.min(tip.y)),
                point(start.x.max(tip.x), start.y.max(tip.y)),
            ),
            parameters: ShapeParameters::Stroke {
                width: (f32::from(self.bounds.size.width) / 16.).max(1.),
                style: super::LineStyle::Solid,
                arrow: self.arrow_style(),
                ends: Default::default(),
            },
            points: vec![start, tip],
            number: None,
            text: None,
            text_style: Default::default(),
            color: self.color,
        })
    }

    /// Keep the badge at its press position and constrain the complete arrow outline.
    pub(super) fn drag_number_tip(&mut self, target: Point<Pixels>, area: Bounds<Pixels>) -> bool {
        let previous = self.number_tip();
        let center = self.bounds.center();
        self.set_number_tip(Some(target));
        let fits = |shape: &Shape| {
            let b = shape.ink_bounds();
            b.left() >= area.left()
                && b.right() <= area.right()
                && b.top() >= area.top()
                && b.bottom() <= area.bottom()
        };
        if !fits(self) {
            let (mut low, mut high) = (0., 1.);
            for _ in 0..24 {
                let fraction = (low + high) / 2.;
                self.set_number_tip(Some(center + (target - center) * fraction));
                if fits(self) {
                    low = fraction;
                } else {
                    high = fraction;
                }
            }
            self.set_number_tip(Some(center + (target - center) * low));
        }
        if self.number_arrow().is_none() {
            self.set_number_tip(None);
        }
        previous != self.number_tip()
    }
}

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        FontRef::try_from_slice(include_bytes!("../../assets/DejaVuSans-Bold.ttf"))
            .expect("bundled DejaVu Sans Bold is a valid font")
    })
}

fn layout(label: &str, diameter: f32, center: ab_glyph::Point) -> Vec<Glyph> {
    // Measure at a large size to keep whole-pixel ink-bound rounding negligible.
    let scaled = font().as_scaled(1000.);
    let mut advance = 0.;
    let mut previous = None;
    let mut min = ab_glyph::point(f32::INFINITY, f32::INFINITY);
    let mut max = ab_glyph::point(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut glyphs = Vec::new();
    for ch in label.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(previous) = previous {
            advance += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(1000., ab_glyph::point(advance, 0.));
        let bounds = scaled
            .outline_glyph(glyph.clone())
            .expect("bundled font covers decimal digits")
            .px_bounds();
        min.x = min.x.min(bounds.min.x);
        min.y = min.y.min(bounds.min.y);
        max.x = max.x.max(bounds.max.x);
        max.y = max.y.max(bounds.max.y);
        advance += scaled.h_advance(id);
        previous = Some(id);
        glyphs.push(glyph);
    }
    let factor = (diameter * 0.55 / (max.y - min.y)).min(diameter * 0.725 / (max.x - min.x));
    for glyph in &mut glyphs {
        glyph.scale = (1000. * factor).into();
        glyph.position.x = center.x + (glyph.position.x - (min.x + max.x) / 2.) * factor;
        glyph.position.y = center.y - (min.y + max.y) / 2. * factor;
    }
    glyphs
}

struct Badge {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn render(shape: &super::Shape, origin: Point<Pixels>, scale: f32) -> Badge {
    let x = f32::from(shape.bounds.left() - origin.x) * scale;
    let y = f32::from(shape.bounds.top() - origin.y) * scale;
    let diameter = f32::from(shape.bounds.size.width) * scale;
    let left = x.floor() as i32;
    let top = y.floor() as i32;
    let width = ((x + diameter).ceil() - left as f32) as u32;
    let height = ((y + diameter).ceil() - top as f32) as u32;
    let mut rgba = vec![0; (width * height * 4) as usize];
    let radius = diameter / 2.;
    let center = ab_glyph::point(x - left as f32 + radius, y - top as f32 + radius);
    let background = shape.color.to_be_bytes();
    // Only the circular background needs coverage calculation; ab_glyph draws all text.
    for row in 0..height {
        for col in 0..width {
            let mut coverage = 0.;
            for sample in 0..8 {
                let dy = row as f32 + (sample as f32 + 0.5) / 8. - center.y;
                if dy.abs() < radius {
                    let half = (radius * radius - dy * dy).sqrt();
                    coverage += ((center.x + half).min(col as f32 + 1.)
                        - (center.x - half).max(col as f32))
                    .clamp(0., 1.)
                        / 8.;
                }
            }
            let offset = ((row * width + col) * 4) as usize;
            rgba[offset..offset + 3].copy_from_slice(&background[..3]);
            rgba[offset + 3] = (coverage * 255.).round() as u8;
        }
    }
    let foreground = foreground(shape.color).to_be_bytes();
    for glyph in layout(
        &shape.number_format().label(shape.number.unwrap_or(1)),
        diameter,
        center,
    ) {
        let outline = font()
            .outline_glyph(glyph)
            .expect("bundled font covers decimal digits");
        let bounds = outline.px_bounds();
        outline.draw(|x, y, coverage| {
            let x = x as i32 + bounds.min.x as i32;
            let y = y as i32 + bounds.min.y as i32;
            if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                return;
            }
            let offset = ((y as u32 * width + x as u32) * 4) as usize;
            let coverage = coverage.clamp(0., 1.);
            for channel in 0..3 {
                rgba[offset + channel] = (rgba[offset + channel] as f32 * (1. - coverage)
                    + foreground[channel] as f32 * coverage)
                    .round() as u8;
            }
        });
    }
    Badge {
        left,
        top,
        width,
        height,
        rgba,
    }
}

pub(super) fn rasterize(
    shape: &super::Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    if let Some(arrow) = shape.number_arrow() {
        super::line::rasterize(&arrow, rgba, w, h, origin, scale);
    }
    let badge = render(shape, origin, scale);
    for row in 0..badge.height {
        let y = badge.top + row as i32;
        if y < 0 || y >= h as i32 {
            continue;
        }
        for col in 0..badge.width {
            let x = badge.left + col as i32;
            if x < 0 || x >= w as i32 {
                continue;
            }
            let src = ((row * badge.width + col) * 4) as usize;
            let dst = ((y as u32 * w + x as u32) * 4) as usize;
            if rgba[dst + 3] == 0 {
                continue;
            } // Preserve desktop gaps.
            let coverage = badge.rgba[src + 3] as f32 / 255.;
            for channel in 0..3 {
                rgba[dst + channel] = (rgba[dst + channel] as f32 * (1. - coverage)
                    + badge.rgba[src + channel] as f32 * coverage)
                    .round() as u8;
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct NumberImage {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) image: Arc<RenderImage>,
}

/// Owned by one overlay. Include subpixel position and DPI so moving a badge or
/// changing displays rerasterizes it. Discard entries no longer visible.
#[derive(Default)]
pub(crate) struct NumberCache {
    entries: HashMap<[u32; 8], NumberImage>,
}
impl NumberCache {
    pub(crate) fn prepare(
        &mut self,
        shapes: &[super::Shape],
        scale: f32,
    ) -> Vec<Option<NumberImage>> {
        let mut used = HashSet::new();
        let images = shapes
            .iter()
            .map(|shape| {
                if shape.kind != super::ShapeKind::Number {
                    return None;
                }
                let key = [
                    shape.number.unwrap_or(1),
                    shape.number_format() as u32,
                    shape.color,
                    f32::from(shape.bounds.left()).to_bits(),
                    f32::from(shape.bounds.top()).to_bits(),
                    f32::from(shape.bounds.size.width).to_bits(),
                    f32::from(shape.bounds.size.height).to_bits(),
                    scale.to_bits(),
                ];
                used.insert(key);
                Some(
                    self.entries
                        .entry(key)
                        .or_insert_with(|| {
                            let badge = render(shape, point(px(0.), px(0.)), scale);
                            NumberImage {
                                bounds: Bounds::new(
                                    point(
                                        px(badge.left as f32 / scale),
                                        px(badge.top as f32 / scale),
                                    ),
                                    size(
                                        px(badge.width as f32 / scale),
                                        px(badge.height as f32 / scale),
                                    ),
                                ),
                                image: crate::render_image::rgba_to_render_image(
                                    badge.rgba,
                                    badge.width,
                                    badge.height,
                                ),
                            }
                        })
                        .clone(),
                )
            })
            .collect();
        self.entries.retain(|key, _| used.contains(key));
        images
    }
}
pub(super) fn foreground(color: u32) -> u32 {
    let rgb = color.to_be_bytes();
    let linear = |v: u8| {
        let v = v as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    if 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]) > 0.179 {
        0x000000ff
    } else {
        0xffffffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn badge(number: u32, diameter: f32) -> super::super::Shape {
        super::super::Shape {
            kind: super::super::ShapeKind::Number,
            number: Some(number),
            text: None,
            text_style: Default::default(),
            bounds: Bounds::new(point(px(20.3), px(20.7)), size(px(diameter), px(diameter))),
            color: 0x0000ffff,
            parameters: crate::annotation::ShapeParameters::new(
                super::super::ShapeKind::Number,
                3.,
            ),
            points: Vec::new(),
        }
    }
    #[test]
    fn font_covers_all_digits_and_multidigit_text_fits_each_size() {
        for number in (0..=12).chain([99, 100, 1000, u32::MAX]) {
            for diameter in [16., 24., 32., 40.] {
                for scale in [1., 1.25, 1.73, 2.] {
                    let diameter = diameter * scale;
                    for glyph in layout(
                        &number.to_string(),
                        diameter,
                        ab_glyph::point(diameter / 2., diameter / 2.),
                    ) {
                        let outline = font().outline_glyph(glyph).unwrap();
                        let bounds = outline.px_bounds();
                        assert!(bounds.min.x >= 0. && bounds.min.y >= 0.);
                        assert!(bounds.max.x <= diameter.ceil() && bounds.max.y <= diameter.ceil());
                    }
                }
            }
        }
        assert_eq!(foreground(0xffffffff), 0x000000ff);
        assert_eq!(foreground(0x000000ff), 0xffffffff);
    }
    #[test]
    fn font_rasterization_preserves_holes_and_desktop_gaps() {
        for scale in [1., 1.25, 1.73, 2.] {
            let w = (80. * scale) as u32;
            let mut pixels = [12, 23, 34, 255].repeat((w * w) as usize);
            let at =
                |x: f32, y: f32| (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
            let gap = at(22., 36.);
            pixels[gap..gap + 4].fill(0);
            rasterize(
                &badge(0, 32.),
                &mut pixels,
                w,
                w,
                point(px(0.), px(0.)),
                scale,
            );
            assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
            assert_eq!(&pixels[at(36., 36.)..at(36., 36.) + 4], &[0, 0, 255, 255]);
            assert_eq!(&pixels[at(5., 5.)..at(5., 5.) + 4], &[12, 23, 34, 255]);
            assert!(pixels.as_chunks::<4>().0.contains(&[255, 255, 255, 255]));
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255 && p[2] == 255)
            );
        }
    }
    #[test]
    fn export_composites_the_same_badge_pixels_and_handles_negative_coordinates() {
        for scale in [1.25, 1.73] {
            let mut shape = badge(12, 32.);
            shape.bounds.origin = point(px(-7.3), px(-6.7));
            let source = render(&shape, point(px(0.), px(0.)), scale);
            let mut dest = [40, 50, 60, 255].repeat(80 * 80);
            rasterize(&shape, &mut dest, 80, 80, point(px(0.), px(0.)), scale);
            for y in 0..source.height {
                for x in 0..source.width {
                    let dx = x as i32 + source.left;
                    let dy = y as i32 + source.top;
                    if dx < 0 || dy < 0 {
                        continue;
                    }
                    let src = ((y * source.width + x) * 4) as usize;
                    let dst = ((dy * 80 + dx) * 4) as usize;
                    let alpha = source.rgba[src + 3] as f32 / 255.;
                    for (channel, background) in [40., 50., 60.].into_iter().enumerate() {
                        assert_eq!(
                            dest[dst + channel],
                            (background * (1. - alpha) + source.rgba[src + channel] as f32 * alpha)
                                .round() as u8
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn preview_cache_reuses_images_and_invalidates_dpi_style_and_position() {
        let mut cache = NumberCache::default();
        let mut shapes = vec![badge(1, 32.)];
        let first = cache.prepare(&shapes, 1.25)[0].clone().unwrap();
        let same = cache.prepare(&shapes, 1.25)[0].clone().unwrap();
        assert!(Arc::ptr_eq(&first.image, &same.image));
        let scaled = cache.prepare(&shapes, 2.)[0].clone().unwrap();
        assert!(!Arc::ptr_eq(&same.image, &scaled.image));
        shapes[0].color = 0xff0000ff;
        let recolored = cache.prepare(&shapes, 2.)[0].clone().unwrap();
        assert!(!Arc::ptr_eq(&scaled.image, &recolored.image));
        shapes[0].bounds.origin.x += px(0.3);
        let moved = cache.prepare(&shapes, 2.)[0].clone().unwrap();
        assert!(!Arc::ptr_eq(&recolored.image, &moved.image));
        assert_eq!(cache.entries.len(), 1);
        cache.prepare(&[], 2.);
        assert!(cache.entries.is_empty());
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;
    use crate::annotation::{Annotations, NumberFormat, ShapeKind};

    #[test]
    fn numeral_boundaries_are_bounded_and_unambiguous() {
        for (value, expected) in [(1, "A"), (26, "Z"), (27, "AA"), (52, "AZ"), (703, "AAA")] {
            assert_eq!(NumberFormat::Alphabetic.label(value), expected);
        }
        for (value, expected) in [
            (4, "IV"),
            (9, "IX"),
            (49, "XLIX"),
            (3999, "MMMCMXCIX"),
            (4000, "4000"),
        ] {
            assert_eq!(NumberFormat::Roman.label(value), expected);
        }
        assert!(NumberFormat::Alphabetic.label(u32::MAX).len() <= 7);
    }

    #[test]
    fn format_changes_invalidate_preview_and_keep_selection_and_history() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Number);
        let area = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        marks.step_number_value(true);
        assert_eq!(marks.next_number(), 2);
        marks.begin(point(px(40.), px(40.)), area, false);
        marks.end();
        assert_eq!(marks.next_number(), 3);
        marks.select_index(0);
        let mut cache = NumberCache::default();
        let old = cache.prepare(&marks.shapes, 1.25)[0].clone().unwrap();
        marks.set_number_format(NumberFormat::Alphabetic);
        assert_eq!(marks.selected_index(), Some(0));
        let changed = cache.prepare(&marks.shapes, 1.25)[0].clone().unwrap();
        assert_ne!(old.image.as_bytes(0), changed.image.as_bytes(0));
        marks.undo();
        assert_eq!(marks.shapes[0].number_format(), NumberFormat::Decimal);
        marks.redo();
        assert_eq!(marks.shapes[0].number_format(), NumberFormat::Alphabetic);
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    fn place(marks: &mut Annotations, repeat: bool) {
        let crop = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        marks.begin(point(px(80.), px(80.)), crop, repeat);
        marks.end();
    }

    fn values(marks: &Annotations) -> Vec<u32> {
        marks
            .committed()
            .iter()
            .filter_map(|shape| shape.number)
            .collect()
    }

    #[test]
    fn explicit_number_cursor_follows_placement_undo_redo_and_branching() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Number);
        marks.set_number_value(40);
        place(&mut marks, false);
        assert_eq!(values(&marks), [40]);
        assert_eq!(marks.next_number(), 41);
        marks.undo();
        assert_eq!(marks.next_number(), 40);
        marks.redo();
        assert_eq!(marks.next_number(), 41);
        marks.undo();
        marks.undo();
        assert_eq!(marks.next_number(), 1);
        marks.redo();
        assert_eq!(marks.next_number(), 40);
        marks.set_number_value(7);
        marks.redo();
        assert!(marks.committed().is_empty());
        place(&mut marks, false);
        assert_eq!(values(&marks), [7]);
        marks.reset();
        assert_eq!(marks.next_number(), 1);
    }

    #[test]
    fn repeated_badges_and_cancelled_placements_do_not_consume_explicit_cursor() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Number);
        place(&mut marks, false);
        marks.set_number_value(12);
        place(&mut marks, true);
        assert_eq!(values(&marks), [1, 1]);
        assert_eq!(marks.next_number(), 12);
        marks.undo();
        marks.redo();
        assert_eq!(marks.next_number(), 12);
        let crop = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        marks.begin(point(px(80.), px(80.)), crop, false);
        marks.cancel();
        assert_eq!(marks.next_number(), 12);
    }

    #[test]
    fn direct_value_edits_reject_zero_keep_selection_and_undo_once() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Number);
        marks.set_number_value(0);
        place(&mut marks, false);
        marks.select_index(0);
        marks.set_number_value(9);
        marks.set_number_value(9);
        marks.set_number_value(0);
        assert_eq!(marks.selected_index(), Some(0));
        assert_eq!(values(&marks), [9]);
        marks.undo();
        assert_eq!(values(&marks), [1]);
        marks.redo();
        marks.select_index(0);
        marks.set_number_value(u32::MAX);
        marks.step_number_value(true);
        marks.undo();
        assert_eq!(values(&marks), [9]);
    }

    #[test]
    fn deletion_renumbers_values_and_cursor_in_one_reversible_operation() {
        for enabled in [false, true] {
            let mut marks = Annotations {
                renumber_on_delete: enabled,
                ..Default::default()
            };
            marks.toggle(ShapeKind::Number);
            for value in [4, 2, 4, 9] {
                marks.set_number_value(value);
                place(&mut marks, false);
            }
            marks.set_number_value(20);
            let original = marks.committed().to_vec();
            marks.delete_shape(1);
            assert_eq!(
                values(&marks),
                if enabled {
                    vec![3, 3, 8]
                } else {
                    vec![4, 4, 9]
                }
            );
            assert_eq!(marks.next_number(), if enabled { 19 } else { 20 });
            marks.undo();
            assert_eq!(marks.committed(), original);
            assert_eq!(marks.next_number(), 20);
            marks.redo();
            assert_eq!(
                values(&marks),
                if enabled {
                    vec![3, 3, 8]
                } else {
                    vec![4, 4, 9]
                }
            );
            place(&mut marks, false);
            marks.undo();
            marks.undo();
            assert_eq!(marks.committed(), original);
            assert_eq!(marks.next_number(), 20);
        }
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::annotation::{ArrowStyle, select};

    fn area() -> Bounds<Pixels> {
        Bounds::new(point(px(-40.), px(-20.)), size(px(240.), px(180.)))
    }

    fn draft(marks: &mut Annotations, target: Point<Pixels>) {
        marks.toggle(ShapeKind::Number);
        marks.set_tool_size(32.);
        marks.begin(point(px(20.), px(60.)), area(), false);
        marks.drag_to(target, area(), false);
    }

    #[test]
    fn number_pointer_keeps_badge_at_press_and_renders_during_the_gesture() {
        let mut marks = Annotations::default();
        draft(&mut marks, point(px(160.), px(60.)));
        let shape = marks.visible().next().unwrap().clone();
        assert_eq!(shape.bounds.center(), point(px(20.), px(60.)));
        assert_eq!(shape.number_tip(), Some(point(px(160.), px(60.))));
        assert!(select::shape_hit(&shape, point(px(90.), px(60.))));
        assert!(!select::shape_hit(&shape, point(px(90.), px(100.))));
        assert!(
            !shape
                .number_arrow()
                .unwrap()
                .line_paths(point(px(0.), px(0.)))
                .is_empty()
        );
        for scale in [1., 1.25, 1.5, 2.] {
            let width = (240. * scale) as u32;
            let height = (180. * scale) as u32;
            let mut preview = vec![255; (width * height * 4) as usize];
            Annotations::rasterize_shapes(
                std::iter::once(&shape),
                &mut preview,
                width,
                height,
                area().origin,
                scale,
            );
            let offset = (((80. * scale) as u32 * width + (130. * scale) as u32) * 4) as usize;
            assert_eq!(&preview[offset..offset + 4], &shape.color.to_be_bytes());
            let mut translated = shape.clone();
            translated.translate(-area().origin);
            let mut local = vec![255; preview.len()];
            Annotations::rasterize_shapes(
                std::iter::once(&translated),
                &mut local,
                width,
                height,
                point(px(0.), px(0.)),
                scale,
            );
            assert_eq!(
                preview, local,
                "badge and arrow share output coordinates at {scale}"
            );
            marks.end();
            let mut exported = vec![255; preview.len()];
            Annotations::rasterize_shapes(
                marks.committed().iter(),
                &mut exported,
                width,
                height,
                area().origin,
                scale,
            );
            assert_eq!(
                preview, exported,
                "release must not change the arrow at {scale}"
            );
        }
        assert_eq!(marks.next_number(), 2);
        marks.undo();
        assert!(marks.committed().is_empty());
        marks.redo();
        assert_eq!(marks.committed(), &[shape]);
    }

    #[test]
    fn number_pointer_silhouettes_handles_sizes_and_moves_keep_crop_bounds() {
        for style in [
            ArrowStyle::Open,
            ArrowStyle::DoubleOpen,
            ArrowStyle::Outline,
            ArrowStyle::Filled,
            ArrowStyle::Dimension,
            ArrowStyle::Triangle,
            ArrowStyle::DoubleTriangle,
        ] {
            for direction in 0..16 {
                let mut marks = Annotations::default();
                let (sin, cos) = (direction as f32 * std::f32::consts::TAU / 16.).sin_cos();
                draft(
                    &mut marks,
                    point(px(20. + cos * 500.), px(60. + sin * 500.)),
                );
                marks.end();
                marks.select_index(0);
                marks.edit_within(area(), |a| a.set_arrow_style(style));
                let before = marks.selected().unwrap().clone();
                let b = before.ink_bounds();
                assert!(b.left() >= area().left() && b.right() <= area().right());
                assert!(b.top() >= area().top() && b.bottom() <= area().bottom());
                assert_eq!(before.bounds.center(), point(px(20.), px(60.)));
                marks.edit_within(area(), |a| a.apply_size(64.));
                let b = marks.selected().unwrap().ink_bounds();
                assert!(b.left() >= area().left() && b.right() <= area().right());
                assert!(b.top() >= area().top() && b.bottom() <= area().bottom());
                marks.place_handle(0, 0, &before, point(px(900.), px(900.)), area());
                let b = marks.selected().unwrap().ink_bounds();
                assert!(b.left() >= area().left() && b.right() <= area().right());
                assert!(b.top() >= area().top() && b.bottom() <= area().bottom());
                marks.place_shape(0, &before, point(px(5.), px(-3.)));
                let moved = marks.selected().unwrap();
                assert_eq!(
                    moved.bounds.origin,
                    before.bounds.origin + point(px(5.), px(-3.))
                );
                assert_eq!(
                    moved.number_tip(),
                    before.number_tip().map(|p| p + point(px(5.), px(-3.)))
                );
                marks.place_shape(0, &before, point(px(0.), px(0.)));
                assert_eq!(marks.selected(), Some(&before));
            }
        }
    }

    #[test]
    fn number_pointer_jitter_cancels_and_style_history_remains_independent() {
        let mut marks = Annotations::default();
        draft(&mut marks, point(px(23.), px(63.)));
        assert!(marks.visible().next().unwrap().number_tip().is_none());
        marks.drag_to(point(px(160.), px(60.)), area(), false);
        assert!(marks.visible().next().unwrap().number_arrow().is_some());
        marks.drag_to(point(px(20.), px(60.)), area(), false);
        assert!(marks.visible().next().unwrap().number_arrow().is_none());
        marks.drag_to(point(px(160.), px(60.)), area(), false);
        marks.end();
        marks.select_index(0);
        let before = marks.selected().unwrap().clone();
        marks.set_arrow_style(ArrowStyle::Dimension);
        let after = marks.selected().unwrap().clone();
        assert_eq!(marks.selected_index(), Some(0));
        assert_eq!(after.arrow_style(), ArrowStyle::Dimension);
        assert_eq!(after.bounds, before.bounds);
        marks.undo();
        assert_eq!(marks.committed(), &[before]);
        marks.redo();
        assert_eq!(marks.committed(), &[after]);
        marks.toggle(ShapeKind::Arrow);
        assert_eq!(marks.arrow_style(), ArrowStyle::Open);
    }
}
