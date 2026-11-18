//! Sequence badges rendered by ab_glyph, shared by preview and PNG export.
use super::{Annotations, NumberFormat, Shape, ShapeKind, ShapeParameters, history::HistoryEntry};
use ab_glyph::{Font, FontRef, Glyph, ScaleFont};
use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};

impl Annotations {
    pub(crate) fn number_fill(&self) -> bool {
        self.selected()
            .filter(|s| s.kind == ShapeKind::Number)
            .map(Shape::number_fill)
            .unwrap_or(self.number_fill)
    }

    pub(crate) fn set_number_fill(&mut self, value: bool) {
        self.number_fill = value;
        self.edit_selected(|shape| {
            if let ShapeParameters::Badge { fill, .. } = &mut shape.parameters {
                *fill = value;
            }
        });
    }

    pub(crate) fn number_format(&self) -> NumberFormat {
        self.selected()
            .filter(|s| s.kind == ShapeKind::Number)
            .map(Shape::number_format)
            .unwrap_or(self.number_format)
    }

    pub(crate) fn set_number_format(&mut self, value: NumberFormat) {
        self.number_format = value;
        self.edit_selected(|shape| {
            if let ShapeParameters::Badge { format, .. } = &mut shape.parameters {
                *format = value;
            }
        });
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
        FontRef::try_from_slice(include_bytes!("../../assets/Rubik-SemiBold.ttf"))
            .expect("bundled Rubik SemiBold is a valid font")
    })
}

