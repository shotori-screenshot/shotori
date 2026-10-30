//! Shared rounded stroke geometry for line preview and PNG rasterization.
use gpui_kit::{FillOptions, Path, PathBuilder, PathStyle, Pixels, Point, point, px};

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
    styled_geometry(points, width, arrow, super::LineStyle::Solid)
}

fn styled_geometry(
    points: &[Point<Pixels>],
    width: f32,
    arrow: bool,
    style: super::LineStyle,
) -> Vec<Vec<Point<Pixels>>> {
    if !arrow {
        return patterned_geometry(points, width, style);
    }
    let Some((&tip, rest)) = points.split_last() else {
        return Vec::new();
    };
    let length: f32 = points
        .windows(2)
        .map(|p| super::gesture::distance(p[0], p[1]))
        .sum();
    if length < 0.001 || rest.is_empty() {
        return Vec::new();
    }
    let head = (width * 4.).max(10.).min(length * 0.6);
    let shaft = width.min(length * 0.25);
    let mut body = points.to_vec();
    let mut remaining = head;
    while body.len() >= 2 {
        let last = body.len() - 1;
        let segment = super::gesture::distance(body[last - 1], body[last]);
        if segment > remaining {
            body[last] = body[last] + (body[last - 1] - body[last]) * (remaining / segment);
            break;
        }
        remaining -= segment;
        body.pop();
    }
    let base = body.last().copied().unwrap_or(points[0]);
    let delta = tip - base;
    let distance = f32::from(delta.x).hypot(f32::from(delta.y));
    if distance < 0.001 {
        return patterned_geometry(points, shaft, style);
    }
    let unit = delta / distance;
    let normal = point(-unit.y, unit.x);
    let mut result = patterned_geometry(&body, shaft, style);
    result.push(vec![
        tip,
        base + normal * (head * 0.45),
        base - normal * (head * 0.45),
    ]);
    result
}

/// Build each silhouette from the sampled centerline so curves, export and picking agree.
pub(crate) fn arrow_geometry(
    points: &[Point<Pixels>],
    width: f32,
    arrow: super::ArrowStyle,
    style: super::LineStyle,
) -> Vec<Vec<Point<Pixels>>> {
    use super::ArrowStyle;
    let style = if arrow.supports_line_style() {
        style
    } else {
        super::LineStyle::Solid
    };
    if arrow == ArrowStyle::Triangle {
        return styled_geometry(points, width, true, style);
    }
    let length: f32 = points
        .windows(2)
        .map(|p| super::gesture::distance(p[0], p[1]))
        .sum();
    if points.len() < 2 || length < 0.001 {
        return Vec::new();
    }
    let head = (width * 4.).max(10.).min(length * 0.3);
    let mut body = points.to_vec();
    let tip = points[points.len() - 1];
    let base = trim_tip(&mut body, head);
    let delta = tip - base;
    let distance = super::gesture::distance(tip, base);
    if distance < 0.001 {
        return patterned_geometry(points, width, style);
    }
    let normal = point(-delta.y, delta.x) / distance;
    let wings = [base + normal * (head * 0.45), base - normal * (head * 0.45)];
    if matches!(arrow, ArrowStyle::Outline | ArrowStyle::Filled) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut travelled = 0.;
        for (ix, &p) in body.iter().enumerate() {
            if ix > 0 {
                travelled += super::gesture::distance(body[ix - 1], p);
            }
            let tangent = body[(ix + 1).min(body.len() - 1)] - body[ix.saturating_sub(1)];
            let len = f32::from(tangent.x).hypot(f32::from(tangent.y));
            let side = if len > 0.001 {
                point(-tangent.y, tangent.x) / len
            } else {
                normal
            };
            let radius = width * 0.5 * (travelled / (length - head).max(0.001));
            left.push(p + side * radius);
            right.push(p - side * radius);
        }
        left.extend([wings[0], tip, wings[1]]);
        left.extend(right.into_iter().rev());
        if arrow == ArrowStyle::Filled {
            return vec![left];
        }
        left.push(left[0]);
        return patterned_geometry(&left, width.min(head * 0.15), style);
    }
    if arrow == ArrowStyle::DoubleTriangle {
        body.reverse();
        // The start head points opposite the centerline's direction.
        let start_tip = points[0];
        let start_base = trim_tip(&mut body, head);
        let d = start_tip - start_base;
        let len = super::gesture::distance(start_tip, start_base).max(0.001);
        let n = point(-d.y, d.x) / len;
        body.reverse();
        let mut result = patterned_geometry(&body, width, style);
        result.push(vec![
            start_tip,
            start_base + n * (head * 0.45),
            start_base - n * (head * 0.45),
        ]);
        result.push(vec![tip, wings[0], wings[1]]);
        return result;
    }
    let mut result = patterned_geometry(points, width, style);
    if arrow == ArrowStyle::Dimension {
        result.extend(polygons(
            &[tip + normal * (head * 0.45), tip - normal * (head * 0.45)],
            width,
        ));
    } else {
        result.extend(polygons(&[wings[0], tip, wings[1]], width));
    }
    if matches!(arrow, ArrowStyle::DoubleOpen | ArrowStyle::Dimension) {
        let mut reversed = points.to_vec();
        reversed.reverse();
        let tip = reversed[reversed.len() - 1];
        let base = trim_tip(&mut reversed, head);
        let d = tip - base;
        let len = super::gesture::distance(tip, base).max(0.001);
        let n = point(-d.y, d.x) / len;
        let base = if arrow == ArrowStyle::Dimension {
            tip
        } else {
            base
        };
        let mut end = vec![base + n * (head * 0.45)];
        if arrow == ArrowStyle::DoubleOpen {
            end.push(tip);
        }
        end.push(base - n * (head * 0.45));
        result.extend(polygons(&end, width));
    }
    result
}

