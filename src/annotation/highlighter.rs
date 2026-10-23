//! Rasterize each translucent stroke once so joins never receive extra coats.
use super::{Shape, ShapeKind, line};
use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use std::sync::Arc;

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
    let radius = px(shape.width / 2.);
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
        pixels[offset + 3] = (coverage * color[3] as f32).round() as u8;
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
            bounds: Bounds::default(),
            color: 0xffd43b60,
            width: 20.,
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