fn layout(label: &str, diameter: f32, filled: bool, center: ab_glyph::Point) -> Vec<Glyph> {
    // 128 px keeps whole-pixel ink-bound rounding below 0.1 px at badge sizes
    // while staying cheap enough (~4k samples) to integrate exact ink mass.
    const MEASURE_PX: f32 = 128.;
    let scaled = font().as_scaled(MEASURE_PX);
    let mut advance = 0.;
    let mut previous = None;
    let mut min = ab_glyph::point(f32::INFINITY, f32::INFINITY);
    let mut max = ab_glyph::point(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut mass = 0.0_f32;
    let mut weighted_x = 0.0_f32;
    let mut glyphs = Vec::new();
    for ch in label.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(previous) = previous {
            advance += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(MEASURE_PX, ab_glyph::point(advance, 0.));
        let Some(outline) = scaled.outline_glyph(glyph.clone()) else {
            continue;
        };
        let bounds = outline.px_bounds();
        min.x = min.x.min(bounds.min.x);
        min.y = min.y.min(bounds.min.y);
        max.x = max.x.max(bounds.max.x);
        max.y = max.y.max(bounds.max.y);
        outline.draw(|gx, _, cov| {
            mass += cov;
            weighted_x += (bounds.min.x + gx as f32 + 0.5) * cov;
        });
        advance += scaled.h_advance(id);
        previous = Some(id);
        glyphs.push(glyph);
    }
    if glyphs.is_empty() || max.x <= min.x || max.y <= min.y {
        return Vec::new();
    }
    // Leave clearance for the optional inset ring (or thicker outline stroke)
    // so multi-digit and wide Roman labels stay comfortably inside the circle.
    let span_x = max.x - min.x;
    let (h_ratio, w_ratio) = if filled { (0.54, 0.66) } else { (0.48, 0.58) };
    let factor = (diameter * h_ratio / (max.y - min.y)).min(diameter * w_ratio / span_x);
    // Pure bounding-box centering shoves strings starting with '1' ("1", "11",
    // "14", "15") to the right because '1' has a left top beak over empty space
    // and a solid stem on the right (and conversely pulls "IV" left). Blending
    // 55% toward the true horizontal ink centroid optically balances every label.
    let bbox_mid_x = (min.x + max.x) / 2.;
    let centroid_x = if mass > 0. {
        weighted_x / mass
    } else {
        bbox_mid_x
    };
    let shift = (0.55 * (centroid_x - bbox_mid_x)).clamp(-0.12 * span_x, 0.12 * span_x);
    let mid_x = bbox_mid_x + shift;
    let mid_y = (min.y + max.y) / 2.;
    for glyph in &mut glyphs {
        glyph.scale = (MEASURE_PX * factor).into();
        glyph.position.x = center.x + (glyph.position.x - mid_x) * factor;
        glyph.position.y = center.y - mid_y * factor;
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

fn span_coverage(dy: f32, col: f32, center_x: f32, radius: f32) -> f32 {
    if dy.abs() >= radius {
        return 0.;
    }
    let half = (radius * radius - dy * dy).sqrt();
    ((center_x + half).min(col + 1.) - (center_x - half).max(col)).clamp(0., 1.)
}

const SHADOW_PEAK_FILLED: f32 = 0.25;
const SHADOW_PEAK_OUTLINE_DARK: f32 = 0.30;
const SHADOW_PEAK_OUTLINE_LIGHT: f32 = 0.42;

/// Porter-Duff `source over` into straight (un-premultiplied) RGBA. Inside an
/// opaque interior (`*dst_a >= 0.999`), blend RGB in linear light so white and
/// black numerals avoid gamma darkening fringes; over translucent shadow edges,
/// preserve exact straight-alpha associativity with GPU/export compositing.
fn composite_over(dst_rgb: &mut [f32; 3], dst_a: &mut f32, src_rgb: [u8; 3], src_a: f32) {
    let src_a = src_a.clamp(0., 1.);
    if src_a <= 0. {
        return;
    }
    let out_a = src_a + *dst_a * (1. - src_a);
    if out_a <= 0. {
        return;
    }
    if *dst_a >= 0.999 {
        let base = [
            dst_rgb[0].round() as u8,
            dst_rgb[1].round() as u8,
            dst_rgb[2].round() as u8,
        ];
        let mut out = [0; 3];
        blend_srgb(&mut out, &base, &src_rgb, src_a);
        *dst_rgb = [out[0] as f32, out[1] as f32, out[2] as f32];
    } else {
        for c in 0..3 {
            dst_rgb[c] = (src_rgb[c] as f32 * src_a + dst_rgb[c] * *dst_a * (1. - src_a)) / out_a;
        }
    }
    *dst_a = out_a;
}

/// Shift an alpha mask downward by `dy` (with subpixel vertical interpolation)
/// and run two passes of separable 1D box blur (tent kernel) for outline shadows.
fn blur_shifted_alpha(alpha: &[f32], width: u32, height: u32, dy: f32, radius: usize) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let dy_floor = dy.floor() as isize;
    let frac = dy - dy.floor();
    let sample = |x: usize, y: isize| -> f32 {
        if y < 0 || y >= h as isize {
            0.
        } else {
            alpha[y as usize * w + x]
        }
    };
    let mut buf = vec![0.0_f32; w * h];
    for y in 0..h {
        let sy = y as isize - dy_floor;
        for x in 0..w {
            buf[y * w + x] = sample(x, sy) * (1. - frac) + sample(x, sy - 1) * frac;
        }
    }
    if radius == 0 {
        return buf;
    }
    let mut tmp = vec![0.0_f32; w * h];
    let r = radius as isize;
    let norm = 1. / (2 * radius + 1) as f32;
    for _ in 0..2 {
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let mut sum = 0.;
                for k in -r..=r {
                    let sx = (x as isize + k).clamp(0, w as isize - 1) as usize;
                    sum += buf[row + sx];
                }
                tmp[row + x] = sum * norm;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.;
                for k in -r..=r {
                    let sy = (y as isize + k).clamp(0, h as isize - 1) as usize;
                    sum += tmp[sy * w + x];
                }
                buf[y * w + x] = sum * norm;
            }
        }
    }
    buf
}