fn trim_tip(points: &mut Vec<Point<Pixels>>, mut distance: f32) -> Point<Pixels> {
    while points.len() >= 2 {
        let last = points.len() - 1;
        let segment = super::gesture::distance(points[last - 1], points[last]);
        if segment > distance {
            points[last] = points[last] + (points[last - 1] - points[last]) * (distance / segment);
            break;
        }
        distance -= segment;
        points.pop();
    }
    points.last().copied().unwrap_or_default()
}

/// Line and arrow points keep endpoints first and an optional on-curve bend apex.
/// Sampling is shared by screen paths, export, picking and selection bounds.
pub(super) fn shape_geometry(shape: &super::Shape) -> Vec<Vec<Point<Pixels>>> {
    if shape.has_custom_geometry() {
        return super::geometry::polygons(shape);
    }
    if shape.kind == super::ShapeKind::EraserRect
        || (shape.kind == super::ShapeKind::Highlighter && shape.highlighter_options().rectangle)
    {
        let b = shape.bounds;
        return vec![vec![
            b.origin,
            point(b.right(), b.top()),
            b.bottom_right(),
            point(b.left(), b.bottom()),
        ]];
    }
    let points = curve_points(shape);
    if shape.kind == super::ShapeKind::Arrow {
        arrow_geometry(
            &points,
            shape.size(),
            shape.arrow_style(),
            shape.line_style(),
        )
    } else if matches!(
        shape.kind,
        super::ShapeKind::Line | super::ShapeKind::Polyline
    ) {
        let ends = shape.line_ends();
        let mut result = patterned_stroke(&points, shape.size(), shape.line_style(), ends);
        append_marker(&mut result, &points, shape.size(), ends.end);
        if ends.start != super::EndMarker::None {
            let reversed: Vec<_> = points.iter().rev().copied().collect();
            append_marker(&mut result, &reversed, shape.size(), ends.start);
        }
        result
    } else {
        patterned_geometry(&points, shape.size(), shape.line_style())
    }
}

