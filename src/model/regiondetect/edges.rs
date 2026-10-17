//! Structure extraction: connected components over the binary maps
//! (with a union-find that records bounding boxes) and the
//! directional edge field — per-axis gradients, non-maximum
//! suppression, a self-calibrating threshold and the long-run
//! "weak line" promotion that lets faint 1px panel borders survive.

use super::IntRect;
use super::image::{GrayImage, RgbaView, canny, color_difference, dilate_linf1, grayscale};
// ── Connected components ─────────────────────────────────────────────

/// A component's bounding box (half-open) and pixel count.
pub(super) struct ComponentBounds {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
    pub(super) pixels: u32,
}

struct UnionFind {
    parent: Vec<i32>,
}

impl UnionFind {
    fn new() -> Self {
        Self { parent: vec![0] }
    }
    fn make_label(&mut self) -> i32 {
        let label = self.parent.len() as i32;
        self.parent.push(label);
        label
    }
    fn find(&mut self, label: i32) -> i32 {
        let mut root = label;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        // Path compression.
        let mut current = label;
        while current != root {
            let next = self.parent[current as usize];
            self.parent[current as usize] = root;
            current = next;
        }
        root
    }
    fn unite(&mut self, a: i32, b: i32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b as usize] = a;
        }
    }
}

/// Two-pass union-find labeling of non-zero pixels. Index 0 is a dummy
/// so a label of 0 can mean "background".
pub(super) fn connected_components(image: &GrayImage, eight: bool) -> Vec<ComponentBounds> {
    if image.width <= 0 || image.height <= 0 {
        return Vec::new();
    }
    let width = image.width;
    let height = image.height;
    let mut labels = UnionFind::new();
    let mut map = vec![0i32; (width as usize) * (height as usize)];

    // First pass: provisional labels, neighboring labels merged.
    for y in 0..height {
        for x in 0..width {
            if image.at(x, y) == 0 {
                continue;
            }
            let mut current = 0;
            if x > 0 && map[y as usize * width as usize + (x - 1) as usize] != 0 {
                current = map[y as usize * width as usize + (x - 1) as usize];
            }
            if y > 0 {
                let prev = &map[(y - 1) as usize * width as usize..y as usize * width as usize];
                let consider = |labels: &mut UnionFind, label: i32, current: &mut i32| {
                    if label == 0 {
                        return;
                    }
                    if *current == 0 {
                        *current = label;
                    } else if *current != label {
                        labels.unite(*current, label);
                    }
                };
                consider(&mut labels, prev[x as usize], &mut current);
                if eight {
                    if x > 0 {
                        consider(&mut labels, prev[(x - 1) as usize], &mut current);
                    }
                    if x + 1 < width {
                        consider(&mut labels, prev[(x + 1) as usize], &mut current);
                    }
                }
            }
            if current == 0 {
                current = labels.make_label();
            }
            map[y as usize * width as usize + x as usize] = current;
        }
    }

    // Second pass: compress labels, accumulate bounding boxes. A
    // component's bounds initialize from its FIRST pixel (not zeros —
    // min() against 0 would pin every box to the origin).
    let mut remap = vec![0i32; labels.parent.len()];
    let mut next = 1;
    let mut bounds: Vec<ComponentBounds> = (0..labels.parent.len())
        .map(|_| ComponentBounds {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            pixels: 0,
        })
        .collect();
    for y in 0..height {
        for x in 0..width {
            let label = map[y as usize * width as usize + x as usize];
            if label == 0 {
                continue;
            }
            let root = labels.find(label);
            if remap[root as usize] == 0 {
                remap[root as usize] = next;
                next += 1;
                bounds[remap[root as usize] as usize] = ComponentBounds {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + 1,
                    pixels: 0,
                };
            }
            let item = &mut bounds[remap[root as usize] as usize];
            item.left = item.left.min(x);
            item.top = item.top.min(y);
            item.right = item.right.max(x + 1);
            item.bottom = item.bottom.max(y + 1);
            item.pixels += 1;
        }
    }
    bounds
}

/// Bounding rects of closed edge rings. After closing, an edge ring is a
/// thin loop whose bounding box already covers the hollow control — no
/// need for a second pass over the inverted image.
pub(super) fn contour_rects(edges: &GrayImage) -> Vec<IntRect> {
    connected_components(edges, true)
        .into_iter()
        .skip(1) // dummy label 0
        .filter(|c| c.pixels >= 16)
        .map(|c| IntRect::new(c.left, c.top, c.right, c.bottom))
        .collect()
}

// ── Edge field ───────────────────────────────────────────────────────

/// Directional color-difference edge maps plus the combined binary
/// field every scorer reads.
pub(super) struct EdgeField {
    pub(super) vertical: GrayImage,
    pub(super) horizontal: GrayImage,
    pub(super) combined: GrayImage,
    pub(super) threshold: u8,
}

