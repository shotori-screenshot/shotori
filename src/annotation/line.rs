//! Shared rounded stroke geometry for line preview and PNG rasterization.
use gpui_kit::{Path, PathBuilder, Pixels, Point, point, px};

/// A stroke is a union of capsules: round endpoints and round joins, including
/// reversals and self-intersections. Both rendering paths use these polygons.
fn polygons(points: &[Point<Pixels>], width: f32) -> Vec<Vec<Point<Pixels>>> {
    if let [center] = points {
        return vec![
            (0..32)
                .map(|i| {
                    let (sin, cos) = (i as f32 * std::f32::consts::TAU / 32.).sin_cos();
                    *center + point(px(cos * width / 2.), px(sin * width / 2.))
                })
                .collect(),
        ];
    }
    points
        .windows(2)
        .filter_map(|pair| {
            let delta = pair[1] - pair[0];
            let dx = f32::from(delta.x);
            let dy = f32::from(delta.y);
            if dx.hypot(dy) < 0.001 {
                return None;
            }
            let angle = dy.atan2(dx);
            let mut polygon = Vec::with_capacity(34);
            for (center, start) in [
                (pair[1], angle - std::f32::consts::FRAC_PI_2),
                (pair[0], angle + std::f32::consts::FRAC_PI_2),
            ] {
                for i in 0..=16 {
                    let (sin, cos) = (start + i as f32 * std::f32::consts::PI / 16.).sin_cos();
                    polygon.push(center + point(px(cos * width / 2.), px(sin * width / 2.)));
                }
            }
            Some(polygon)
        })
        .collect()
}

/// Keep the tip exactly at the release position; shorten both head and shaft
/// proportionally for small arrows instead of letting the head point backwards.
///
/// `pub(super)`: the visual outline polygons are also the hit region and
/// the selection-highlight geometry ("what you see is what you click").
pub(super) fn geometry(
    points: &[Point<Pixels>],
    width: f32,
    arrow: bool,
) -> Vec<Vec<Point<Pixels>>> {
    if !arrow {
        return polygons(points, width);
    }
    let [start, tip] = points else {
        return Vec::new();
    };
    let delta = *tip - *start;
    let length = f32::from(delta.x).hypot(f32::from(delta.y));
    if length < 0.001 {
        return Vec::new();
    }
    let unit = delta / length;
    let normal = point(-unit.y, unit.x);
    let head = (width * 4.).max(10.).min(length * 0.6);
    let shaft = width.min(length * 0.25);
    let base = *tip - unit * head;
    let mut result = polygons(&[*start, base], shaft);
    result.push(vec![
        *tip,
        base + normal * (head * 0.45),
        base - normal * (head * 0.45),
    ]);
    result
}

pub(super) fn paths(shape: &super::Shape, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
    geometry(
        &shape.points,
        shape.width,
        shape.kind == super::ShapeKind::Arrow,
    )
    .into_iter()
    .filter_map(|polygon| {
        let mut builder = PathBuilder::fill();
        builder.move_to(polygon[0] + offset);
        for p in &polygon[1..] {
            builder.line_to(*p + offset);
        }
        builder.close();
        builder.build().ok()
    })
    .collect()
}

pub(super) fn rasterize(
    shape: &super::Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let color = shape.color.to_be_bytes();
    coverage(shape, w, h, origin, scale, |offset, coverage| {
        if rgba[offset + 3] == 0 {
            return;
        }
        let alpha = coverage * color[3] as f32 / 255.;
        for channel in 0..3 {
            rgba[offset + channel] = (rgba[offset + channel] as f32 * (1. - alpha)
                + color[channel] as f32 * alpha)
                .round() as u8;
        }
    });
}