pub(super) fn patterned_geometry(
    points: &[Point<Pixels>],
    width: f32,
    style: super::LineStyle,
) -> Vec<Vec<Point<Pixels>>> {
    patterned_stroke(points, width, style, super::LineEnds::default())
}

fn patterned_stroke(
    points: &[Point<Pixels>],
    width: f32,
    style: super::LineStyle,
    ends: super::LineEnds,
) -> Vec<Vec<Point<Pixels>>> {
    if style == super::LineStyle::Solid {
        return stroke_polygons(points, width, ends);
    }
    let unit = width.max(1.);
    let marks: &[(f32, f32)] = match style {
        super::LineStyle::Solid => unreachable!(),
        super::LineStyle::Dashed => &[(4., 3.)],
        super::LineStyle::Dotted => &[(0., 3.)],
        super::LineStyle::DashDot => &[(4., 3.), (0., 3.)],
        super::LineStyle::DashDotDot => &[(4., 3.), (0., 3.), (0., 3.)],
    };
    let length: f32 = points
        .windows(2)
        .map(|p| super::gesture::distance(p[0], p[1]))
        .sum();
    let mut result = Vec::new();
    let mut offset = 0.;
    let mut mark = 0;
    while offset < length {
        let (ink, gap) = marks[mark % marks.len()];
        let path = subpath(points, offset, (offset + ink * unit).min(length));
        result.extend(stroke_polygons(&path, width, ends));
        offset += (ink + gap) * unit;
        mark += 1;
    }
    result
}

fn subpath(points: &[Point<Pixels>], from: f32, to: f32) -> Vec<Point<Pixels>> {
    let mut result = Vec::new();
    let mut distance = 0.;
    for pair in points.windows(2) {
        let length = super::gesture::distance(pair[0], pair[1]);
        if length <= 0.001 {
            continue;
        }
        if to < distance {
            break;
        }
        if from <= distance + length {
            let direction = (pair[1] - pair[0]) / length;
            if result.is_empty() {
                result.push(pair[0] + direction * (from - distance).max(0.));
            }
            if to > from {
                result.push(pair[0] + direction * (to - distance).min(length));
            }
        }
        distance += length;
    }
    result
}

fn stroke_polygons(
    points: &[Point<Pixels>],
    width: f32,
    ends: super::LineEnds,
) -> Vec<Vec<Point<Pixels>>> {
    if !ends.square_caps && !ends.sharp_joins {
        return polygons(points, width);
    }
    if let [center] = points {
        if ends.square_caps {
            let r = px(width / 2.);
            return vec![vec![
                *center + point(-r, -r),
                *center + point(r, -r),
                *center + point(r, r),
                *center + point(-r, r),
            ]];
        }
        return polygons(points, width);
    }
    let Some(first) = points.first() else {
        return Vec::new();
    };
    use lyon_tessellation::{
        BuffersBuilder, LineCap, LineJoin, StrokeOptions, StrokeTessellator, StrokeVertex,
        VertexBuffers,
    };
    let mut path = lyon_tessellation::path::Path::builder();
    path.begin(lyon_tessellation::math::point(
        f32::from(first.x),
        f32::from(first.y),
    ));
    for p in &points[1..] {
        path.line_to(lyon_tessellation::math::point(
            f32::from(p.x),
            f32::from(p.y),
        ));
    }
    path.end(false);
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_tolerance(0.1)
        .with_line_cap(if ends.square_caps {
            LineCap::Square
        } else {
            LineCap::Round
        })
        .with_line_join(if ends.sharp_joins {
            LineJoin::Miter
        } else {
            LineJoin::Round
        });
    let mut mesh: VertexBuffers<Point<Pixels>, u32> = VertexBuffers::new();
    let result = StrokeTessellator::new().tessellate_path(
        &path.build(),
        &options,
        &mut BuffersBuilder::new(&mut mesh, |v: StrokeVertex| {
            point(px(v.position().x), px(v.position().y))
        }),
    );
    if result.is_err() {
        return Vec::new();
    }
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.iter().map(|&i| mesh.vertices[i as usize]).collect())
        .collect()
}

