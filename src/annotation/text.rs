//! System-font shaping, fallback and rasterization for multilingual annotations.
use super::Shape;
use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use gpui_kit::{Pixels, Point};
use std::cell::RefCell;

thread_local! {
    static FONTS: RefCell<(FontSystem, SwashCache)> = RefCell::new((FontSystem::new(), SwashCache::new()));
}

pub(crate) fn with_fonts<T>(f: impl FnOnce(&mut FontSystem) -> T) -> T {
    FONTS.with_borrow_mut(|(fonts, _)| f(fonts))
}

/// Isolate layout tests from installed fonts and platform fallback differences.
#[cfg(test)]
pub(crate) fn with_test_font<T>(f: impl FnOnce() -> T) -> T {
    struct RestoreFonts(Option<(FontSystem, SwashCache)>);
    impl Drop for RestoreFonts {
        fn drop(&mut self) {
            FONTS.with(|fonts| fonts.replace(self.0.take().unwrap()));
        }
    }
    let mut db = cosmic_text::fontdb::Database::new();
    db.load_font_data(include_bytes!("../../assets/DejaVuSans-Bold.ttf").to_vec());
    db.set_sans_serif_family("DejaVu Sans");
    let fonts = FontSystem::new_with_locale_and_db("en-US".into(), db);
    let _restore = RestoreFonts(Some(
        FONTS.with(|cell| cell.replace((fonts, SwashCache::new()))),
    ));
    f()
}
pub(crate) fn buffer(text: &str, width: f32, limit: f32, font_size: f32) -> Buffer {
    styled_buffer(text, width, limit, font_size, &TextStyle::default())
}

/// All text paths use the same font attributes, wrapping and decoration metrics.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextStyle {
    pub(crate) font: Option<String>,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) outline: Option<u32>,
    pub(crate) background: Option<u32>,
    pub(crate) padding: f32,
    pub(crate) radius: f32,
    pub(crate) rotation: f32,
    pub(crate) wrap: TextWrap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextWrap {
    #[default]
    Word,
    Character,
    None,
}

impl TextWrap {
    pub(crate) const ALL: [Self; 3] = [Self::Word, Self::Character, Self::None];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Character => "character",
            Self::None => "none",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: None,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            padding: 6.,
            radius: 4.,
            rotation: 0.,
            wrap: TextWrap::Word,
        }
    }
}

impl TextStyle {
    pub(crate) fn inset(&self) -> f32 {
        (if self.background.is_some() {
            self.padding
        } else {
            0.
        }) + if self.outline.is_some() { 2. } else { 0. }
    }

    pub(crate) fn attrs(&self) -> Attrs<'_> {
        Attrs::new()
            .family(self.font.as_deref().map_or(Family::SansSerif, Family::Name))
            .weight(if self.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            })
            .style(if self.italic {
                cosmic_text::Style::Italic
            } else {
                cosmic_text::Style::Normal
            })
    }

    pub(crate) fn wrapping(&self) -> cosmic_text::Wrap {
        match self.wrap {
            TextWrap::Word => cosmic_text::Wrap::WordOrGlyph,
            TextWrap::Character => cosmic_text::Wrap::Glyph,
            TextWrap::None => cosmic_text::Wrap::None,
        }
    }
}

impl super::Annotations {
    pub(crate) fn text_style(&self) -> &TextStyle {
        self.editing_text()
            .or_else(|| self.selected().filter(|s| s.kind == super::ShapeKind::Text))
            .map(|s| &s.text_style)
            .unwrap_or(&self.text_style)
    }

    /// Apply one reversible style edit, rejecting layouts that cannot fit the crop.
    pub(crate) fn set_text_style(&mut self, value: TextStyle) {
        self.end_size_drag();
        self.drag_text_style(value);
        self.end_size_drag();
    }

