//! Rasterize each translucent stroke once so joins never receive extra coats.
use super::{Annotations, Shape, ShapeKind, ShapeParameters, line};
use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HighlightBlend {
    #[default]
    Multiply,
    Translucent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HighlighterOptions {
    pub(crate) rectangle: bool,
    pub(crate) blend: HighlightBlend,
}

impl Shape {
    pub(crate) fn highlighter_options(&self) -> HighlighterOptions {
        match self.parameters {
            ShapeParameters::Highlighter { options, .. } => options,
            _ => HighlighterOptions::default(),
        }
    }

    pub(crate) fn needs_composited_ink(&self) -> bool {
        self.kind == ShapeKind::Highlighter
            && (self.highlighter_options().rectangle
                || self.highlighter_options().blend == HighlightBlend::Multiply)
    }
}

impl Annotations {
    pub(crate) fn highlighter_options(&self) -> HighlighterOptions {
        self.selected()
            .filter(|s| s.kind == ShapeKind::Highlighter)
            .map(Shape::highlighter_options)
            .unwrap_or(self.highlighter)
    }

    pub(crate) fn set_highlighter_rectangle(&mut self, rectangle: bool) {
        // Changing the drawing gesture affects future strokes, never their geometry.
        self.finish_interaction();
        self.tool = Some(ShapeKind::Highlighter);
        self.highlighter.rectangle = rectangle;
    }

    pub(crate) fn set_highlighter_blend(&mut self, blend: HighlightBlend) {
        self.highlighter.blend = blend;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Highlighter { options, .. } = &mut self.shapes[ix].parameters {
                options.blend = blend;
            }
            self.commit_move(ix, before);
        }
    }
}

/// Blend the union coverage once, preserving captured transparent gaps.
pub(super) fn blend_pixel(shape: &Shape, pixel: &mut [u8], coverage: f32) {
    if pixel[3] == 0 {
        return;
    }
    let color = shape.color.to_be_bytes();
    let multiply = shape.kind == ShapeKind::Highlighter
        && shape.highlighter_options().blend == HighlightBlend::Multiply;
    let opacity = if shape.kind == ShapeKind::Highlighter && !multiply {
        96. / 255.
    } else {
        1.
    };
    let alpha = coverage * color[3] as f32 / 255. * opacity;
    for channel in 0..3 {
        let background = pixel[channel] as f32;
        let foreground = if multiply {
            background * color[channel] as f32 / 255.
        } else {
            color[channel] as f32
        };
        pixel[channel] = (background * (1. - alpha) + foreground * alpha).round() as u8;
    }
}

#[derive(Clone)]
pub(crate) struct StrokeImage {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) image: Arc<RenderImage>,
}

struct Entry {
    shape: Shape,
    scale: f32,
    viewport: Bounds<Pixels>,
    image: Option<StrokeImage>,
}

#[derive(Default)]
pub(crate) struct HighlighterCache {
    entries: Vec<Option<Entry>>,
}

impl HighlighterCache {
    pub(crate) fn prepare(
        &mut self,
        shapes: &[Shape],
        scale: f32,
        viewport: Bounds<Pixels>,
    ) -> Vec<Option<StrokeImage>> {
        self.entries.resize_with(shapes.len(), || None);
        shapes
            .iter()
            .zip(&mut self.entries)
            .map(|(shape, entry)| {
                if shape.kind != ShapeKind::Highlighter {
                    *entry = None;
                    return None;
                }
                if !entry.as_ref().is_some_and(|e| {
                    e.shape == *shape && e.scale == scale && e.viewport == viewport
                }) {
                    *entry = Some(Entry {
                        shape: shape.clone(),
                        scale,
                        viewport,
                        image: render(shape, scale, viewport),
                    });
                }
                entry.as_ref().and_then(|e| e.image.clone())
            })
            .collect()
    }
}

