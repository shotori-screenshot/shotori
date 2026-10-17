//! # Smart region detection: pixel-based selection candidates
//!
//! Hover-highlight + click-to-select for UI regions (panels, controls,
//! text blocks) straight from the frozen pixels — no compositor IPC
//! involved. Ported from mark-shot's `region_detection` module; the
//! algorithm is identical, the comments preserve its hard-won rationale.
//!
//! Why this exists next to [`crate::platform::windowsnap`]: the IPC
//! backends cover floating windows only (niri's tiled geometry is not
//! exposed at all), and naive pixel detection was prototyped and
//! rejected once before — content edges inside windows scored as
//! strongly as real boundaries, so snapping silently grabbed wrong
//! regions. This pipeline fixes exactly that with:
//!
//! - a **directional color-difference edge field** (not just Canny):
//!   per-axis gradients with non-maximum suppression, a self-calibrating
//!   threshold (35th percentile of the gradient histogram, clamped), and
//!   long-run "weak line" promotion so faint 1px panel borders survive
//!   while short text strokes do not;
//! - **four proposal sources** (closed contours, connected components,
//!   edge-bounded background, paired long lines) so both small controls
//!   and large panels get nominated;
//! - **border-support scoring** — a candidate must have edges along all
//!   four sides — plus texture penalties that reject text blocks and
//!   icon grids;
//! - **sub-pixel-free refinement** on the ORIGINAL resolution: each side
//!   searches ±radius px for the strongest color boundary, undoing the
//!   downscale blur.
//!
//! Analysis runs at most 1280 px on the long edge, once per capture, on
//! a background thread (see `main.rs`); the result is a candidate list
//! cheap to query per pointer move. Compositor window rects are injected
//! afterwards as high-quality candidates (see
//! [`RegionDetector::add_window_rects`]) so hover prefers UI controls
//! and the wheel hierarchy can step out to whole windows.
//!
//! Debug builds run the pixel loops without optimization (~10× slower);
//! the feature simply lights up a moment later. Release analysis of a
//! 2560×1440 frame is on the order of a hundred milliseconds.
//!
//! Offline tuning: `SHOTORI_REGIONDUMP=<dir>` (read in main.rs) makes the
//! analysis save its input pixels, edge map and every scored proposal —
//! accepted or not, with the gate that rejected it — via
//! [`DetectTrace::save_artifacts`]. Grade ground truth and score the
//! dumps with `scripts/regiondetect_workbench.py` (or
//! `scripts/regiondetect_eval.py` directly).

// ── Geometry ─────────────────────────────────────────────────────────

/// Half-open pixel rect `[left, right) × [top, bottom)` — the pipeline's
/// native space (analysis and full-resolution pixels alike). Unlike a
/// width/height rect it cannot express a negative size, and edge tests
/// are unambiguous when mapping between resolutions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl IntRect {
    pub(crate) fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
    fn width(self) -> i32 {
        self.right - self.left
    }
    fn height(self) -> i32 {
        self.bottom - self.top
    }
    fn area(self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }
    /// Point containment, half-open on right/bottom (±1 px against Qt's
    /// inclusive QRect — immaterial for hit-tests).
    fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    /// `[left, top, right, bottom]` — the dump/JSON representation.
    fn array(self) -> [i32; 4] {
        [self.left, self.top, self.right, self.bottom]
    }
    /// Full containment (borders may touch).
    fn contains_rect(self, other: IntRect) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// Where a candidate came from. Detection sources merge (a rect proposed
/// by both contours and lines is more trustworthy); `WINDOW` marks
/// compositor-rect injection and changes hover priority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sources(u8);

impl Sources {
    pub(crate) const CONTOUR: Self = Self(1 << 0);
    const COMPONENT: Self = Self(1 << 1);
    const BACKGROUND: Self = Self(1 << 2);
    const LINES: Self = Self(1 << 3);
    pub(crate) const WINDOW: Self = Self(1 << 4);

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// A selection candidate: the rect plus why we believe in it. Quality
/// distinguishes solid frames from hypothesis-grade pairings; sources
/// mark provenance (window injection changes hover priority).
#[derive(Clone, Debug)]
pub(crate) struct RegionCandidate {
    pub(crate) rect: IntRect,
    quality: f32,
    sources: Sources,
}

impl RegionCandidate {
    /// Hand-built candidate for integration tests in other modules.
    #[cfg(test)]
    pub(crate) fn synthetic(rect: IntRect, quality: f32, sources: Sources) -> Self {
        Self {
            rect,
            quality,
            sources,
        }
    }
}

// ── Buffers ──────────────────────────────────────────────────────────

/// Single-channel buffer, row-major.
struct GrayImage {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl GrayImage {
    fn new(width: i32, height: i32) -> Self {
        Self {
            width: width.max(0),
            height: height.max(0),
            pixels: vec![0; (width.max(0) as usize) * (height.max(0) as usize)],
        }
    }
    /// Checked read; outside is 0 — the edge field treats out-of-bounds
    /// neighbors as flat, same as mark-shot's guarded lambdas.
    fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[y as usize * self.width as usize + x as usize]
    }
    /// Checked write; outside is dropped (callers stay in-bounds by
    /// construction, this is the panic guard for the background thread).
    fn set(&mut self, x: i32, y: i32, value: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let index = y as usize * self.width as usize + x as usize;
        if let Some(slot) = self.pixels.get_mut(index) {
            *slot = value;
        }
    }
}

/// Read-only RGBA8 view. Shotori captures are already straight RGBA,
/// rotated and Y-flipped (see `platform::capture`), so no
/// premultiplication dance is needed.
#[derive(Clone, Copy)]
struct RgbaView<'a> {
    width: i32,
    height: i32,
    data: &'a [u8],
}

impl<'a> RgbaView<'a> {
    fn new(data: &'a [u8], width: i32, height: i32) -> Option<Self> {
        if width < 0 || height < 0 || data.len() < width as usize * height as usize * 4 {
            return None;
        }
        Some(Self {
            width,
            height,
            data,
        })
    }
    /// Checked pixel read; outside is opaque black.
    fn px(&self, x: i32, y: i32) -> [u8; 3] {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return [0, 0, 0];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }
}

/// Max channel difference between two RGB triples — the "color
/// distance" every threshold below is stated in.
fn color_difference(a: [u8; 3], b: [u8; 3]) -> u8 {
    let dr = (i32::from(a[0]) - i32::from(b[0])).abs();
    let dg = (i32::from(a[1]) - i32::from(b[1])).abs();
    let db = (i32::from(a[2]) - i32::from(b[2])).abs();
    dr.max(dg).max(db).min(255) as u8
}

/// Owned RGBA buffer at analysis resolution.
struct RgbaBuf {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

impl RgbaBuf {
    fn view(&self) -> RgbaView<'_> {
        RgbaView {
            width: self.width,
            height: self.height,
            data: &self.data,
        }
    }
}

/// BT.601 integer weights — the same fixed-point ratios as the `image`
/// crate's grayscale, so thresholds behave like the reference
/// implementation's.
fn grayscale(view: RgbaView<'_>) -> GrayImage {
    let mut gray = GrayImage::new(view.width, view.height);
    for y in 0..gray.height {
        for x in 0..gray.width {
            let [r, g, b] = view.px(x, y);
            let value = (19595 * i32::from(r) + 38470 * i32::from(g) + 7471 * i32::from(b)) >> 16;
            gray.set(x, y, value.clamp(0, 255) as u8);
        }
    }
    gray
}