    /// Merge continuous decoration changes into one reversible edit.
    pub(crate) fn drag_text_style(&mut self, value: TextStyle) {
        if self.text_style() == &value {
            return;
        }
        if self.editing_text().is_some() {
            self.text_style = value.clone();
            if let Some(shape) = self.editing_text_mut() {
                shape.text_style = value;
            }
            return;
        }
        if let Some(ix) = self.selected_index()
            && self.shapes[ix].kind == super::ShapeKind::Text
        {
            let before = self.shapes[ix].clone();
            let mut after = before.clone();
            after.text_style = value.clone();
            let limit = self.resize_bounds.map_or(before.bounds.size.width, |area| {
                area.right() - before.bounds.left()
            });
            let buffer = styled_buffer(
                after.text.as_deref().unwrap_or(""),
                (f32::from(limit) - value.inset() * 2.).max(1.),
                f32::MAX,
                after.size(),
                &value,
            );
            let width = buffer.layout_runs().map(|r| r.line_w).fold(1., f32::max) + 1.;
            let height = buffer
                .layout_runs()
                .map(|r| r.line_top + r.line_height)
                .fold(after.size() * 1.35, f32::max);
            after.bounds.size = gpui_kit::size(
                gpui_kit::px(width + value.inset() * 2.),
                gpui_kit::px(height + value.inset() * 2.),
            );
            if self.resize_bounds.is_some_and(|area| {
                let b = after.ink_bounds();
                b.left() < area.left()
                    || b.top() < area.top()
                    || b.right() > area.right()
                    || b.bottom() > area.bottom()
            }) {
                return;
            }
            self.shapes[ix] = after;
            if self.size_drag_active
                && let Some(super::HistoryEntry::Edit {
                    ix: previous,
                    after,
                    ..
                }) = self.history.last_mut()
                && *previous == ix
            {
                *after = self.shapes[ix].clone();
            } else {
                self.commit_move(ix, before);
                self.size_drag_active = true;
            }
        }
        self.text_style = value;
    }
}

/// Enumerate installed families once; font discovery is not repeated per frame.
pub(crate) fn families() -> &'static [String] {
    static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FAMILIES.get_or_init(|| {
        with_fonts(|fonts| {
            let mut names: Vec<_> = fonts
                .db()
                .faces()
                .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
                .collect();
            names.sort_unstable();
            names.dedup();
            names
        })
    })
}

pub(crate) fn styled_buffer(
    text: &str,
    width: f32,
    limit: f32,
    font_size: f32,
    style: &TextStyle,
) -> Buffer {
    with_fonts(|fonts| {
        let mut buffer = Buffer::new(fonts, Metrics::new(font_size, font_size * 1.35));
        buffer.set_size(Some(width), Some(limit));
        buffer.set_wrap(style.wrapping());
        buffer.set_text(text, &style.attrs(), Shaping::Advanced, None);
        buffer.shape_until_scroll(fonts, false);
        buffer
    })
}