fn render(shape: &Shape, scale: f32, viewport: Bounds<Pixels>) -> Option<StrokeImage> {
    let first = *shape.points.first()?;
    let (min, max) = shape.points.iter().fold((first, first), |(min, max), p| {
        (
            point(min.x.min(p.x), min.y.min(p.y)),
            point(max.x.max(p.x), max.y.max(p.y)),
        )
    });
    let radius = px(shape.size() / 2.);
    // Clip before allocating: a stroke can span several distant outputs.
    let left = (f32::from((min.x - radius).max(viewport.left())) * scale).floor();
    let top = (f32::from((min.y - radius).max(viewport.top())) * scale).floor();
    let right = (f32::from((max.x + radius).min(viewport.right())) * scale).ceil();
    let bottom = (f32::from((max.y + radius).min(viewport.bottom())) * scale).ceil();
    if right <= left || bottom <= top {
        return None;
    }
    let w = (right - left) as u32;
    let h = (bottom - top) as u32;
    let origin: Point<Pixels> = point(px(left / scale), px(top / scale));
    let mut pixels = vec![0; w as usize * h as usize * 4];
    let color = shape.color.to_be_bytes();
    line::coverage(shape, w, h, origin, scale, |offset, coverage| {
        pixels[offset..offset + 3].copy_from_slice(&color[..3]);
        pixels[offset + 3] = (coverage * color[3] as f32 * 96. / 255.).round() as u8;
    });
    Some(StrokeImage {
        bounds: Bounds::new(origin, size(px(w as f32 / scale), px(h as f32 / scale))),
        image: crate::render_image::rgba_to_render_image(pixels, w, h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stroke() -> Shape {
        Shape {
            kind: ShapeKind::Highlighter,
            number: None,
            text: None,
            text_style: Default::default(),
            bounds: Bounds::default(),
            color: 0xffd43b60,
            parameters: crate::annotation::ShapeParameters::new(ShapeKind::Highlighter, 20.),
            points: [(10., 30.), (70., 30.), (10., 30.), (40., 10.), (40., 60.)]
                .map(|(x, y)| point(px(x), px(y)))
                .to_vec(),
        }
    }
    fn viewport() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(80.), px(80.)))
    }
    #[test]
    fn retracing_and_crossings_are_one_coat_but_separate_strokes_stack() {
        let shape = stroke();
        let mut pixels = vec![255; 80 * 80 * 4];
        line::rasterize(&shape, &mut pixels, 80, 80, viewport().origin, 1.);
        let at = |x: usize, y: usize| (y * 80 + x) * 4;
        assert_eq!(&pixels[at(40, 30)..at(40, 30) + 4], &[255, 239, 181, 255]);
        assert_eq!(&pixels[at(65, 30)..at(65, 30) + 4], &[255, 239, 181, 255]);
        pixels[at(40, 30) + 3] = 0;
        line::rasterize(&shape, &mut pixels, 80, 80, viewport().origin, 1.);
        assert_eq!(pixels[at(40, 30) + 3], 0);
        assert!(pixels[at(65, 30) + 2] < 181);
    }
    #[test]
    fn preview_matches_export_at_fractional_scales_and_cache_invalidates() {
        let mut shape = stroke();
        if let ShapeParameters::Highlighter { options, .. } = &mut shape.parameters {
            options.blend = HighlightBlend::Translucent;
        }
        shape.color |= 255;
        let mut cache = HighlighterCache::default();
        for scale in [1., 1.25, 1.73, 2.] {
            let preview = cache.prepare(&[shape.clone()], scale, viewport())[0]
                .clone()
                .unwrap();
            let same = cache.prepare(&[shape.clone()], scale, viewport())[0]
                .clone()
                .unwrap();
            assert!(Arc::ptr_eq(&preview.image, &same.image));
            let w = (f32::from(preview.bounds.size.width) * scale).round() as u32;
            let h = (f32::from(preview.bounds.size.height) * scale).round() as u32;
            let mut dest = [40, 50, 60, 255].repeat((w * h) as usize);
            line::rasterize(&shape, &mut dest, w, h, preview.bounds.origin, scale);
            for (src, dst) in preview
                .image
                .as_bytes(0)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dest.as_chunks::<4>().0.iter())
            {
                assert!(src[3] <= 96);
                let alpha = src[3] as f32 / 255.;
                for (channel, bg) in [40., 50., 60.].into_iter().enumerate() {
                    let expected =
                        (bg * (1. - alpha) + src[2 - channel] as f32 * alpha).round() as i16;
                    assert!((dst[channel] as i16 - expected).abs() <= 1);
                }
            }
        }
        let old = cache.prepare(&[shape.clone()], 1., viewport())[0]
            .clone()
            .unwrap();
        shape.points.push(point(px(75.), px(75.)));
        let changed = cache.prepare(&[shape], 1., viewport())[0].clone().unwrap();
        assert!(!Arc::ptr_eq(&old.image, &changed.image));
        cache.prepare(&[], 1., viewport());
        assert!(cache.entries.is_empty());
    }
}
