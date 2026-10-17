//! The pixel buffers and their cheap transforms: RGBA views over the
//! frozen capture, the grayscale/downsampled analysis buffers, and
//! the morphology + Canny that turn them into binary structure maps.
//! Pure array work — no allocation strategy beyond `Vec`, no unsafe.

// ── Buffers ──────────────────────────────────────────────────────────

/// Single-channel buffer, row-major.
pub(super) struct GrayImage {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) pixels: Vec<u8>,
}

impl GrayImage {
    pub(super) fn new(width: i32, height: i32) -> Self {
        Self {
            width: width.max(0),
            height: height.max(0),
            pixels: vec![0; (width.max(0) as usize) * (height.max(0) as usize)],
        }
    }
    /// Checked read; outside is 0 — the edge field treats out-of-bounds
    /// neighbors as flat, same as mark-shot's guarded lambdas.
    pub(super) fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[y as usize * self.width as usize + x as usize]
    }
    /// Checked write; outside is dropped (callers stay in-bounds by
    /// construction, this is the panic guard for the background thread).
    pub(super) fn set(&mut self, x: i32, y: i32, value: u8) {
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
pub(super) struct RgbaView<'a> {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) data: &'a [u8],
}

impl<'a> RgbaView<'a> {
    pub(super) fn new(data: &'a [u8], width: i32, height: i32) -> Option<Self> {
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
    pub(super) fn px(&self, x: i32, y: i32) -> [u8; 3] {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return [0, 0, 0];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }
}

/// Max channel difference between two RGB triples — the "color
/// distance" every threshold below is stated in.
pub(super) fn color_difference(a: [u8; 3], b: [u8; 3]) -> u8 {
    let dr = (i32::from(a[0]) - i32::from(b[0])).abs();
    let dg = (i32::from(a[1]) - i32::from(b[1])).abs();
    let db = (i32::from(a[2]) - i32::from(b[2])).abs();
    dr.max(dg).max(db).min(255) as u8
}

/// Owned RGBA buffer at analysis resolution.
pub(super) struct RgbaBuf {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) data: Vec<u8>,
}

impl RgbaBuf {
    pub(super) fn view(&self) -> RgbaView<'_> {
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
pub(super) fn grayscale(view: RgbaView<'_>) -> GrayImage {
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
pub(super) fn downscale(source: &RgbaView<'_>, target_w: i32, target_h: i32) -> RgbaBuf {
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
pub(super) fn dilate_linf1(image: &GrayImage) -> GrayImage {
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
pub(super) fn erode_linf1(image: &GrayImage) -> GrayImage {
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
pub(super) fn close_linf1(image: &GrayImage) -> GrayImage {
    erode_linf1(&dilate_linf1(image))
}

/// Separable Gaussian blur, radius 3, σ 1.4, quantized to 8 bits.
pub(super) fn gaussian_blur(image: &GrayImage) -> GrayImage {
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
pub(super) fn canny(image: &GrayImage, low_threshold: f32, high_threshold: f32) -> GrayImage {
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