/// Area-average downscale (box filter). Qt's SmoothTransformation is
/// bilinear; area averaging is strictly better for large factors (no
/// aliasing of thin borders) and the downstream thresholds
/// self-calibrate on the gradient histogram either way.
fn downscale(source: &RgbaView<'_>, target_w: i32, target_h: i32) -> RgbaBuf {
    let mut out = RgbaBuf {
        width: target_w,
        height: target_h,
        data: vec![0; target_w as usize * target_h as usize * 4],
    };
    let sx = source.width as f64 / target_w as f64;
    let sy = source.height as f64 / target_h as f64;
    for ty in 0..target_h {
        let y0 = ty as f64 * sy;
        let y1 = (ty + 1) as f64 * sy;
        for tx in 0..target_w {
            let x0 = tx as f64 * sx;
            let x1 = (tx + 1) as f64 * sx;
            let (mut acc, mut weight) = ([0f64; 4], 0f64);
            let ys = y0.floor() as i32;
            let ye = y1.ceil() as i32;
            let xs = x0.floor() as i32;
            let xe = x1.ceil() as i32;
            for y in ys..ye.max(ys) {
                let wy = overlap(y as f64, y0, y1);
                for x in xs..xe.max(xs) {
                    let w = overlap(x as f64, x0, x1) * wy;
                    if w <= 0. {
                        continue;
                    }
                    let i = (y as usize * source.width as usize + x as usize) * 4;
                    for (c, slot) in acc.iter_mut().enumerate() {
                        *slot += w * f64::from(source.data[i + c]);
                    }
                    weight += w;
                }
            }
            let o = (ty as usize * target_w as usize + tx as usize) * 4;
            for (c, value) in acc.iter().enumerate() {
                out.data[o + c] = (if weight > 0. { value / weight } else { 0. })
                    .round()
                    .clamp(0., 255.) as u8;
            }
        }
    }
    out
}

/// 1D overlap of [p, p+1) with [lo, hi).
fn overlap(p: f64, lo: f64, hi: f64) -> f64 {
    (p + 1.).min(hi) - p.max(lo)
}

// ── Morphology & Canny ───────────────────────────────────────────────

/// L∞ radius-1 dilation of a (binary-valued) gray image.
fn dilate_linf1(image: &GrayImage) -> GrayImage {
    let mut out = GrayImage::new(image.width, image.height);
    for y in 0..image.height {
        for x in 0..image.width {
            let mut value = 0u8;
            'window: for ny in (y - 1).max(0)..=(y + 1).min(image.height - 1) {
                for nx in (x - 1).max(0)..=(x + 1).min(image.width - 1) {
                    value = value.max(image.at(nx, ny));
                    if value == 255 {
                        break 'window; // early exit: binary image
                    }
                }
            }
            out.set(x, y, value);
        }
    }
    out
}

/// L∞ radius-1 erosion of a (binary-valued) gray image.
fn erode_linf1(image: &GrayImage) -> GrayImage {
    let mut out = GrayImage::new(image.width, image.height);
    for y in 0..image.height {
        for x in 0..image.width {
            let mut value = 255u8;
            'window: for ny in (y - 1).max(0)..=(y + 1).min(image.height - 1) {
                for nx in (x - 1).max(0)..=(x + 1).min(image.width - 1) {
                    value = value.min(image.at(nx, ny));
                    if value == 0 {
                        break 'window;
                    }
                }
            }
            out.set(x, y, value);
        }
    }
    out
}

/// Closing (dilate then erode): bridges 1px gaps in edge rings so their
/// connected component covers the whole control instead of two arcs.
fn close_linf1(image: &GrayImage) -> GrayImage {
    erode_linf1(&dilate_linf1(image))
}

/// Separable Gaussian blur, radius 3, σ 1.4, quantized to 8 bits.
fn gaussian_blur(image: &GrayImage) -> GrayImage {
    const RADIUS: i32 = 3;
    const SIGMA: f32 = 1.4;
    let mut kernel = [0f32; 7];
    let mut sum = 0f32;
    for (i, slot) in kernel.iter_mut().enumerate() {
        let d = i as f32 - RADIUS as f32;
        let value = (-(d * d) / (2. * SIGMA * SIGMA)).exp();
        *slot = value;
        sum += value;
    }
    for slot in &mut kernel {
        *slot /= sum;
    }

    let width = image.width;
    let height = image.height;
    let mut mid = vec![0f32; (width as usize) * (height as usize)];
    let mut out = GrayImage::new(width, height);
    // Horizontal pass; borders clamp (not crop) so the kernel stays
    // normalized at the edges.
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0f32;
            for (k, weight) in kernel.iter().enumerate() {
                let sx = (x + k as i32 - RADIUS).clamp(0, width - 1);
                acc += f32::from(image.at(sx, y)) * weight;
            }
            mid[y as usize * width as usize + x as usize] = acc;
        }
    }
    // Vertical pass + quantization.
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0f32;
            for (k, weight) in kernel.iter().enumerate() {
                let sy = (y + k as i32 - RADIUS).clamp(0, height - 1);
                acc += mid[sy as usize * width as usize + x as usize] * weight;
            }
            out.set(x, y, acc.round().clamp(0., 255.) as u8);
        }
    }
    out
}

/// Quantize a Sobel direction into 4 buckets of 45°. tan(22.5°) ≈ 0.414
/// avoids a per-pixel atan2.
fn quantized_direction(gx: f32, gy: f32) -> u8 {
    const TAN_22_5: f32 = 0.414_213_56;
    let ax = gx.abs();
    let ay = gy.abs();
    if ay <= ax * TAN_22_5 {
        0 // horizontal gradient → vertical edge
    } else if ax <= ay * TAN_22_5 {
        2 // vertical gradient → horizontal edge
    } else if gx * gy >= 0. {
        1
    } else {
        3
    }
}

/// Canny edge detection. Magnitudes are compared as SQUARES (no hypot);
/// hysteresis grows strong edges through weak ones with a DFS stack.
fn canny(image: &GrayImage, low_threshold: f32, high_threshold: f32) -> GrayImage {
    let mut edges = GrayImage::new(image.width, image.height);
    if image.width < 3 || image.height < 3 {
        return edges;
    }
    let blurred = gaussian_blur(image);
    let width = image.width;
    let height = image.height;
    let mut magnitude = vec![0f32; (width as usize) * (height as usize)];
    let mut direction = vec![0u8; (width as usize) * (height as usize)];
    let low_sq = low_threshold * low_threshold;
    let high_sq = high_threshold * high_threshold;

    // 1. Sobel gradients + direction quantization.
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let tl = f32::from(blurred.at(x - 1, y - 1));
            let t = f32::from(blurred.at(x, y - 1));
            let tr = f32::from(blurred.at(x + 1, y - 1));
            let l = f32::from(blurred.at(x - 1, y));
            let r = f32::from(blurred.at(x + 1, y));
            let bl = f32::from(blurred.at(x - 1, y + 1));
            let b = f32::from(blurred.at(x, y + 1));
            let br = f32::from(blurred.at(x + 1, y + 1));
            let gx = -tl + tr - 2. * l + 2. * r - bl + br;
            let gy = -tl - 2. * t - tr + bl + 2. * b + br;
            let index = y as usize * width as usize + x as usize;
            magnitude[index] = gx * gx + gy * gy;
            direction[index] = quantized_direction(gx, gy);
        }
    }

    // 2. Non-maximum suppression + double threshold classification.
    // The asymmetric `<=`/`<` picks one winner per 2px step so adjacent
    // equal plateaus do not both survive.
    let mut classified = vec![0u8; (width as usize) * (height as usize)];
    let mut stack: Vec<usize> = Vec::with_capacity((width + height) as usize);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y as usize * width as usize + x as usize;
            let value = magnitude[index];
            let (na, nb) = match direction[index] {
                0 => (magnitude[index - 1], magnitude[index + 1]),
                1 => (
                    magnitude[(y - 1) as usize * width as usize + (x + 1) as usize],
                    magnitude[(y + 1) as usize * width as usize + (x - 1) as usize],
                ),
                2 => (
                    magnitude[(y - 1) as usize * width as usize + x as usize],
                    magnitude[(y + 1) as usize * width as usize + x as usize],
                ),
                _ => (
                    magnitude[(y - 1) as usize * width as usize + (x - 1) as usize],
                    magnitude[(y + 1) as usize * width as usize + (x + 1) as usize],
                ),
            };
            if value <= na || value < nb {
                continue;
            }
            if value >= high_sq {
                classified[index] = 255;
                stack.push(index);
            } else if value >= low_sq {
                classified[index] = 128;
            }
        }
    }

    // 3. Hysteresis: strong seeds grow through weak neighbors.
    while let Some(index) = stack.pop() {
        edges.pixels[index] = 255;
        let x = (index % width as usize) as i32;
        let y = (index / width as usize) as i32;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                if nx <= 0 || ny <= 0 || nx >= width - 1 || ny >= height - 1 {
                    continue;
                }
                let neighbor = ny as usize * width as usize + nx as usize;
                if classified[neighbor] == 128 {
                    classified[neighbor] = 255;
                    stack.push(neighbor);
                }
            }
        }
    }
    edges
}

