//! Sequence badges rendered by ab_glyph, shared by preview and PNG export.
use ab_glyph::{Font, FontRef, Glyph, ScaleFont};
use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        FontRef::try_from_slice(include_bytes!("../../assets/DejaVuSans-Bold.ttf"))
            .expect("bundled DejaVu Sans Bold is a valid font")
    })
}

fn layout(number: u32, diameter: f32, center: ab_glyph::Point) -> Vec<Glyph> {
    // Measure at a large size to keep whole-pixel ink-bound rounding negligible.
    let scaled = font().as_scaled(1000.);
    let mut advance = 0.;
    let mut previous = None;
    let mut min = ab_glyph::point(f32::INFINITY, f32::INFINITY);
    let mut max = ab_glyph::point(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut glyphs = Vec::new();
    for ch in number.to_string().chars() {
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
    for glyph in layout(shape.number.unwrap_or(1), diameter, center) {
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
    entries: HashMap<[u32; 7], NumberImage>,
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
            bounds: Bounds::new(point(px(20.3), px(20.7)), size(px(diameter), px(diameter))),
            color: 0x0000ffff,
            width: 3.,
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
                        number,
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