fn append_marker(
    result: &mut Vec<Vec<Point<Pixels>>>,
    points: &[Point<Pixels>],
    width: f32,
    marker: super::EndMarker,
) {
    use super::EndMarker;
    if marker == EndMarker::None || points.len() < 2 {
        return;
    }
    let tip = points[points.len() - 1];
    let length: f32 = points
        .windows(2)
        .map(|p| super::gesture::distance(p[0], p[1]))
        .sum();
    if length < 0.001 {
        return;
    }
    let head = (width * 4.).max(10.).min(length * 0.45);
    let mut body = points.to_vec();
    let base = trim_tip(&mut body, head);
    let d = tip - base;
    let normal = point(-d.y, d.x) / super::gesture::distance(base, tip).max(0.001);
    let half = normal * (head * 0.45);
    match marker {
        EndMarker::None => {}
        EndMarker::Open => result.extend(polygons(&[base + half, tip, base - half], width)),
        EndMarker::Triangle => result.push(vec![base + half, tip, base - half]),
        EndMarker::Bar => result.extend(polygons(&[tip + half, tip - half], width)),
    }
}

fn curve_points(shape: &super::Shape) -> std::borrow::Cow<'_, [Point<Pixels>]> {
    if !matches!(shape.kind, super::ShapeKind::Line | super::ShapeKind::Arrow) {
        return std::borrow::Cow::Borrowed(&shape.points);
    }
    let [start, end, midpoint] = shape.points.as_slice() else {
        return std::borrow::Cow::Borrowed(&shape.points);
    };
    // Solve the quadratic Bezier equation B(1/2) = midpoint for its control point.
    // https://pomax.github.io/bezierinfo/#quadratic
    let control = *midpoint * 2. - (*start + *end) / 2.;
    let length =
        super::gesture::distance(*start, control) + super::gesture::distance(control, *end);
    // Include t=1/2 exactly so the visible curve passes through its bend handle.
    let steps = ((length / 4.).ceil().clamp(8., 512.) as usize).next_multiple_of(2);
    std::borrow::Cow::Owned(
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                *start * ((1. - t) * (1. - t)) + control * (2. * t * (1. - t)) + *end * (t * t)
            })
            .collect(),
    )
}