/// Union all segments before blending: retracing within one gesture is one coat.
pub(super) fn coverage(
    shape: &super::Shape,
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
    mut paint_pixel: impl FnMut(usize, f32),
) {
    // Build in logical units before scaling so the minimum head size scales too.
    let polygons = geometry(
        &shape.points,
        shape.width,
        shape.kind == super::ShapeKind::Arrow,
    )
    .into_iter()
    .map(|polygon| {
        polygon
            .into_iter()
            .map(|p| (p - origin) * scale)
            .collect::<Vec<_>>()
    })
    .collect::<Vec<_>>();
    if polygons.is_empty() {
        return;
    }
    let vertices = || polygons.iter().flatten();
    let top = vertices()
        .map(|p| f32::from(p.y))
        .fold(f32::INFINITY, f32::min)
        .floor()
        .clamp(0., h as f32) as usize;
    let bottom = vertices()
        .map(|p| f32::from(p.y))
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0., h as f32) as usize;
    let left = vertices()
        .map(|p| f32::from(p.x))
        .fold(f32::INFINITY, f32::min)
        .floor()
        .clamp(0., w as f32) as usize;
    let right = vertices()
        .map(|p| f32::from(p.x))
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0., w as f32) as usize;
    let vertical_bounds: Vec<_> = polygons
        .iter()
        .map(|polygon| {
            polygon
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(top, bottom), p| {
                    (top.min(f32::from(p.y)), bottom.max(f32::from(p.y)))
                })
        })
        .collect();
    // Per-edge y-range, precomputed once: each scanline sample then tests
    // only the few edges that actually cross it (a 34-gon capsule usually
    // contributes ~4-8 of its 34 edges to one sample row, not all of them).
    // (ymin, ymax, xa, ya, xb, yb); the crossing test `y >= ymin && y < ymax`
    // is bit-identical to the original half-open `(ay<=y && y<by) || …`.
    type Edge = (f32, f32, f32, f32, f32, f32); // (ymin, ymax, xa, ya, xb, yb)
    let edges: Vec<Vec<Edge>> = polygons
        .iter()
        .map(|polygon| {
            polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .map(|(a, b)| {
                    let ay = f32::from(a.y);
                    let by = f32::from(b.y);
                    (
                        ay.min(by),
                        ay.max(by),
                        f32::from(a.x),
                        ay,
                        f32::from(b.x),
                        by,
                    )
                })
                .collect()
        })
        .collect();
    // Sweep the polygons by their first covered row. A long freehand stroke
    // should not test every segment on every scanline of its bounding box.
    let mut starts: Vec<_> = vertical_bounds
        .iter()
        .enumerate()
        .map(|(index, &(top, _))| (top.floor().max(0.) as usize, index))
        .collect();
    starts.sort_unstable_by_key(|&(row, _)| row);
    let mut cursor = 0;
    let mut active = Vec::new();
    let mut coverage = vec![0_f32; right - left];
    let mut intervals = Vec::with_capacity(polygons.len());
    for row in top..bottom {
        while cursor < starts.len() && starts[cursor].0 <= row {
            active.push(starts[cursor].1);
            cursor += 1;
        }
        active.retain(|&index| vertical_bounds[index].1 > row as f32);
        coverage.fill(0.);
        for sample in 0..8 {
            let y = row as f32 + (sample as f32 + 0.5) / 8.;
            intervals.clear();
            for &index in &active {
                let (top, bottom) = vertical_bounds[index];
                if y < top || y >= bottom {
                    continue;
                }
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for &(ymin, ymax, xa, ya, xb, yb) in &edges[index] {
                    if y >= ymin && y < ymax {
                        let x = xa + (y - ya) / (yb - ya) * (xb - xa);
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
                if lo < hi {
                    intervals.push((lo, hi));
                }
            }
            intervals.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            // Union before blending so joints/crossings never accumulate opacity.
            let mut union: Option<(f32, f32)> = None;
            let mut paint = |start: f32, end: f32| {
                let first = start.floor().clamp(left as f32, right as f32) as usize;
                let last = end.ceil().clamp(left as f32, right as f32) as usize;
                for x in first..last {
                    coverage[x - left] +=
                        (end.min(x as f32 + 1.) - start.max(x as f32)).max(0.) / 8.;
                }
            };
            for &(start, end) in &intervals {
                match union {
                    Some((lo, hi)) if start <= hi => union = Some((lo, hi.max(end))),
                    Some((lo, hi)) => {
                        paint(lo, hi);
                        union = Some((start, end));
                    }
                    None => union = Some((start, end)),
                }
            }
            if let Some((start, end)) = union {
                paint(start, end);
            }
        }
        for (ix, coverage) in coverage.iter().copied().enumerate() {
            let offset = (row * w as usize + left + ix) * 4;
            if coverage > 0. {
                paint_pixel(offset, coverage.min(1.));
            }
        }
    }
}

/// Coverage intervals at the same eight vertical samples used by full export.
/// Keeping the union (rather than blending new segments over old pixels) avoids
/// dark seams when translucent strokes retrace or intersect themselves.
#[derive(Default)]
pub(crate) struct StrokePreview {
    key: Option<StrokeKey>,
    points: Vec<Point<Pixels>>,
    rows: Vec<Vec<(f32, f32)>>,
    pixels: Vec<u8>,
}

#[derive(PartialEq)]
struct StrokeKey {
    dimensions: (u32, u32),
    origin: Point<Pixels>,
    scale: f32,
    width: f32,
    color: u32,
    kind: super::ShapeKind,
}

impl StrokePreview {
    pub(crate) fn render(
        &mut self,
        shape: &super::Shape,
        base: &[u8],
        dimensions: (u32, u32),
        origin: Point<Pixels>,
        scale: f32,
    ) -> &[u8] {
        let (w, h) = dimensions;
        let key = StrokeKey {
            dimensions,
            origin,
            scale,
            width: shape.width,
            color: shape.color,
            kind: shape.kind,
        };
        let reset = self.key.as_ref() != Some(&key)
            || !shape.points.starts_with(&self.points)
            // A dot uses a differently oriented polygon from a capsule's cap.
            || (self.points.len() == 1 && shape.points.len() > 1);
        if reset {
            self.key = Some(key);
            self.points.clear();
            self.rows = vec![Vec::new(); h as usize * 8];
            self.pixels = base.to_vec();
        }
        if !reset && self.points.len() == shape.points.len() {
            return &self.pixels;
        }
        let start = self.points.len().saturating_sub(1);
        let added = polygons(&shape.points[start..], shape.width);
        let mut dirty = vec![(w as usize, 0_usize); h as usize];
        for polygon in added {
            let polygon: Vec<_> = polygon.into_iter().map(|p| (p - origin) * scale).collect();
            let top = polygon
                .iter()
                .map(|p| f32::from(p.y))
                .fold(f32::INFINITY, f32::min)
                .floor()
                .clamp(0., h as f32) as usize;
            let bottom = polygon
                .iter()
                .map(|p| f32::from(p.y))
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .clamp(0., h as f32) as usize;
            for (row, changed) in dirty.iter_mut().enumerate().take(bottom).skip(top) {
                for sample in 0..8 {
                    let y = row as f32 + (sample as f32 + 0.5) / 8.;
                    let mut lo = f32::INFINITY;
                    let mut hi = f32::NEG_INFINITY;
                    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                        let ay = f32::from(a.y);
                        let by = f32::from(b.y);
                        if (ay <= y && y < by) || (by <= y && y < ay) {
                            let x = f32::from(a.x) + (y - ay) / (by - ay) * f32::from(b.x - a.x);
                            lo = lo.min(x);
                            hi = hi.max(x);
                        }
                    }
                    lo = lo.max(0.);
                    hi = hi.min(w as f32);
                    if lo >= hi {
                        continue;
                    }
                    let intervals = &mut self.rows[row * 8 + sample];
                    let first = intervals.partition_point(|&(_, end)| end < lo);
                    if intervals
                        .get(first)
                        .is_some_and(|&(start, end)| start <= lo && hi <= end)
                    {
                        continue;
                    }
                    changed.0 = changed.0.min(lo.floor() as usize);
                    changed.1 = changed.1.max(hi.ceil() as usize);
                    let mut last = first;
                    while last < intervals.len() && intervals[last].0 <= hi {
                        lo = lo.min(intervals[last].0);
                        hi = hi.max(intervals[last].1);
                        last += 1;
                    }
                    intervals.splice(first..last, [(lo, hi)]);
                }
            }
        }
        let color = shape.color.to_be_bytes();
        let mut coverage = Vec::new();
        for (row, (left, right)) in dirty.into_iter().enumerate() {
            if right <= left {
                continue;
            }
            coverage.clear();
            coverage.resize(right - left, 0_f32);
            for sample in 0..8 {
                let intervals = &self.rows[row * 8 + sample];
                let first = intervals.partition_point(|&(_, end)| end <= left as f32);
                for &(start, end) in intervals[first..]
                    .iter()
                    .take_while(|&&(start, _)| start < right as f32)
                {
                    let first = start.floor().max(left as f32) as usize;
                    let last = end.ceil().min(right as f32) as usize;
                    for x in first..last {
                        coverage[x - left] +=
                            (end.min(x as f32 + 1.) - start.max(x as f32)).max(0.) / 8.;
                    }
                }
            }
            for (i, coverage) in coverage.iter().enumerate() {
                let offset = (row * w as usize + left + i) * 4;
                let coverage = coverage.min(1.);
                if base[offset + 3] != 0 {
                    let alpha = coverage * color[3] as f32 / 255.;
                    for channel in 0..3 {
                        self.pixels[offset + channel] = (base[offset + channel] as f32
                            * (1. - alpha)
                            + color[channel] as f32 * alpha)
                            .round() as u8;
                    }
                }
            }
        }
        self.points.clone_from(&shape.points);
        &self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_strokes_match_full_coverage_at_crossings_and_after_edits() {
        use crate::annotation::{Shape, ShapeKind};
        for scale in [1., 1.25, 1.73, 2.] {
            for kind in [ShapeKind::Pencil, ShapeKind::Highlighter] {
                let origin = point(px(-10.), px(20.));
                let original: Vec<_> = (0..96 * 96)
                    .flat_map(|i| [(i % 251) as u8, 90, 170, if i % 31 == 0 { 0 } else { 255 }])
                    .collect();
                let base: Vec<_> = original
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [180, 120, 60, p[3]])
                    .collect();
                let mut shape = Shape {
                    kind,
                    number: None,
                    text: None,
                    bounds: Default::default(),
                    color: 0xe0305060,
                    width: 7.3,
                    points: vec![origin + point(px(12.), px(12.))],
                };
                let mut cache = StrokePreview::default();
                for i in 0..70 {
                    if i < 60 {
                        shape.points.push(
                            origin
                                + point(
                                    px(5. + (i * 7 % 70) as f32),
                                    px(5. + (i * 11 % 70) as f32),
                                ),
                        );
                    } else if i == 60 {
                        shape.points.truncate(5);
                    } else if i == 61 {
                        shape.width = 13.;
                    } else if i == 62 {
                        shape.color = 0x103090ff;
                    } else if i == 63 {
                        shape.points.reverse();
                    }
                    let actual = cache.render(&shape, &base, (96, 96), origin, scale);
                    let mut expected = base.clone();
                    rasterize(&shape, &mut expected, 96, 96, origin, scale);
                    assert_eq!(actual, expected, "{kind:?} scale {scale}, update {i}");
                }
            }
        }
    }

    #[test]
    fn arrow_tip_and_head_direction_survive_reverse_diagonal_and_short_drags() {
        let start = point(px(40.), px(40.));
        for (dx, dy) in [
            (1., 0.),
            (-1., 0.),
            (0., 1.),
            (0., -1.),
            (1., 1.),
            (-1., 1.),
            (1., -1.),
            (-1., -1.),
        ] {
            for length in [2., 5., 60.] {
                let tip = start + point(px(dx * length), px(dy * length));
                let geometry = geometry(&[start, tip], 5., true);
                assert_eq!(geometry.len(), 2);
                let head = &geometry[1];
                assert_eq!(head[0], tip);
                let delta = tip - start;
                let norm_squared = f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2);
                for vertex in head {
                    let relative = *vertex - start;
                    let along = (f32::from(relative.x) * f32::from(delta.x)
                        + f32::from(relative.y) * f32::from(delta.y))
                        / norm_squared;
                    assert!((0.39..=1.001).contains(&along));
                }
            }
        }
        assert!(geometry(&[start, start], 5., true).is_empty());
    }

    #[test]
    fn arrow_export_contains_a_head_without_overshooting_and_preserves_gaps() {
        let shape = super::super::Shape {
            kind: super::super::ShapeKind::Arrow,
            number: None,
            text: None,
            bounds: gpui_kit::Bounds::default(),
            points: vec![point(px(20.), px(30.)), point(px(70.), px(30.))],
            width: 3.,
            color: 0xff0000ff,
        };
        for scale in [1., 1.25, 1.73, 2.] {
            let w = (100. * scale) as u32;
            let mut pixels = [0, 0, 0, 255].repeat((w * w) as usize);
            let at =
                |x: f32, y: f32| (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
            let gap = at(40., 30.);
            pixels[gap..gap + 4].fill(0);
            rasterize(&shape, &mut pixels, w, w, point(px(0.), px(0.)), scale);
            assert_eq!(&pixels[at(45., 30.)..at(45., 30.) + 4], &[255, 0, 0, 255]);
            assert_eq!(&pixels[at(60., 33.)..at(60., 33.) + 4], &[255, 0, 0, 255]);
            assert_eq!(&pixels[at(71., 30.)..at(71., 30.) + 4], &[0, 0, 0, 255]);
            assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255)
            );
            assert_eq!(paths(&shape, point(px(0.), px(0.))).len(), 2);
        }
    }
}