pub(super) fn rasterize(
    shape: &Shape,
    rgba: &mut [u8],
    w: u32,
    h: u32,
    origin: Point<Pixels>,
    scale: f32,
) {
    let Some(text) = &shape.text else {
        return;
    };
    let width = (f32::from(shape.bounds.size.width) * scale).round().max(0.) as usize;
    let height = (f32::from(shape.bounds.size.height) * scale)
        .round()
        .max(0.) as usize;
    if width == 0 || height == 0 {
        return;
    }
    let style = &shape.text_style;
    let inset = (style.inset() * scale).round() as i32;
    let mut glyphs = vec![0u8; width * height * 4];
    FONTS.with_borrow_mut(|(fonts, cache)| {
        let mut buffer = Buffer::new(
            fonts,
            Metrics::new(shape.size() * scale, shape.size() * scale * 1.35),
        );
        buffer.set_size(
            Some((width as f32 - 2. * inset as f32).max(1.)),
            Some(height as f32),
        );
        buffer.set_wrap(style.wrapping());
        buffer.set_text(text, &style.attrs(), Shaping::Advanced, None);
        let [r, g, b, a] = shape.color.to_be_bytes();
        buffer.draw(
            fonts,
            cache,
            Color::rgba(r, g, b, a),
            |x, y, pw, ph, color| {
                for row in (y + inset).max(0)..(y + inset + ph as i32).min(height as i32) {
                    for col in (x + inset).max(0)..(x + inset + pw as i32).min(width as i32) {
                        let offset = (row as usize * width + col as usize) * 4;
                        over(
                            &mut glyphs[offset..offset + 4],
                            [color.r(), color.g(), color.b(), color.a()],
                        );
                    }
                }
            },
        );
    });
    let mut tile = vec![0u8; glyphs.len()];
    if let Some(color) = style.background {
        let radius = (style.radius * scale).min(width.min(height) as f32 / 2.);
        for y in 0..height {
            for x in 0..width {
                let dx = (radius - (x as f32 + 0.5).min(width as f32 - x as f32 - 0.5)).max(0.);
                let dy = (radius - (y as f32 + 0.5).min(height as f32 - y as f32 - 0.5)).max(0.);
                let coverage = (radius + 0.5 - dx.hypot(dy)).clamp(0., 1.);
                let mut color = color.to_be_bytes();
                color[3] = (color[3] as f32 * coverage).round() as u8;
                tile[(y * width + x) * 4..][..4].copy_from_slice(&color);
            }
        }
    }
    if let Some(color) = style.outline {
        let radius = (1.5 * scale).ceil() as i32;
        for y in 0..height {
            for x in 0..width {
                let mut coverage = 0u8;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if dx * dx + dy * dy > radius * radius {
                            continue;
                        }
                        let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                        if sx >= 0 && sy >= 0 && sx < width as i32 && sy < height as i32 {
                            coverage =
                                coverage.max(glyphs[(sy as usize * width + sx as usize) * 4 + 3]);
                        }
                    }
                }
                let mut color = color.to_be_bytes();
                color[3] = (color[3] as u16 * coverage as u16 / 255) as u8;
                over(&mut tile[(y * width + x) * 4..][..4], color);
            }
        }
    }
    for (dst, src) in tile
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(glyphs.as_chunks::<4>().0.iter())
    {
        over(dst, [src[0], src[1], src[2], src[3]]);
    }
    let left = (f32::from(shape.bounds.left() - origin.x) * scale).round();
    let top = (f32::from(shape.bounds.top() - origin.y) * scale).round();
    let b = shape.ink_bounds();
    let x0 = ((f32::from(b.left() - origin.x) * scale).floor() as i32).max(0);
    let y0 = ((f32::from(b.top() - origin.y) * scale).floor() as i32).max(0);
    let x1 = ((f32::from(b.right() - origin.x) * scale).ceil() as i32).min(w as i32);
    let y1 = ((f32::from(b.bottom() - origin.y) * scale).ceil() as i32).min(h as i32);
    let (sin, cos) = style.rotation.sin_cos();
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - left - width as f32 / 2.;
            let dy = y as f32 + 0.5 - top - height as f32 / 2.;
            let sx = (dx * cos + dy * sin + width as f32 / 2.).floor() as i32;
            let sy = (-dx * sin + dy * cos + height as f32 / 2.).floor() as i32;
            if sx < 0 || sy < 0 || sx >= width as i32 || sy >= height as i32 {
                continue;
            }
            let src = (sy as usize * width + sx as usize) * 4;
            let dst = (y as usize * w as usize + x as usize) * 4;
            if rgba[dst + 3] == 0 {
                continue;
            }
            let alpha = tile[src + 3] as f32 / 255.;
            for c in 0..3 {
                rgba[dst + c] = (rgba[dst + c] as f32 * (1. - alpha) + tile[src + c] as f32 * alpha)
                    .round() as u8;
            }
        }
    }
}