fn render(shape: &super::Shape, origin: Point<Pixels>, scale: f32) -> Badge {
    let x = f32::from(shape.bounds.left() - origin.x) * scale;
    let y = f32::from(shape.bounds.top() - origin.y) * scale;
    let diameter = f32::from(shape.bounds.size.width) * scale;
    let radius = diameter / 2.;
    let shadow_dy = 0.85 * scale;
    let shadow_spread = 0.4 * scale;
    let shadow_blur = (diameter * 0.085).clamp(1.75 * scale, 3.6 * scale);
    let pad = (shadow_blur + shadow_dy + shadow_spread).ceil();
    let left = (x - pad).floor() as i32;
    let top = (y - pad).floor() as i32;
    let width = ((x + diameter + pad).ceil() - left as f32) as u32;
    let height = ((y + diameter + pad).ceil() - top as f32) as u32;
    let mut rgba = vec![0; (width * height * 4) as usize];
    let center = ab_glyph::point(x - left as f32 + radius, y - top as f32 + radius);
    let background = shape.color.to_be_bytes();
    let bg_rgb = [background[0], background[1], background[2]];
    let label = shape.number_format().label(shape.number.unwrap_or(1));

    if shape.number_fill() {
        let ring = badge_ring(shape.color, diameter, scale);
        // Subtle 360° ambient + downward drop shadow separates vivid saturated
        // badges (red, blue, green) from identical-hue backgrounds without
        // forcing a heavy dark/white border onto saturated circles.
        for row in 0..height {
            for col in 0..width {
                let px_x = col as f32 + 0.5;
                let px_y = row as f32 + 0.5;
                let dist = (px_x - center.x).hypot(px_y - (center.y + shadow_dy))
                    - (radius + shadow_spread);
                let t = 1. - (dist / shadow_blur).clamp(0., 1.);
                let mut alpha = SHADOW_PEAK_FILLED * t * t * (3. - 2. * t);
                let mut rgb = [0.0_f32; 3];

                let mut outer = 0.;
                let mut inner = 0.;
                for sample in 0..8 {
                    let dy = row as f32 + (sample as f32 + 0.5) / 8. - center.y;
                    outer += span_coverage(dy, col as f32, center.x, radius) / 8.;
                    if let Some((_, ring_width)) = ring {
                        inner +=
                            span_coverage(dy, col as f32, center.x, (radius - ring_width).max(0.))
                                / 8.;
                    }
                }
                if outer > 0. {
                    let body = if let Some((ring_color, _)) = ring {
                        let ring_share = ((outer - inner) / outer).clamp(0., 1.);
                        let mut mixed = [0; 3];
                        blend_srgb(&mut mixed, &bg_rgb, &ring_color, ring_share);
                        mixed
                    } else {
                        bg_rgb
                    };
                    composite_over(&mut rgb, &mut alpha, body, outer);
                }
                let offset = ((row * width + col) * 4) as usize;
                rgba[offset] = rgb[0].round() as u8;
                rgba[offset + 1] = rgb[1].round() as u8;
                rgba[offset + 2] = rgb[2].round() as u8;
                rgba[offset + 3] = (alpha * 255.).round() as u8;
            }
        }
        let fg = foreground(shape.color).to_be_bytes();
        let fg_rgb = [fg[0], fg[1], fg[2]];
        for glyph in layout(&label, diameter, true, center) {
            let Some(outline) = font().outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px_x = gx as i32 + bounds.min.x as i32;
                let px_y = gy as i32 + bounds.min.y as i32;
                if px_x < 0 || px_y < 0 || px_x >= width as i32 || px_y >= height as i32 {
                    return;
                }
                let offset = ((px_y as u32 * width + px_x as u32) * 4) as usize;
                let mut rgb = [
                    rgba[offset] as f32,
                    rgba[offset + 1] as f32,
                    rgba[offset + 2] as f32,
                ];
                let mut alpha = rgba[offset + 3] as f32 / 255.;
                composite_over(&mut rgb, &mut alpha, fg_rgb, coverage);
                rgba[offset] = rgb[0].round() as u8;
                rgba[offset + 1] = rgb[1].round() as u8;
                rgba[offset + 2] = rgb[2].round() as u8;
                rgba[offset + 3] = (alpha * 255.).round() as u8;
            });
        }
    } else {
        // Outline style: hollow circle ring and numeral share `shape.color`,
        // backed by a soft blurred shadow/halo so white-on-white and
        // black-on-black outline badges remain legible.
        let stroke_w = (diameter * 0.085).max(1.75 * scale);
        let inner_r = (radius - stroke_w).max(0.);
        let mut ink = vec![0.0_f32; (width * height) as usize];
        for row in 0..height {
            for col in 0..width {
                let mut outer = 0.;
                let mut inner = 0.;
                for sample in 0..8 {
                    let dy = row as f32 + (sample as f32 + 0.5) / 8. - center.y;
                    outer += span_coverage(dy, col as f32, center.x, radius) / 8.;
                    inner += span_coverage(dy, col as f32, center.x, inner_r) / 8.;
                }
                ink[(row * width + col) as usize] = (outer - inner).clamp(0., 1.);
            }
        }
        for glyph in layout(&label, diameter, false, center) {
            let Some(outline) = font().outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px_x = gx as i32 + bounds.min.x as i32;
                let px_y = gy as i32 + bounds.min.y as i32;
                if px_x < 0 || px_y < 0 || px_x >= width as i32 || px_y >= height as i32 {
                    return;
                }
                let ix = (px_y as u32 * width + px_x as u32) as usize;
                let cov = coverage.clamp(0., 1.);
                ink[ix] = 1. - (1. - ink[ix]) * (1. - cov);
            });
        }
        let dark_ink = relative_luminance(shape.color) < DARK_BADGE_LUMINANCE;
        let shadow_dy_outline = if dark_ink { 0. } else { 0.75 * scale };
        let blur_r = (1.4 * scale).ceil() as usize;
        let blurred = blur_shifted_alpha(&ink, width, height, shadow_dy_outline, blur_r);
        let (shadow_rgb, peak) = if dark_ink {
            ([255.0_f32; 3], SHADOW_PEAK_OUTLINE_LIGHT)
        } else {
            ([0.0_f32; 3], SHADOW_PEAK_OUTLINE_DARK)
        };
        for (ix, (&fg_a, &sh)) in ink.iter().zip(blurred.iter()).enumerate() {
            let mut rgb = shadow_rgb;
            let mut alpha = (sh * peak).clamp(0., 1.);
            composite_over(&mut rgb, &mut alpha, bg_rgb, fg_a);
            let offset = ix * 4;
            rgba[offset] = rgb[0].round() as u8;
            rgba[offset + 1] = rgb[1].round() as u8;
            rgba[offset + 2] = rgb[2].round() as u8;
            rgba[offset + 3] = (alpha * 255.).round() as u8;
        }
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
    entries: HashMap<[u32; 9], NumberImage>,
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
                    u32::from(shape.number_fill()),
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
fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0., 1.);
    let srgb = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    };
    (srgb * 255.).round() as u8
}