/// Promote sufficiently long LOW-gradient runs to directional edges:
/// faint 1px panel borders (below the global threshold) still span the
/// whole panel, while short weak runs (text strokes) stay suppressed.
/// A run must also dominate its orthogonal gradient (4·g ≥ 3·other) so
/// corners stay directional.
fn weak_lines(gradient: &GrayImage, other: &GrayImage, vertical: bool) -> GrayImage {
    let mut mask = GrayImage::new(gradient.width, gradient.height);
    let across = if vertical {
        gradient.width
    } else {
        gradient.height
    };
    let along = if vertical {
        gradient.height
    } else {
        gradient.width
    };
    for fixed in 0..across {
        let mut start: i32 = -1;
        for position in 0..=along {
            let (x, y) = if vertical {
                (fixed, position)
            } else {
                (position, fixed)
            };
            let hit = position < along
                && gradient.at(x, y) >= 5
                && i32::from(gradient.at(x, y)) * 4 >= i32::from(other.at(x, y)) * 3;
            if hit {
                if start < 0 {
                    start = position;
                }
            } else {
                if start >= 0 && position - start >= 24 {
                    for along_pos in start..position {
                        let (px, py) = if vertical {
                            (fixed, along_pos)
                        } else {
                            (along_pos, fixed)
                        };
                        mask.set(px, py, 255);
                    }
                }
                start = -1;
            }
        }
    }
    mask
}

/// Build the edge field for the (analysis-resolution) image.
pub(super) fn build_edge_field(view: RgbaView<'_>) -> EdgeField {
    let width = view.width;
    let height = view.height;
    let mut vertical = GrayImage::new(width, height);
    let mut horizontal = GrayImage::new(width, height);
    let mut histogram = [0u32; 256];

    // Per-axis color-difference gradients: gx = max diff to the left /
    // right neighbors, gy likewise vertically. Color differences (not
    // grayscale) keep hue boundaries like a blue-on-gray panel visible.
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let center = view.px(x, y);
            let gx = color_difference(view.px(x - 1, y), center)
                .max(color_difference(center, view.px(x + 1, y)));
            let gy = color_difference(view.px(x, y - 1), center)
                .max(color_difference(center, view.px(x, y + 1)));
            vertical.set(x, y, gx);
            horizontal.set(x, y, gy);
            let mag = gx.max(gy);
            if mag > 2 {
                histogram[mag as usize] += 1;
            }
        }
    }

    // Threshold at the 35th percentile of the gradient histogram,
    // halved and clamped to [5, 18]: keep weak borders, but a few
    // bright text areas must not drag the global threshold up.
    let count: u32 = histogram.iter().sum();
    let mut accumulated = 0u32;
    let mut percentile = 12u8;
    for (value, bucket) in histogram.iter().enumerate() {
        accumulated += bucket;
        if u64::from(accumulated) > u64::from(count) * 35 / 100 {
            percentile = value as u8;
            break;
        }
    }
    let threshold = (percentile / 2).clamp(5, 18);

    let weak_vertical = weak_lines(&vertical, &horizontal, true);
    let weak_horizontal = weak_lines(&horizontal, &vertical, false);
    let high = (f32::from(threshold) * 3.).clamp(15., 54.);
    let canny_edges = canny(&grayscale(view), high * 0.4, high);

    // Non-maximum suppression on the directional color edges: a 2px
    // gradient step would otherwise fatten the bounding box of every
    // control by one pixel on each side.
    let mut binary_vertical = GrayImage::new(width, height);
    let mut binary_horizontal = GrayImage::new(width, height);
    let mut color_mask = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let gx = vertical.at(x, y);
            let gy = horizontal.at(x, y);
            let vertical_peak = gx >= vertical.at(x - 1, y) && gx > vertical.at(x + 1, y);
            let horizontal_peak = gy >= horizontal.at(x, y - 1) && gy > horizontal.at(x, y + 1);
            let vertical_hit = vertical_peak
                && ((gx >= threshold && i32::from(gx) * 4 >= i32::from(gy) * 3)
                    || weak_vertical.at(x, y) != 0);
            let horizontal_hit = horizontal_peak
                && ((gy >= threshold && i32::from(gy) * 4 >= i32::from(gx) * 3)
                    || weak_horizontal.at(x, y) != 0);
            if vertical_hit {
                binary_vertical.set(x, y, 255);
            }
            if horizontal_hit {
                binary_horizontal.set(x, y, 255);
            }
            if vertical_hit || horizontal_hit {
                color_mask.set(x, y, 255);
            }
        }
    }

    // Canny fills ONLY the gaps far from any color edge — adding its
    // edges next to strong color edges would paint a halo one step
    // outside every control.
    let dilated_color = dilate_linf1(&dilate_linf1(&color_mask));
    let mut combined = color_mask;
    for y in 0..height {
        for x in 0..width {
            if canny_edges.at(x, y) != 0
                && dilated_color.at(x, y) == 0
                && vertical.at(x, y).max(horizontal.at(x, y)) < threshold
            {
                combined.set(x, y, 255);
            }
        }
    }
    EdgeField {
        vertical: binary_vertical,
        horizontal: binary_horizontal,
        combined,
        threshold,
    }
}