pub(super) fn paths(shape: &super::Shape, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
    shape_geometry(shape)
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

/// The live freehand draft painted straight onto the overlay while the
/// stroke is in flight (the rasterized release form lives in the
/// selection image). All capsule subpaths wind the same direction, so a
/// NonZero fill unions them into one shape: retraces within the stroke
/// coat once, matching the export's coverage union. The lyon default
/// fill rule is EvenOdd, which would punch a hole at every capsule
/// overlap. Chunks stay under the u16 tessellation index ceiling.
pub(super) fn draft_paths(shape: &super::Shape, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
    const CHUNK: usize = 1024; // 34 vertices per capsule × 1024 ≪ u16::MAX
    polygons(&shape.points, shape.size())
        .chunks(CHUNK)
        .filter_map(|chunk| {
            let mut builder =
                PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
            for polygon in chunk {
                builder.move_to(polygon[0] + offset);
                for p in &polygon[1..] {
                    builder.line_to(*p + offset);
                }
                builder.close();
            }
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
    coverage(shape, w, h, origin, scale, |offset, coverage| {
        super::highlighter::blend_pixel(shape, &mut rgba[offset..offset + 4], coverage);
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
    let polygons = shape_geometry(shape)
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
    highlighter: super::highlighter::HighlighterOptions,
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
            width: shape.size(),
            color: shape.color,
            kind: shape.kind,
            highlighter: shape.highlighter_options(),
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
        let added = polygons(&shape.points[start..], shape.size());
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
                self.pixels[offset..offset + 4].copy_from_slice(&base[offset..offset + 4]);
                super::highlighter::blend_pixel(
                    shape,
                    &mut self.pixels[offset..offset + 4],
                    coverage,
                );
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
    fn arrow_silhouettes_are_distinct_and_filled_arrows_ignore_dash_patterns() {
        use crate::annotation::{ArrowStyle, LineStyle, Shape, ShapeKind, ShapeParameters};
        let styles = [
            ArrowStyle::Open,
            ArrowStyle::DoubleOpen,
            ArrowStyle::Outline,
            ArrowStyle::Filled,
            ArrowStyle::Dimension,
            ArrowStyle::Triangle,
            ArrowStyle::DoubleTriangle,
        ];
        for points in [
            vec![point(px(20.), px(40.)), point(px(160.), px(40.))],
            vec![
                point(px(20.), px(80.)),
                point(px(160.), px(80.)),
                point(px(130.), px(35.)),
            ],
        ] {
            let mut images = Vec::new();
            for arrow in styles {
                let mut mark = Shape {
                    kind: ShapeKind::Arrow,
                    number: None,
                    text: None,
                    text_style: Default::default(),
                    bounds: Default::default(),
                    points: points.clone(),
                    color: 0xff0000ff,
                    parameters: ShapeParameters::Stroke {
                        ends: Default::default(),
                        width: 3.,
                        style: LineStyle::Solid,
                        arrow,
                    },
                };
                let mut pixels = [0, 0, 0, 255].repeat(200 * 200);
                rasterize(&mark, &mut pixels, 200, 200, point(px(0.), px(0.)), 1.);
                assert!(
                    pixels.as_chunks::<4>().0.iter().any(|p| p[0] != 0),
                    "{arrow:?}"
                );
                assert!(!images.contains(&pixels), "duplicate silhouette: {arrow:?}");
                if arrow == ArrowStyle::Filled {
                    for style in [
                        LineStyle::Dashed,
                        LineStyle::Dotted,
                        LineStyle::DashDot,
                        LineStyle::DashDotDot,
                    ] {
                        mark.parameters = ShapeParameters::Stroke {
                            ends: Default::default(),
                            width: 3.,
                            style,
                            arrow,
                        };
                        let mut patterned = [0, 0, 0, 255].repeat(200 * 200);
                        rasterize(&mark, &mut patterned, 200, 200, point(px(0.), px(0.)), 1.);
                        assert_eq!(pixels, patterned);
                    }
                }
                let bounds = mark.selection_box();
                assert!(
                    shape_geometry(&mark)
                        .iter()
                        .flatten()
                        .all(|p| bounds.contains(p))
                );
                images.push(pixels);
            }
        }
        for arrow in styles {
            for end in [
                point(px(20.), px(20.)),
                point(px(20.01), px(20.)),
                point(px(18.), px(19.)),
            ] {
                let geometry = arrow_geometry(
                    &[point(px(20.), px(20.)), end],
                    20.,
                    arrow,
                    LineStyle::DashDotDot,
                );
                assert!(
                    geometry
                        .iter()
                        .flatten()
                        .all(|p| f32::from(p.x).is_finite() && f32::from(p.y).is_finite())
                );
            }
        }
    }

    #[test]
    fn free_bend_handle_stays_on_curve_at_every_orientation() {
        use crate::annotation::{Shape, ShapeKind, ShapeParameters};
        for kind in [ShapeKind::Line, ShapeKind::Arrow] {
            for angle in [
                0_f32,
                0.7,
                std::f32::consts::FRAC_PI_2,
                2.8,
                std::f32::consts::PI,
                -1.2,
            ] {
                let tangent = point(px(angle.cos()), px(angle.sin()));
                let normal = point(-tangent.y, tangent.x);
                let center = point(px(300.), px(300.));
                let mut mark = Shape {
                    kind,
                    number: None,
                    text: None,
                    text_style: Default::default(),
                    bounds: Default::default(),
                    color: 0xff0000ff,
                    parameters: ShapeParameters::new(kind, 3.),
                    points: vec![center - tangent * 100., center + tangent * 100.],
                };
                for height in [-90., -1., 0., 1., 90.] {
                    for sideways in [-200., -99., 0., 99., 200.] {
                        mark.set_handle(2, center + normal * height + tangent * sideways);
                        let apex = mark.handle_points()[2];
                        let expected = center + normal * height + tangent * sideways;
                        assert!(super::super::gesture::distance(apex, expected) < 0.001);
                        let points = curve_points(&mark);
                        assert!(
                            super::super::gesture::distance(points[points.len() / 2], apex) < 0.001
                        );
                        for p in points.iter() {
                            let delta = *p - center;
                            let bend = f32::from(delta.x) * f32::from(normal.x)
                                + f32::from(delta.y) * f32::from(normal.y);
                            assert!(
                                bend >= height.min(0.) - 0.001 && bend <= height.max(0.) + 0.001
                            );
                        }
                    }
                }
                mark.set_handle(0, mark.points[1]);
                assert!(
                    curve_points(&mark)
                        .iter()
                        .all(|p| f32::from(p.x).is_finite() && f32::from(p.y).is_finite())
                );
            }
        }
    }

    #[test]
    fn patterned_lines_keep_phase_across_vertices_and_export_visible_gaps() {
        use crate::annotation::{LineStyle, Shape, ShapeKind, ShapeParameters};
        for style in [
            LineStyle::Dashed,
            LineStyle::Dotted,
            LineStyle::DashDot,
            LineStyle::DashDotDot,
        ] {
            let mut shape = Shape {
                kind: ShapeKind::Polyline,
                number: None,
                text: None,
                text_style: Default::default(),
                bounds: Default::default(),
                points: vec![point(px(10.), px(20.)), point(px(90.), px(20.))],
                parameters: ShapeParameters::Stroke {
                    ends: Default::default(),
                    width: 2.,
                    style,
                    arrow: super::super::ArrowStyle::Triangle,
                },
                color: 0xff0000ff,
            };
            for scale in [1., 1.25, 2.] {
                let width = (100. * scale) as u32;
                let mut continuous = [0, 0, 0, 255].repeat((width * width) as usize);
                rasterize(
                    &shape,
                    &mut continuous,
                    width,
                    width,
                    point(px(0.), px(0.)),
                    scale,
                );
                shape.points.insert(1, point(px(37.), px(20.)));
                let mut split = [0, 0, 0, 255].repeat((width * width) as usize);
                rasterize(
                    &shape,
                    &mut split,
                    width,
                    width,
                    point(px(0.), px(0.)),
                    scale,
                );
                assert_eq!(continuous, split, "{style:?}, scale {scale}");
                shape.points.remove(1);
                let row = (20. * scale) as usize;
                let reds: Vec<_> = (12..85)
                    .map(|x| continuous[(row * width as usize + (x as f32 * scale) as usize) * 4])
                    .collect();
                assert!(reds.contains(&0));
                assert!(reds.iter().any(|v| *v > 0), "{style:?}, scale {scale}");
            }
            shape.kind = ShapeKind::Arrow;
            let geometry = shape_geometry(&shape);
            assert_eq!(geometry.last().unwrap()[0], shape.points[1]);
            assert!(geometry.iter().flatten().all(|p| p.x <= shape.points[1].x));
        }
    }

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
                    text_style: Default::default(),
                    bounds: Default::default(),
                    color: 0xe0305060,
                    parameters: crate::annotation::ShapeParameters::new(kind, 7.3),
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
                        shape.parameters.set_size(13.);
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
            text_style: Default::default(),
            bounds: gpui_kit::Bounds::default(),
            points: vec![point(px(20.), px(30.)), point(px(70.), px(30.))],
            parameters: crate::annotation::ShapeParameters::Stroke {
                ends: Default::default(),
                width: 3.,
                style: super::super::LineStyle::Solid,
                arrow: super::super::ArrowStyle::Triangle,
            },
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