fn blend_srgb(dst: &mut [u8], a: &[u8], b: &[u8], t: f32) {
    for channel in 0..3 {
        let la = srgb_to_linear(a[channel]);
        let lb = srgb_to_linear(b[channel]);
        dst[channel] = linear_to_srgb(la * (1. - t) + lb * t);
    }
}

fn relative_luminance(color: u32) -> f32 {
    let rgb = color.to_be_bytes();
    0.2126 * srgb_to_linear(rgb[0])
        + 0.7152 * srgb_to_linear(rgb[1])
        + 0.0722 * srgb_to_linear(rgb[2])
}

/// Threshold raised above the WCAG midpoint (0.179) so vivid saturated fills
/// like red, orange, green and blue keep crisp white numerals; only genuinely
/// bright fills (yellow, white, pastels) flip to black.
const LIGHT_BADGE_LUMINANCE: f32 = 0.50;
const DARK_BADGE_LUMINANCE: f32 = 0.05;

pub(super) fn foreground(color: u32) -> u32 {
    if relative_luminance(color) > LIGHT_BADGE_LUMINANCE {
        0x000000ff
    } else {
        0xffffffff
    }
}

/// Light badges get a dark rim so white/yellow circles stay legible on light
/// screenshots; near-black badges get a soft light rim so they separate from
/// dark windows while staying subtle on white ones.
fn badge_ring(color: u32, diameter: f32, scale: f32) -> Option<([u8; 3], f32)> {
    let luminance = relative_luminance(color);
    if luminance > LIGHT_BADGE_LUMINANCE {
        Some(([0, 0, 0], (diameter * 0.055).max(1.25 * scale)))
    } else if luminance < DARK_BADGE_LUMINANCE {
        let bg = color.to_be_bytes();
        let mut rim = [0; 3];
        blend_srgb(&mut rim, &bg[..3], &[255, 255, 255], 0.72);
        Some((rim, (diameter * 0.045).max(1.0 * scale)))
    } else {
        None
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
        let samples = (0..=12)
            .chain([99, 100, 1000, u32::MAX])
            .map(|n| n.to_string())
            .chain(('A'..='Z').map(|ch| ch.to_string()))
            .chain(["IV", "VIII", "XXXVIII", "MMMDCCCLXXXVIII"].map(str::to_owned));
        for label in samples {
            for filled in [true, false] {
                for diameter in [16., 24., 32., 40.] {
                    for scale in [1., 1.25, 1.73, 2.] {
                        let diameter = diameter * scale;
                        for glyph in layout(
                            &label,
                            diameter,
                            filled,
                            ab_glyph::point(diameter / 2., diameter / 2.),
                        ) {
                            let outline = font().outline_glyph(glyph).unwrap();
                            let bounds = outline.px_bounds();
                            assert!(bounds.min.x >= 0. && bounds.min.y >= 0.);
                            assert!(
                                bounds.max.x <= diameter.ceil() && bounds.max.y <= diameter.ceil()
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(foreground(0xffffffff), 0x000000ff);
        assert_eq!(foreground(0x000000ff), 0xffffffff);
        // Default swatches: vivid colors and black take white text; yellow and white take black.
        let defaults = crate::model::settings::DEFAULT_ANNOTATION_COLORS;
        assert_eq!(foreground(defaults[0]), 0xffffffff); // Red
        assert_eq!(foreground(defaults[1]), 0xffffffff); // Orange
        assert_eq!(foreground(defaults[2]), 0x000000ff); // Yellow
        assert_eq!(foreground(defaults[3]), 0xffffffff); // Green
        assert_eq!(foreground(defaults[4]), 0xffffffff); // Blue
        assert_eq!(foreground(defaults[5]), 0xffffffff); // Black
        assert_eq!(foreground(defaults[6]), 0x000000ff); // White
    }
    #[test]
    fn asymmetric_labels_are_optically_centered_by_ink_mass() {
        for label in ["1", "11", "14", "15", "IV"] {
            let center = ab_glyph::point(32., 32.);
            let glyphs = layout(label, 64., true, center);
            let mut mass = 0.0_f32;
            let mut weighted_x = 0.0_f32;
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for glyph in glyphs {
                let outline = font().outline_glyph(glyph).unwrap();
                let bounds = outline.px_bounds();
                min_x = min_x.min(bounds.min.x);
                max_x = max_x.max(bounds.max.x);
                outline.draw(|gx, _, cov| {
                    mass += cov;
                    weighted_x += (bounds.min.x + gx as f32 + 0.5) * cov;
                });
            }
            let centroid_err = (weighted_x / mass - center.x).abs();
            let naive_err = (weighted_x / mass - (min_x + max_x) / 2.).abs();
            assert!(
                centroid_err < naive_err * 0.6,
                "label {label} remained off-center: centroid_err={centroid_err}, naive_err={naive_err}"
            );
        }
    }
    #[test]
    fn contrasting_rings_and_shadows_separate_all_badges_from_matching_backgrounds() {
        let defaults = crate::model::settings::DEFAULT_ANNOTATION_COLORS;
        assert!(badge_ring(defaults[0], 32., 1.).is_none());
        assert!(badge_ring(defaults[4], 32., 1.).is_none());

        let mut white_badge = badge(8, 32.);
        white_badge.bounds.origin = point(px(0.), px(0.));
        white_badge.color = defaults[6];
        let rendered_white = render(&white_badge, point(px(0.), px(0.)), 1.);
        let top_rim_row = (0 - rendered_white.top) as u32;
        let top_edge =
            ((top_rim_row * rendered_white.width + rendered_white.width / 2) * 4) as usize;
        assert_eq!(&rendered_white.rgba[top_edge..top_edge + 3], &[0, 0, 0]);
        assert!(rendered_white.rgba[top_edge + 3] > 180);

        let mut black_badge = badge(7, 32.);
        black_badge.bounds.origin = point(px(0.), px(0.));
        black_badge.color = defaults[5];
        let rendered_black = render(&black_badge, point(px(0.), px(0.)), 1.);
        let top_rim_row = (0 - rendered_black.top) as u32;
        let top_edge =
            ((top_rim_row * rendered_black.width + rendered_black.width / 2) * 4) as usize;
        assert!(rendered_black.rgba[top_edge] > 160);
        assert_eq!(
            rendered_black.rgba[top_edge],
            rendered_black.rgba[top_edge + 1]
        );

        // Saturated red badge on an identical red background stays separated on all 4 sides.
        let red = defaults[0].to_be_bytes();
        let mut red_badge = badge(1, 32.);
        red_badge.bounds.origin = point(px(10.), px(10.));
        red_badge.color = defaults[0];
        let mut canvas = red.repeat(64 * 64);
        rasterize(&red_badge, &mut canvas, 64, 64, point(px(0.), px(0.)), 1.);
        for (px_x, px_y) in [(26, 9), (26, 42), (9, 26), (42, 26)] {
            let off = (px_y * 64 + px_x) * 4;
            assert!(
                canvas[off] < red[0] - 12,
                "expected shadow separation at ({px_x}, {px_y}), got {:?}",
                &canvas[off..off + 4]
            );
        }
    }
    #[test]
    fn outline_badges_preserve_transparent_interiors_and_invalidate_cache_with_undo() {
        let defaults = crate::model::settings::DEFAULT_ANNOTATION_COLORS;
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Number);
        let area = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(120.)));
        marks.begin(point(px(50.), px(50.)), area, false);
        marks.end();
        marks.select_index(0);

        let mut cache = NumberCache::default();
        let filled_img = cache.prepare(&marks.shapes, 1.)[0].clone().unwrap();
        marks.set_number_fill(false);
        assert!(!marks.number_fill());
        assert!(!marks.shapes[0].number_fill());
        let outline_img = cache.prepare(&marks.shapes, 1.)[0].clone().unwrap();
        assert!(!Arc::ptr_eq(&filled_img.image, &outline_img.image));

        // Sample inside the ring but outside the central '1' glyph: interior stays transparent.
        let rendered = render(&marks.shapes[0], point(px(0.), px(0.)), 2.);
        let cx = (50. * 2. - rendered.left as f32) as u32;
        let cy = (50. * 2. - rendered.top as f32) as u32;
        let hole_off = ((cy * rendered.width + (cx - 18)) * 4) as usize;
        assert_eq!(rendered.rgba[hole_off + 3], 0);

        // White outline on white and black outline on black both remain visible.
        for (color, bg) in [
            (defaults[6], [255u8, 255, 255, 255]),
            (defaults[5], [0, 0, 0, 255]),
        ] {
            let mut s = marks.shapes[0].clone();
            s.color = color;
            let mut canvas = bg.repeat(120 * 120);
            rasterize(&s, &mut canvas, 120, 120, point(px(0.), px(0.)), 1.);
            assert!(
                canvas.as_chunks::<4>().0.iter().any(|p| *p != bg),
                "outline badge {color:#010x} vanished on matching background"
            );
        }

        marks.undo();
        assert!(marks.shapes[0].number_fill());
        marks.redo();
        assert!(!marks.shapes[0].number_fill());
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
    fn grouped_deletion_preserves_duplicate_numbers_and_cursor_through_undo() {
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
            marks.select_indices(vec![1, 2, 1, usize::MAX]);
            assert!(marks.delete_selected());
            assert_eq!(
                values(&marks),
                if enabled { vec![3, 7] } else { vec![4, 9] }
            );
            assert_eq!(marks.next_number(), if enabled { 18 } else { 20 });
            let deleted = marks.committed().to_vec();
            marks.undo();
            assert_eq!(marks.committed(), original);
            assert_eq!(marks.next_number(), 20);
            marks.redo();
            assert_eq!(marks.committed(), deleted);
            marks.undo();
            assert_eq!(marks.committed(), original);
            assert_eq!(marks.next_number(), 20);
        }
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
            for direction in 0..8 {
                let mut marks = Annotations::default();
                let (sin, cos) = (direction as f32 * std::f32::consts::TAU / 8.).sin_cos();
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