fn over(dst: &mut [u8], color: [u8; 4]) {
    let a = color[3] as f32 / 255.;
    let b = dst[3] as f32 / 255.;
    let alpha = a + b * (1. - a);
    if alpha == 0. {
        return;
    }
    for c in 0..3 {
        dst[c] = ((color[c] as f32 * a + dst[c] as f32 * b * (1. - a)) / alpha).round() as u8;
    }
    dst[3] = (alpha * 255.).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::ShapeKind;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn editing_text_commits_complete_style_and_undo_restores_it() {
        with_test_font(|| {
            let mut a = crate::annotation::Annotations::default();
            a.add_text(
                Bounds::new(point(px(50.), px(50.)), size(px(160.), px(80.))),
                "Alpha".into(),
            );
            let before = a.shape(0).unwrap().clone();
            assert!(a.begin_text_edit(0));
            let mut style = a.text_style().clone();
            style.bold = true;
            style.italic = true;
            style.font = Some("DejaVu Sans".into());
            style.background = Some(0x40808080);
            style.outline = Some(0xffffffff);
            style.padding = 10.;
            a.set_text_style(style.clone());
            a.preview_text(before.bounds, "Alpha beta".into());
            let expected = a.shape(0).unwrap().clone();
            a.finish_text_edit(true);
            assert_eq!(a.shape(0), Some(&expected));
            assert_eq!(a.shape(0).unwrap().text_style, style);
            a.undo();
            assert_eq!(a.shape(0), Some(&before));
            a.redo();
            assert_eq!(a.shape(0), Some(&expected));
            assert!(a.begin_text_edit(0));
            a.set_text_style(TextStyle::default());
            a.finish_text_edit(false);
            assert_eq!(a.shape(0), Some(&expected));
        });
    }

    #[test]
    fn background_style_edit_and_size_changes_preserve_content_and_history() {
        with_test_font(|| {
            let mut a = crate::annotation::Annotations::default();
            a.add_text(
                Bounds::new(point(px(50.), px(50.)), size(px(160.), px(80.))),
                "Alpha".into(),
            );
            a.select_index(0);
            let before = a.shape(0).unwrap().clone();
            let area = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(400.)));
            let style = TextStyle {
                background: Some(0x40808080),
                padding: 10.,
                ..Default::default()
            };
            a.edit_within(area, |a| a.set_text_style(style.clone()));
            let styled = a.shape(0).unwrap().clone();
            assert_eq!(styled.text_style, style);
            let inset = px(20.);
            a.apply_size(styled.size());
            a.apply_size(styled.size() * 1.5);
            let scaled = a.shape(0).unwrap().clone();
            assert_eq!(scaled.text_style.padding, 10.);
            assert_eq!(
                scaled.bounds.size.width,
                (styled.bounds.size.width - inset) * 1.5 + inset
            );
            a.end_size_drag();
            a.undo();
            assert_eq!(a.shape(0), Some(&styled));
            a.undo();
            assert_eq!(a.shape(0), Some(&before));
        });
    }

    #[test]
    fn fractional_scale_clips_text_and_preserves_transparent_gaps() {
        for scale in [1_f32, 1.25, 1.73, 2.] {
            let shape = Shape {
                kind: ShapeKind::Text,
                number: None,
                text: Some("Hello 中文\nSecond line".into()),
                text_style: Default::default(),
                bounds: Bounds::new(point(px(10.), px(10.)), size(px(140.), px(90.))),
                color: 0x000000ff,
                parameters: crate::annotation::ShapeParameters::new(ShapeKind::Text, 24.),
                points: Vec::new(),
            };
            let side = (180. * scale).round() as u32;
            let gap = (65. * scale).round() as u32;
            let mut pixels = vec![255; (side * side * 4) as usize];
            for y in 0..side {
                pixels[((y * side + gap) * 4 + 3) as usize] = 0;
            }
            rasterize(
                &shape,
                &mut pixels,
                side,
                side,
                point(px(0.), px(0.)),
                scale,
            );
            let mut rows = [false; 2];
            for (i, p) in pixels.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (i as u32 % side, i as u32 / side);
                if x == gap {
                    assert_eq!(p[3], 0);
                }
                if p[0] != 255 {
                    assert!(x >= (10. * scale).round() as u32 && x < (150. * scale).round() as u32);
                    assert!(y >= (10. * scale).round() as u32 && y < (100. * scale).round() as u32);
                    rows[usize::from(y as f32 >= 42. * scale)] = true;
                }
            }
            assert_eq!(rows, [true, true]);
        }
    }
}