// ── Connected components ─────────────────────────────────────────────

/// A component's bounding box (half-open) and pixel count.
struct ComponentBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    pixels: u32,
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
fn connected_components(image: &GrayImage, eight: bool) -> Vec<ComponentBounds> {
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
fn contour_rects(edges: &GrayImage) -> Vec<IntRect> {
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
struct EdgeField {
    vertical: GrayImage,
    horizontal: GrayImage,
    combined: GrayImage,
    threshold: u8,
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
fn build_edge_field(view: RgbaView<'_>) -> EdgeField {
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

// ── Proposals ────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Proposal {
    rect: IntRect,
    source: Sources,
}

/// A run of edge pixels along one scanline.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Segment {
    fixed: i32,
    start: i32,
    end: i32,
}

/// Extract sufficiently long directional segments (gaps ≤ 3 px allowed,
/// minimum length 24 px), deduplicated and capped at 192 per direction.
fn long_segments(edges: &GrayImage, vertical: bool) -> Vec<Segment> {
    let across = if vertical { edges.width } else { edges.height };
    let along = if vertical { edges.height } else { edges.width };
    let mut segments: Vec<Segment> = Vec::new();
    for fixed in 0..across {
        let mut start: i32 = -1;
        let mut last = 0;
        for position in 0..=along {
            let hit = position < along
                && (if vertical {
                    edges.at(fixed, position)
                } else {
                    edges.at(position, fixed)
                }) != 0;
            if hit {
                if start < 0 {
                    start = position;
                }
                last = position;
            } else if start >= 0 && (position == along || position - last > 3) {
                if last - start >= 24 {
                    segments.push(Segment {
                        fixed,
                        start,
                        end: last + 1,
                    });
                }
                start = -1;
            }
        }
    }

    // Longest first; near-duplicates (parallel double edges of one
    // border) collapse to a single segment.
    segments.sort_by_key(|s| std::cmp::Reverse(s.end - s.start));
    let mut unique: Vec<Segment> = Vec::new();
    for line in segments {
        let duplicate = unique.iter().any(|other| {
            (line.fixed - other.fixed).abs() <= 2
                && (line.start - other.start).abs() <= 5
                && (line.end - other.end).abs() <= 5
        });
        if !duplicate {
            unique.push(line);
            if unique.len() >= 192 {
                break;
            }
        }
    }
    unique
}

/// Proposals from closed contour bounding boxes.
fn contour_proposals(edges: &GrayImage) -> Vec<Proposal> {
    contour_rects(edges)
        .into_iter()
        .map(|rect| Proposal {
            rect,
            source: Sources::CONTOUR,
        })
        .collect()
}

/// Proposals from connected components — foreground blobs (controls
/// with solid fills) and, in background mode, the panels delimited by
/// edges: the edge-bounded background regions reveal panel/splitter
/// layouts that no single contour covers.
fn component_proposals(image: &GrayImage, background: bool) -> Vec<Proposal> {
    connected_components(image, !background)
        .into_iter()
        .skip(1)
        .filter(|item| item.pixels >= 24)
        .filter_map(|item| {
            let rect = IntRect::new(item.left, item.top, item.right, item.bottom);
            // A background region only counts when it is nearly filled:
            // an L-shaped leftover around overlapping windows is not a
            // selectable panel.
            if background && f64::from(item.pixels) / (rect.area().max(1) as f64) < 0.72 {
                return None;
            }
            Some(Proposal {
                rect,
                source: if background {
                    Sources::BACKGROUND
                } else {
                    Sources::COMPONENT
                },
            })
        })
        .collect()
}

/// Pair two long parallel segments of similar extent into a rect.
/// Windows and panels whose border is only partially detected (rounded
/// corners, shadows) still produce two solid long sides.
fn line_proposals(field: &EdgeField) -> Vec<Proposal> {
    let mut proposals = Vec::new();
    for vertical in [true, false] {
        let edges = if vertical {
            &field.vertical
        } else {
            &field.horizontal
        };
        let mut segments = long_segments(edges, vertical);
        segments.sort_by_key(|s| s.fixed);
        for index in 0..segments.len() {
            let first = segments[index];
            let aligned: Vec<Segment> = segments[index + 1..]
                .iter()
                .copied()
                .filter(|second| {
                    let tolerance = pair_tolerance(first, *second);
                    (first.fixed - second.fixed).abs() >= 8
                        && (first.start - second.start).abs() <= tolerance
                        && (first.end - second.end).abs() <= tolerance
                })
                .collect();
            if aligned.is_empty() {
                continue;
            }
            // A few pairs only: the nearest partners plus the farthest —
            // exhaustive pairing was measured as noise, not coverage.
            let mut pairs: Vec<Segment> = aligned.iter().copied().take(2).collect();
            let last = *aligned.last().expect("non-empty");
            if !pairs.contains(&last) {
                pairs.push(last);
            }
            for second in pairs {
                let tolerance = pair_tolerance(first, second);
                if (first.fixed - second.fixed).abs() < 8
                    || (first.start - second.start).abs() > tolerance
                    || (first.end - second.end).abs() > tolerance
                {
                    continue;
                }
                let lo = first.fixed.min(second.fixed);
                let hi = first.fixed.max(second.fixed) + 1;
                let start = first.start.min(second.start);
                let end = first.end.max(second.end);
                proposals.push(Proposal {
                    rect: if vertical {
                        IntRect::new(lo, start, hi, end)
                    } else {
                        IntRect::new(start, lo, end, hi)
                    },
                    source: Sources::LINES,
                });
            }
        }
    }
    proposals
}

/// Alignment tolerance for a pair, proportional to the shorter segment
/// (12%), clamped to [4, 16] px.
fn pair_tolerance(a: Segment, b: Segment) -> i32 {
    ((a.end - a.start).min(b.end - b.start) / 12).clamp(4, 16)
}

// ── Scoring ──────────────────────────────────────────────────────────

/// Continuous support of ONE rectangle side: the fraction of sampled
/// positions (10%-trimmed ends, stride to cap at ~160 samples) where a
/// directional edge sits within ±2 px of the side. Sides at the image
/// border are trusted at 0.85 — a panel touching the screenshot edge
/// must not lose points for the edge it cannot show.
fn side_support(edges: &GrayImage, fixed: i32, start: i32, end: i32, vertical: bool) -> f32 {
    let limit = if vertical { edges.width } else { edges.height };
    if fixed <= 1 || fixed + 2 >= limit {
        return 0.85;
    }
    let margin = ((end - start) / 10).min(12);
    let (start, end) = (start + margin, end - margin);
    if end <= start {
        return 0.;
    }
    let step = ((end - start) / 160).max(1);
    let mut best = 0f32;
    let across_max = (fixed + 2).min(limit - 1);
    for across in (fixed - 2).max(0)..=across_max {
        let (mut supported, mut samples) = (0i32, 0i32);
        let mut along = start;
        while along < end {
            let hit = if vertical {
                edges.at(across, along)
            } else {
                edges.at(along, across)
            } != 0;
            supported += i32::from(hit);
            samples += 1;
            along += step;
        }
        best = best.max(supported as f32 / samples.max(1) as f32);
    }
    best
}

/// Support of all four sides: left, top, right, bottom.
fn border_sides(field: &EdgeField, rect: IntRect) -> [f32; 4] {
    [
        side_support(&field.vertical, rect.left, rect.top, rect.bottom, true),
        side_support(&field.horizontal, rect.top, rect.left, rect.right, false),
        side_support(&field.vertical, rect.right - 1, rect.top, rect.bottom, true),
        side_support(
            &field.horizontal,
            rect.bottom - 1,
            rect.left,
            rect.right,
            false,
        ),
    ]
}

/// Edge density inside the rect (3 px inset, ≤32×32 sample grid) — the
/// texture signal: text blocks and icon grids light up.
fn interior_density(edges: &GrayImage, rect: IntRect) -> f32 {
    const INSET: i32 = 3;
    if rect.width() <= INSET * 2 || rect.height() <= INSET * 2 {
        return 0.;
    }
    let (mut count, mut samples) = (0i32, 0i32);
    let y_step = (rect.height() / 32).max(1);
    let x_step = (rect.width() / 32).max(1);
    let mut y = rect.top + INSET;
    while y < rect.bottom - INSET {
        let mut x = rect.left + INSET;
        while x < rect.right - INSET {
            count += i32::from(edges.at(x, y) != 0);
            samples += 1;
            x += x_step;
        }
        y += y_step;
    }
    count as f32 / samples.max(1) as f32
}

/// Border support of the rect inset by 4 px: a solid border (panel)
/// keeps support; a cluster of inner edges (text) collapses.
fn inset_support(field: &EdgeField, rect: IntRect) -> f32 {
    if rect.width() < 12 || rect.height() < 12 {
        return 0.;
    }
    let inner = IntRect::new(rect.left + 4, rect.top + 4, rect.right - 4, rect.bottom - 4);
    let sides = border_sides(field, inner);
    (sides[0] + sides[1] + sides[2] + sides[3]) / 4.
}

/// Analysis-px rect → original-resolution px. Rounding is `(v·to +
/// from/2)/from` so edges land on the pixel they came from.
fn scale_rect(rect: IntRect, analysis_w: i32, analysis_h: i32, width: i32, height: i32) -> IntRect {
    let convert = |value: i32, from: i32, to: i32| {
        ((i64::from(value) * i64::from(to) + i64::from(from) / 2) / i64::from(from)) as i32
    };
    IntRect::new(
        convert(rect.left, analysis_w, width),
        convert(rect.top, analysis_h, height),
        convert(rect.right, analysis_w, width),
        convert(rect.bottom, analysis_h, height),
    )
}

/// Refine ONE side on the original image: search ±radius px around the
/// downscaled estimate for the position with the best coverage of
/// color-difference boundaries (count ≥ min(threshold, 8)), with a tiny
/// strength term and a distance penalty so ties stay near the estimate.
fn refine_side(
    view: &RgbaView<'_>,
    original: i32,
    start: i32,
    end: i32,
    vertical: bool,
    radius: i32,
    threshold: u8,
) -> i32 {
    let limit = if vertical { view.width } else { view.height };
    if original <= 1 || original >= limit - 1 {
        return original;
    }
    let margin = ((end - start) / 10).clamp(1, 24);
    let along_limit = if vertical { view.height } else { view.width };
    let start = (start + margin).max(0);
    let end = (end - margin).min(along_limit);
    if end <= start {
        return original;
    }
    let step = ((end - start) / 128).max(1);
    let mut best = original;
    let mut best_score = 0f32;
    for position in (original - radius).max(1)..=(original + radius).min(limit - 1) {
        let (mut count, mut strength, mut samples) = (0i32, 0f32, 0i32);
        let mut along = start;
        while along < end {
            let a = if vertical {
                view.px(position - 1, along)
            } else {
                view.px(along, position - 1)
            };
            let b = if vertical {
                view.px(position, along)
            } else {
                view.px(along, position)
            };
            let diff = color_difference(a, b);
            count += i32::from(diff >= threshold.min(8));
            strength += f32::from(diff.min(48)) / 48.;
            samples += 1;
            along += step;
        }
        let coverage = count as f32 / samples.max(1) as f32;
        let score = coverage * 0.85 + strength / samples.max(1) as f32 * 0.15
            - (position - original).abs() as f32 * 0.008;
        if coverage >= 0.55 && score > best_score {
            best_score = score;
            best = position;
        }
    }
    best
}

/// Refine all four sides. Left/right refine first (over the full
/// height), then top/bottom over the REFINED width — a vertical side
/// needs the widest possible evidence span.
fn refine_rect(view: &RgbaView<'_>, rect: IntRect, radius: i32, threshold: u8) -> IntRect {
    let left = refine_side(
        view,
        rect.left,
        rect.top,
        rect.bottom,
        true,
        radius,
        threshold,
    );
    let right = refine_side(
        view,
        rect.right,
        rect.top,
        rect.bottom,
        true,
        radius,
        threshold,
    );
    let top = refine_side(view, rect.top, left, right, false, radius, threshold);
    let bottom = refine_side(view, rect.bottom, left, right, false, radius, threshold);
    if right - left < 8 || bottom - top < 8 {
        return rect; // refinement collapsed: keep the estimate
    }
    IntRect::new(
        left.max(0),
        top.max(0),
        right.min(view.width),
        bottom.min(view.height),
    )
}

/// Two rects are the same candidate when all four edges sit within
/// `tolerance` AND areas are within 20% of each other — equal edges
/// with wildly different areas are a nested rect, not a duplicate.
fn nearly_same_rect(a: IntRect, b: IntRect, tolerance: i32) -> bool {
    let (area_a, area_b) = (a.area(), b.area());
    (a.left - b.left).abs() <= tolerance
        && (a.top - b.top).abs() <= tolerance
        && (a.right - b.right).abs() <= tolerance
        && (a.bottom - b.bottom).abs() <= tolerance
        && f64::from(area_a.min(area_b) as u32) / f64::from(area_a.max(area_b).max(1) as u32) > 0.8
}

/// Merge `candidate` into the list: on a near-duplicate keep the
/// higher-quality RECT but union the sources (multi-source = more
/// trustworthy).
fn merge_candidate(
    candidates: &mut Vec<RegionCandidate>,
    candidate: RegionCandidate,
    tolerance: i32,
) {
    for existing in candidates.iter_mut() {
        if !nearly_same_rect(existing.rect, candidate.rect, tolerance) {
            continue;
        }
        let sources = existing.sources.union(candidate.sources);
        if candidate.quality > existing.quality {
            *existing = candidate;
        }
        existing.sources = sources;
        return;
    }
    candidates.push(candidate);
}

// ── Pipeline ─────────────────────────────────────────────────────────

/// Analysis resolution: the long edge capped at 1280 px (never below
/// 3 px, or Canny's 3×3 kernels would degenerate).
fn analysis_size(width: i32, height: i32) -> (i32, i32) {
    const MAX_DIMENSION: i32 = 1280;
    let longest = width.max(height);
    if longest <= MAX_DIMENSION {
        return (width, height);
    }
    let scale = f64::from(MAX_DIMENSION) / f64::from(longest);
    let w = ((f64::from(width) * scale).round() as i32).max(3);
    let h = ((f64::from(height) * scale).round() as i32).max(3);
    (w, h)
}

/// Budgets: proposals can explode to tens of thousands on busy screens;
/// candidates are what survives scoring.
const MAX_PROPOSALS: usize = 4096;
const MAX_REFINED: usize = 512;
const MAX_CANDIDATES: usize = 512;

/// One-shot region detection over a capture. Returns scored candidates
/// in original-image pixels (refined), sorted by area ascending.
/// Test-only convenience; production enters through
/// [`RegionDetector::analyze_with_trace`].
#[cfg(test)]
fn detect_regions(view: RgbaView<'_>) -> Vec<RegionCandidate> {
    detect_regions_traced(view, None)
}

/// [`detect_regions`] with an optional diagnostics trace: every scored
/// proposal is recorded with its measurements and the gate that rejected
/// it (see [`DetectTrace`]). Pure observation — the result is identical
/// either way.
fn detect_regions_traced(
    view: RgbaView<'_>,
    mut trace: Option<&mut DetectTrace>,
) -> Vec<RegionCandidate> {
    let (width, height) = (view.width, view.height);
    if width < 3 || height < 3 {
        return Vec::new();
    }

    let (analysis_w, analysis_h) = analysis_size(width, height);
    if let Some(t) = &mut trace {
        t.image = [width, height];
        t.analysis = [analysis_w, analysis_h];
    }
    // Conditional borrow: at native resolution the pipeline reads the
    // caller's buffer directly; only a real downscale allocates.
    let downscaled;
    let analysis = if analysis_w != width || analysis_h != height {
        downscaled = downscale(&view, analysis_w, analysis_h);
        downscaled.view()
    } else {
        view
    };

    let field = build_edge_field(analysis);
    if let Some(t) = &mut trace {
        t.threshold = field.threshold;
        t.edge_field = field.combined.pixels.clone();
    }
    let closed = close_linf1(&field.combined);
    let mut proposals = contour_proposals(&closed);
    proposals.extend(component_proposals(&closed, false));
    proposals.extend(line_proposals(&field));

    // Background regions: everything NOT behind a dilated edge — panels
    // and split layouts bounded by edges nominate their interiors.
    let barriers = dilate_linf1(&field.combined);
    let mut background = GrayImage::new(analysis_w, analysis_h);
    for y in 0..analysis_h {
        for x in 0..analysis_w {
            background.set(x, y, if barriers.at(x, y) == 0 { 255 } else { 0 });
        }
    }
    proposals.extend(component_proposals(&background, true));

    let scale = width as f32 / analysis_w as f32;
    let min_width = ((12.0f32 / scale).ceil() as i32).max(5);
    let min_height = ((12.0f32 / scale).ceil() as i32).max(5);
    let full_area = i64::from(analysis_w) * i64::from(analysis_h);
    let mut sized: Vec<Proposal> = Vec::with_capacity(proposals.len());
    for proposal in proposals {
        let fits = proposal.rect.width() >= min_width
            && proposal.rect.height() >= min_height
            && proposal.rect.area() < full_area * 995 / 1000;
        if fits {
            sized.push(proposal);
        } else if let Some(t) = &mut trace {
            t.record(&proposal, None, Some("size"));
        }
    }
    proposals = sized;

    // Keep both ends of the size spectrum: dropping only the middle
    // preserves small controls AND large panels within the budget.
    proposals.sort_by_key(|p| p.rect.area());
    if proposals.len() > MAX_PROPOSALS {
        let keep = MAX_PROPOSALS / 2;
        let split = proposals.len() - keep;
        if let Some(t) = &mut trace {
            for proposal in &proposals[keep..split] {
                t.record(proposal, None, Some("budget"));
            }
        }
        let mut trimmed = Vec::with_capacity(MAX_PROPOSALS);
        trimmed.extend_from_slice(&proposals[..keep]);
        trimmed.extend_from_slice(&proposals[split..]);
        proposals = trimmed;
    }

    let mut candidates: Vec<RegionCandidate> = Vec::new();
    let merge_tolerance = (scale.ceil() as i32) + 1;
    for proposal in proposals {
        let rect = proposal.rect;
        let sides = border_sides(&field, rect);
        let mut sorted = sides;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let support = (sides[0] + sides[1] + sides[2] + sides[3]) / 4.;
        // A real region has SOME edge on every side (sorted[0]) and a
        // decent majority on at least three (sorted[1]).
        let mut rejected = if sorted[1] < 0.48 {
            Some("second_side")
        } else if sorted[0] < 0.18 {
            Some("weakest_side")
        } else if support < 0.58 {
            Some("support")
        } else if proposal.source.has(Sources::LINES) && (sorted[0] < 0.70 || support < 0.85) {
            // Line-pairing proposals are hypothesis-grade: demand more.
            Some("lines_gate")
        } else {
            None
        };
        let (mut density, mut inset, mut compact) = (0f32, 0f32, false);
        if rejected.is_none() {
            density = interior_density(&field.combined, rect);
            inset = inset_support(&field, rect);
            compact = rect.width().min(rect.height()) as f32 * scale < 32.0;
            // Compact + busy inside = text block or icon grid, not a panel.
            if compact && (density > 0.30 || (density > 0.12 && inset > 0.60)) {
                rejected = Some("texture");
            }
        }
        let texture_penalty = if compact {
            density * 0.30 + inset * 0.18
        } else {
            density * 0.08
        };
        let quality = (support * 0.65 + sorted[0] * 0.25 + 0.10 - texture_penalty).clamp(0., 1.);
        if let Some(t) = &mut trace {
            t.record(
                &proposal,
                Some(&ProposalScore {
                    sides,
                    weakest: sorted[0],
                    second: sorted[1],
                    support,
                    density,
                    inset,
                    compact,
                    quality,
                }),
                rejected,
            );
        }
        if rejected.is_some() {
            continue;
        }
        merge_candidate(
            &mut candidates,
            RegionCandidate {
                rect: scale_rect(rect, analysis_w, analysis_h, width, height),
                quality,
                sources: proposal.source,
            },
            merge_tolerance,
        );
    }

    // Refinement touches original pixels: afford it for the best only.
    candidates.sort_by(|a, b| {
        b.quality
            .partial_cmp(&a.quality)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(MAX_REFINED);
    let radius = ((scale.ceil() as i32) * 3).clamp(3, 16);
    for candidate in candidates.iter_mut() {
        candidate.rect = refine_rect(&view, candidate.rect, radius, field.threshold);
    }
    let mut refined: Vec<RegionCandidate> = Vec::new();
    for candidate in candidates {
        merge_candidate(&mut refined, candidate, 2);
    }
    if let Some(t) = &mut trace {
        t.candidates = refined
            .iter()
            .map(|c| TracedCandidate {
                rect: c.rect.array(),
                quality: c.quality,
                sources: sources_label(c.sources),
            })
            .collect();
    }
    refined
}

// ── Diagnostics trace ────────────────────────────────────────────────

/// One proposal's scoring measurements, recorded wholesale in the dump.
struct ProposalScore {
    sides: [f32; 4],
    weakest: f32,
    second: f32,
    support: f32,
    density: f32,
    inset: f32,
    compact: bool,
    quality: f32,
}

/// A scored proposal as it looked at the scoring gate. `rejected` names
/// the gate that killed it — `size`/`budget` mean it never reached
/// scoring, so its measurements are zeros.
#[derive(serde::Serialize)]
struct TracedProposal {
    rect_analysis: [i32; 4],
    rect_image: [i32; 4],
    source: &'static str,
    sides: [f32; 4],
    weakest_side: f32,
    second_side: f32,
    support: f32,
    density: f32,
    inset_support: f32,
    compact: bool,
    quality: f32,
    rejected: Option<&'static str>,
}

/// A final candidate (refined, merged), pre window-injection.
#[derive(serde::Serialize)]
struct TracedCandidate {
    rect: [i32; 4],
    quality: f32,
    sources: String,
}

/// The full scoring ledger of one `detect_regions` run: every proposal
/// with the gate that rejected it, plus the surviving candidates. Only
/// populated when the caller asks (`RegionDetector::analyze_with_trace`,
/// wired to `SHOTORI_REGIONDUMP` in main.rs); the pipeline result is
/// bit-identical either way. Consumed offline by
/// `scripts/regiondetect_eval.py` for IoU scoring and miss diagnosis —
/// "the region I expected is missing" becomes "gate X killed the closest
/// proposal with weakest side 0.31", which is where to turn the screw.
#[derive(serde::Serialize)]
pub struct DetectTrace {
    image: [i32; 2],
    analysis: [i32; 2],
    threshold: u8,
    /// The combined binary edge field, row-major at `analysis`
    /// resolution (rendered to `-edges.png`, skipped in the JSON).
    #[serde(skip_serializing)]
    edge_field: Vec<u8>,
    proposals: Vec<TracedProposal>,
    candidates: Vec<TracedCandidate>,
}

impl DetectTrace {
    fn new(width: u32, height: u32) -> Self {
        Self {
            image: [width as i32, height as i32],
            analysis: [0, 0],
            threshold: 0,
            edge_field: Vec::new(),
            proposals: Vec::new(),
            candidates: Vec::new(),
        }
    }

    fn record(
        &mut self,
        proposal: &Proposal,
        score: Option<&ProposalScore>,
        rejected: Option<&'static str>,
    ) {
        let zero = ProposalScore {
            sides: [0.; 4],
            weakest: 0.,
            second: 0.,
            support: 0.,
            density: 0.,
            inset: 0.,
            compact: false,
            quality: 0.,
        };
        let s = score.unwrap_or(&zero);
        self.proposals.push(TracedProposal {
            rect_analysis: proposal.rect.array(),
            rect_image: scale_rect(
                proposal.rect,
                self.analysis[0],
                self.analysis[1],
                self.image[0],
                self.image[1],
            )
            .array(),
            source: source_label(proposal.source),
            sides: s.sides,
            weakest_side: s.weakest,
            second_side: s.second,
            support: s.support,
            density: s.density,
            inset_support: s.inset,
            compact: s.compact,
            quality: s.quality,
            rejected,
        });
    }

    /// Write `<stem>-input.png` (the analyzed capture), `<stem>-edges.png`
    /// (combined edge field at analysis resolution) and `<stem>-trace.json`
    /// into `dir`. The JSON goes LAST on purpose: its existence marks a
    /// complete dump (a process dying mid-save leaves orphan PNGs, not a
    /// half-written ledger for the eval script to trust). Runs on the
    /// analysis thread — debug-path I/O by contract, the dump only
    /// exists because the developer asked for it. `stem` must be
    /// filename-safe (Wayland output names are).
    pub fn save_artifacts(
        &self,
        dir: &std::path::Path,
        stem: &str,
        rgba: &[u8],
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        if self.image[0] > 0 && rgba.len() >= self.image[0] as usize * self.image[1] as usize * 4 {
            crate::model::export::save_png(
                &dir.join(format!("{stem}-input.png")),
                self.image[0] as u32,
                self.image[1] as u32,
                rgba,
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        }
        if let [w, h] = self.analysis
            && w > 0
            && self.edge_field.len() == (w * h) as usize
        {
            write_gray_png(
                &dir.join(format!("{stem}-edges.png")),
                w,
                h,
                &self.edge_field,
            )?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(dir.join(format!("{stem}-trace.json")), json)?;
        Ok(())
    }
}

/// Single-channel PNG for the diagnostics edge map.
fn write_gray_png(path: &std::path::Path, w: i32, h: i32, gray: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    writer
        .write_image_data(gray)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    writer
        .finish()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}

/// Stable label for a single-source proposal (the dump's JSON).
fn source_label(source: Sources) -> &'static str {
    if source.has(Sources::WINDOW) {
        "window"
    } else if source.has(Sources::CONTOUR) {
        "contour"
    } else if source.has(Sources::COMPONENT) {
        "component"
    } else if source.has(Sources::BACKGROUND) {
        "background"
    } else if source.has(Sources::LINES) {
        "lines"
    } else {
        "unknown"
    }
}

/// '+'-joined labels for merged sources (candidates can merge several).
fn sources_label(sources: Sources) -> String {
    let mut names: Vec<&'static str> = Vec::new();
    for (bit, name) in [
        (Sources::CONTOUR, "contour"),
        (Sources::COMPONENT, "component"),
        (Sources::BACKGROUND, "background"),
        (Sources::LINES, "lines"),
        (Sources::WINDOW, "window"),
    ] {
        if sources.has(bit) {
            names.push(name);
        }
    }
    names.join("+")
}

// ── Detector & cursor ────────────────────────────────────────────────

/// Frozen per-capture candidate index. Detection runs ONCE; hover and
/// hierarchy switching only query this. `pub` with private fields: the
/// binary passes it through opaquely from the background analysis into
/// the session.
pub struct RegionDetector {
    image_width: i32,
    image_height: i32,
    candidates: Vec<RegionCandidate>,
}

impl RegionDetector {
    /// Analyze a capture (RGBA8, `width`×`height`).
    pub fn analyze(rgba: &[u8], width: u32, height: u32) -> Self {
        Self::analyze_with_trace(rgba, width, height, false).0
    }

    /// [`Self::analyze`] with the diagnostics trace (see [`DetectTrace`]).
    pub fn analyze_with_trace(
        rgba: &[u8],
        width: u32,
        height: u32,
        trace: bool,
    ) -> (Self, Option<DetectTrace>) {
        let (w, h) = (width as i32, height as i32);
        let mut detect_trace = trace.then(|| DetectTrace::new(width, height));
        let mut candidates = match RgbaView::new(rgba, w, h) {
            Some(view) => detect_regions_traced(view, detect_trace.as_mut()),
            None => Vec::new(),
        };
        // Best-first for the cap, then stable area order for hierarchy
        // queries (children before parents).
        candidates.sort_by(|a, b| {
            b.quality
                .partial_cmp(&a.quality)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.rect.area().cmp(&b.rect.area()))
        });
        candidates.truncate(MAX_CANDIDATES);
        candidates.sort_by_key(|c| c.rect.area());
        (
            Self {
                image_width: w,
                image_height: h,
                candidates,
            },
            detect_trace,
        )
    }

    /// Build from fixed candidates — the test injection path.
    #[cfg(test)]
    pub(crate) fn from_candidates(
        width: i32,
        height: i32,
        candidates: Vec<RegionCandidate>,
    ) -> Self {
        Self {
            image_width: width,
            image_height: height,
            candidates,
        }
    }

    fn bounds(&self) -> IntRect {
        IntRect::new(0, 0, self.image_width, self.image_height)
    }

    /// The full-image rect (hover treats it as "no hit").
    pub(crate) fn image_bounds(&self) -> IntRect {
        self.bounds()
    }

    /// How many candidates survived scoring (diagnostics logging).
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    #[cfg(test)]
    fn candidates(&self) -> &[RegionCandidate] {
        &self.candidates
    }

    /// Inject compositor window rects (already in this image's pixel
    /// space, clipped to its bounds). They join the hierarchy as
    /// high-quality parents; hover still prefers UI candidates.
    pub(crate) fn add_window_rects(&mut self, windows: &[IntRect]) {
        for window in windows {
            let clipped = IntRect::new(
                window.left.max(0),
                window.top.max(0),
                window.right.min(self.image_width),
                window.bottom.min(self.image_height),
            );
            if clipped.width() < 8 || clipped.height() < 8 {
                continue;
            }
            merge_candidate(
                &mut self.candidates,
                RegionCandidate {
                    rect: clipped,
                    quality: 0.92,
                    sources: Sources::WINDOW,
                },
                2,
            );
        }
        self.candidates.sort_by_key(|c| c.rect.area());
    }

    /// Best candidate covering a point (image px). UI candidates win
    /// over injected window rects — hovering a control inside a window
    /// should pick the control; the window remains reachable via the
    /// wheel. Falls back to the full image.
    fn region_at(&self, x: i32, y: i32) -> IntRect {
        let mut best_ui: Option<&RegionCandidate> = None;
        let mut best_window: Option<&RegionCandidate> = None;
        for candidate in &self.candidates {
            if !candidate.rect.contains_point(x, y) {
                continue;
            }
            let window = candidate.sources.has(Sources::WINDOW);
            let best = if window {
                &mut best_window
            } else {
                &mut best_ui
            };
            if best.is_none_or(|current| rank(candidate) < rank(current)) {
                *best = Some(candidate);
            }
        }
        best_ui
            .or(best_window)
            .map(|c| c.rect)
            .unwrap_or_else(|| self.bounds())
    }

    /// Smallest parent of `current` containing `point`, or the full
    /// image when there is none.
    fn parent_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        self.candidates
            .iter()
            .filter(|c| {
                c.rect.contains_point(x, y) && c.rect != current && c.rect.contains_rect(current)
            })
            .min_by_key(|c| c.rect.area())
            .map(|c| c.rect)
            .unwrap_or_else(|| self.bounds())
    }

    /// Largest child of `current` containing `point`, or `current`.
    fn child_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        self.candidates
            .iter()
            .filter(|c| {
                c.rect.contains_point(x, y) && c.rect != current && current.contains_rect(c.rect)
            })
            .max_by_key(|c| c.rect.area())
            .map(|c| c.rect)
            .unwrap_or(current)
    }
}

/// Ranking for `region_at`: reliable bounds first (quality ≥ 0.80),
/// then smaller rects at equal quality — the innermost solid frame
/// under the cursor.
fn rank(candidate: &RegionCandidate) -> f32 {
    let tier = if candidate.quality >= 0.80 {
        0.
    } else {
        10000.
    };
    let size = (candidate.rect.area().max(0) as f32).sqrt().sqrt();
    tier + size / candidate.quality.max(0.1).powi(2)
}

/// Hover state for the smart-selection phase: small pointer moves do
/// not jump rects, and a wheel-selected level survives clicks until the
/// pointer actually leaves.
pub(crate) struct SelectionCursor {
    current: Option<IntRect>,
    anchor: (f32, f32),
    manual_level: bool,
}

impl Default for SelectionCursor {
    fn default() -> Self {
        Self {
            current: None,
            anchor: (0., 0.),
            manual_level: false,
        }
    }
}

impl SelectionCursor {
    /// Update the hovered candidate for a pointer position (image px).
    /// Returns the current rect.
    pub(crate) fn hover(&mut self, detector: &RegionDetector, x: i32, y: i32) -> IntRect {
        let tolerance = if self.manual_level { 6. } else { 3. };
        if let Some(current) = self.current
            && current.contains_point(x, y)
        {
            let distance =
                ((x as f32 - self.anchor.0).powi(2) + (y as f32 - self.anchor.1).powi(2)).sqrt();
            if distance <= tolerance {
                return current;
            }
        }
        let rect = detector.region_at(x, y);
        self.current = Some(rect);
        self.anchor = (x as f32, y as f32);
        self.manual_level = false;
        rect
    }

    /// Wheel step: positive grows to the parent region, negative
    /// shrinks to the child. The chosen level sticks until the pointer
    /// leaves the rect (or moves beyond the manual tolerance).
    pub(crate) fn step(
        &mut self,
        detector: &RegionDetector,
        x: i32,
        y: i32,
        direction: i32,
    ) -> IntRect {
        let current = self.hover(detector, x, y);
        let rect = if direction > 0 {
            detector.parent_of(current, x, y)
        } else if direction < 0 {
            detector.child_of(current, x, y)
        } else {
            current
        };
        self.current = Some(rect);
        self.anchor = (x as f32, y as f32);
        self.manual_level = true;
        rect
    }

    /// Is the current rect a wheel-chosen level (not plain hover)?
    pub(crate) fn manual_level(&self) -> bool {
        self.manual_level
    }

    pub(crate) fn reset(&mut self) {
        self.current = None;
        self.manual_level = false;
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: keep test helpers explicit; also avoids
    // pulling gpui's test attribute shadowing (see selection.rs).
    use super::{
        ComponentBounds, EdgeField, GrayImage, IntRect, RegionCandidate, RegionDetector, RgbaBuf,
        RgbaView, SelectionCursor, Sources, analysis_size, build_edge_field, canny,
        connected_components, detect_regions, refine_rect, scale_rect,
    };

    /// A W×H RGBA image: dark background with a light panel.
    fn panel_image(w: i32, h: i32, panel: IntRect) -> RgbaBuf {
        let mut data = vec![32u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if panel.contains_point(x, y) {
                    data[i] = 220;
                    data[i + 1] = 220;
                    data[i + 2] = 225;
                } else {
                    data[i] = 32;
                    data[i + 1] = 32;
                    data[i + 2] = 36;
                }
                data[i + 3] = 255;
            }
        }
        RgbaBuf {
            width: w,
            height: h,
            data,
        }
    }

    fn candidate(rect: IntRect, quality: f32, sources: Sources) -> RegionCandidate {
        RegionCandidate {
            rect,
            quality,
            sources,
        }
    }

    #[test]
    fn analysis_size_caps_the_long_edge() {
        assert_eq!(analysis_size(800, 600), (800, 600));
        let (w, h) = analysis_size(2560, 1440);
        assert_eq!(w.max(h), 1280);
        assert_eq!((w, h), (1280, 720));
        // portrait
        let (w, h) = analysis_size(600, 3000);
        assert_eq!((w, h), (256, 1280));
    }

    #[test]
    fn scale_rect_rounds_half_up_per_edge() {
        let rect = IntRect::new(10, 20, 300, 200);
        let scaled = scale_rect(rect, 1280, 720, 2560, 1440);
        assert_eq!(scaled, IntRect::new(20, 40, 600, 400));
    }

    #[test]
    fn connected_components_bounds_and_count() {
        let mut image = GrayImage::new(100, 100);
        for y in 10..20 {
            for x in 10..30 {
                image.set(x, y, 255);
            }
        }
        for y in 50..60 {
            for x in 70..90 {
                image.set(x, y, 128);
            }
        }
        let components: Vec<ComponentBounds> = connected_components(&image, true)
            .into_iter()
            .filter(|c| c.pixels > 0)
            .collect();
        assert_eq!(components.len(), 2);
        assert!(components.iter().any(|c| c.left == 10
            && c.top == 10
            && c.right == 30
            && c.bottom == 20
            && c.pixels == 200));
        assert!(
            components
                .iter()
                .any(|c| c.left == 70 && c.top == 50 && c.right == 90 && c.bottom == 60)
        );
    }

    #[test]
    fn canny_finds_a_step_edge() {
        let mut image = GrayImage::new(80, 60);
        for y in 0..60 {
            for x in 40..80 {
                image.set(x, y, 200);
            }
        }
        let edges = canny(&image, 12., 30.);
        let hits: Vec<(i32, i32)> = (0..60)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .filter(|&(x, y)| edges.at(x, y) != 0)
            .collect();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|&(x, _)| (38..=42).contains(&x)));
    }

    #[test]
    fn refinement_snaps_to_the_true_edge() {
        let image = panel_image(200, 150, IntRect::new(40, 30, 160, 90));
        let estimate = IntRect::new(37, 27, 164, 93);
        let refined = refine_rect(&image.view(), estimate, 6, 8);
        assert_eq!(refined, IntRect::new(40, 30, 160, 90));
    }

    #[test]
    fn detect_regions_finds_a_solid_panel() {
        let panel = IntRect::new(40, 30, 160, 90);
        let image = panel_image(240, 160, panel);
        let candidates = detect_regions(image.view());
        assert!(!candidates.is_empty());
        assert!(
            candidates.iter().any(|c| (c.rect.left - 40).abs() <= 2
                && (c.rect.top - 30).abs() <= 2
                && (c.rect.right - 160).abs() <= 2
                && (c.rect.bottom - 90).abs() <= 2),
            "no candidate matches the panel: {candidates:?}"
        );
    }

    #[test]
    fn edge_field_threshold_stays_inside_its_clamps() {
        let image = panel_image(240, 160, IntRect::new(40, 30, 160, 90));
        let EdgeField { threshold, .. } = build_edge_field(image.view());
        assert!((5..=18).contains(&threshold), "threshold {threshold}");
    }

    #[test]
    fn region_at_prefers_ui_over_window_candidates() {
        let window = candidate(IntRect::new(0, 0, 380, 280), 0.92, Sources::WINDOW);
        let control = candidate(IntRect::new(100, 100, 200, 180), 0.85, Sources::CONTOUR);
        let detector =
            RegionDetector::from_candidates(400, 300, vec![window.clone(), control.clone()]);
        assert_eq!(detector.region_at(150, 140), control.rect);
        // outside the control: the window candidate takes over
        assert_eq!(detector.region_at(50, 50), window.rect);
        // and a total miss falls back to the full image
        assert_eq!(detector.region_at(390, 290), IntRect::new(0, 0, 400, 300));
    }

    #[test]
    fn cursor_hovers_debounces_and_steps_hierarchy() {
        let panel = candidate(IntRect::new(40, 30, 160, 90), 0.9, Sources::CONTOUR);
        let window = candidate(IntRect::new(0, 0, 240, 100), 0.92, Sources::WINDOW);
        let full = IntRect::new(0, 0, 240, 160);
        let detector = RegionDetector::from_candidates(240, 160, vec![panel.clone(), window]);
        let mut cursor = SelectionCursor::default();

        // hover inside the panel
        assert_eq!(cursor.hover(&detector, 100, 60), panel.rect);
        // small move: no jump
        assert_eq!(cursor.hover(&detector, 101, 61), panel.rect);
        // move out of the panel: the window rect
        assert_eq!(
            cursor.hover(&detector, 10, 50),
            IntRect::new(0, 0, 240, 100)
        );
        // step down into the panel again (manual level)
        assert_eq!(cursor.step(&detector, 100, 60, -1), panel.rect);
        assert!(cursor.manual_level());
        // hover within tolerance keeps the manual level
        assert_eq!(cursor.hover(&detector, 101, 61), panel.rect);
        assert!(cursor.manual_level());
        // step up: panel → window → full bounds
        assert_eq!(
            cursor.step(&detector, 100, 60, 1),
            IntRect::new(0, 0, 240, 100)
        );
        assert_eq!(cursor.step(&detector, 100, 60, 1), full);
        // reset
        cursor.reset();
        assert!(!cursor.manual_level());
    }

    #[test]
    fn window_rects_merge_into_candidates() {
        let mut detector = RegionDetector::from_candidates(
            240,
            160,
            vec![candidate(
                IntRect::new(10, 10, 100, 100),
                0.9,
                Sources::CONTOUR,
            )],
        );
        detector.add_window_rects(&[IntRect::new(0, 0, 240, 160), IntRect::new(-5, -5, 4, 4)]);
        // the tiny clipped-out rect is dropped, the window kept
        assert_eq!(detector.candidates().len(), 2);
        assert!(
            detector
                .candidates()
                .iter()
                .any(|c| c.sources.has(Sources::WINDOW))
        );
    }

    #[test]
    fn analyze_end_to_end_on_synthetic_screen() {
        // two panels on one dark screen
        let mut image = panel_image(300, 200, IntRect::new(20, 20, 140, 90));
        for y in 120..170 {
            for x in 160..280 {
                let i = (y * 300 + x) * 4;
                image.data[i] = 200;
                image.data[i + 1] = 210;
                image.data[i + 2] = 200;
            }
        }
        let detector = RegionDetector::analyze(&image.data, 300, 200);
        assert!(!detector.candidates().is_empty());
        // hovering inside the second panel must return it (not bounds)
        let rect = detector.region_at(200, 140);
        assert!(rect.left >= 155 && rect.right <= 285, "got {rect:?}");
        assert!(rect.top >= 115 && rect.bottom <= 175, "got {rect:?}");
    }

    #[test]
    fn analyze_rejects_truncated_input() {
        let detector = RegionDetector::analyze(&[0u8; 16], 100, 100);
        assert!(detector.candidates().is_empty());
        let view = RgbaView::new(&[0u8; 16], 100, 100);
        assert!(view.is_none());
    }

    #[test]
    fn trace_records_proposals_candidates_and_rejections() {
        let image = panel_image(240, 160, IntRect::new(40, 30, 160, 90));
        let (detector, trace) = RegionDetector::analyze_with_trace(&image.data, 240, 160, true);
        let trace = trace.expect("trace was requested");
        assert_eq!(trace.image, [240, 160]);
        assert_eq!(trace.analysis, [240, 160]); // below the 1280 cap
        assert!((5..=18).contains(&trace.threshold));
        assert!(!trace.edge_field.is_empty());
        // scoring saw proposals and accepted at least one
        assert!(!trace.proposals.is_empty());
        assert!(trace.proposals.iter().any(|p| p.rejected.is_none()));
        assert!(trace.proposals.iter().any(|p| p.rejected.is_some()));
        // the final ledger mirrors the detector's candidates
        assert_eq!(trace.candidates.len(), detector.candidates().len());
        // the no-trace path stays clean
        let (_, none) = RegionDetector::analyze_with_trace(&image.data, 240, 160, false);
        assert!(none.is_none());
    }
}
