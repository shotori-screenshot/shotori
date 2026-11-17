//! Annotation settings page: editing defaults, 7-color palette editor, initial
//! swatch choices, and per-tool sizes & styles grouped by tool family.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use crate::annotation::text::TextWrap;
use crate::annotation::{ArrowStyle, HighlightBlend, NumberFormat};
use crate::model::settings::{
    ARROW_STYLES, HIGHLIGHT_BLENDS, NUMBER_FORMATS, PALETTE, Settings, arrow_style_label,
    highlight_blend_label, number_format_label,
};
use crate::ui::color_picker::ColorPicker;
use crate::ui::toolbar::{hsla_to_u32, u32_to_hsla};

use super::{SettingsWindow, card_group, card_row, page_body, page_header};

#[derive(Clone)]
pub(super) struct ArrowStyleItem(ArrowStyle);

impl SearchableListItem for ArrowStyleItem {
    type Value = ArrowStyle;

    fn title(&self) -> SharedString {
        arrow_style_label(self.0).into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct HighlightBlendItem(HighlightBlend);

impl SearchableListItem for HighlightBlendItem {
    type Value = HighlightBlend;

    fn title(&self) -> SharedString {
        highlight_blend_label(self.0).into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct NumberFormatItem(NumberFormat);

impl SearchableListItem for NumberFormatItem {
    type Value = NumberFormat;

    fn title(&self) -> SharedString {
        number_format_label(self.0).into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct TextWrapItem(TextWrap);

impl SearchableListItem for TextWrapItem {
    type Value = TextWrap;

    fn title(&self) -> SharedString {
        match self.0 {
            TextWrap::Word => crate::i18n::t!("toolbar.wrap_word"),
            TextWrap::Character => crate::i18n::t!("toolbar.wrap_character"),
            TextWrap::None => crate::i18n::t!("toolbar.wrap_none"),
        }
        .into_owned()
        .into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

pub(super) struct AnnotationPageState {
    swatches: [Entity<ColorPickerState>; PALETTE],
    stroke_width: Entity<SliderState>,
    geometry_radius: Entity<SliderState>,
    arrow_style: Entity<SelectState<SearchableVec<ArrowStyleItem>>>,
    highlighter_width: Entity<SliderState>,
    highlighter_blend: Entity<SelectState<SearchableVec<HighlightBlendItem>>>,
    text_size: Entity<SliderState>,
    text_wrap: Entity<SelectState<SearchableVec<TextWrapItem>>>,
    number_size: Entity<SliderState>,
    number_format: Entity<SelectState<SearchableVec<NumberFormatItem>>>,
    mosaic_strength: Entity<SliderState>,
    eraser_size: Entity<SliderState>,
    spotlight_opacity: Entity<SliderState>,
    magnifier_zoom: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

fn u8_slider(
    min: u8,
    max: u8,
    initial: u8,
    cx: &mut Context<SettingsWindow>,
    subs: &mut Vec<Subscription>,
    apply: impl Fn(&mut Settings, u8) -> bool + 'static,
) -> Entity<SliderState> {
    let min_f = f32::from(min);
    let max_f = f32::from(max);
    let state = cx.new(|_| {
        SliderState::new()
            .max(max_f)
            .min(min_f)
            .step(1.)
            .default_value(f32::from(initial))
    });
    subs.push(cx.subscribe(&state, move |this, _, event, cx| {
        if let SliderEvent::Change(SliderValue::Single(value)) = event {
            let v = value.round().clamp(min_f, max_f) as u8;
            if apply(&mut this.settings, v) {
                this.persist(cx);
            }
        }
    }));
    state
}

impl AnnotationPageState {
    pub(super) fn new(
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        let mut subs = Vec::new();
        let colors = *settings.annotation_colors();
        let swatches = std::array::from_fn(|ix| {
            let value = u32_to_hsla(colors[ix]);
            let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(value));
            subs.push(cx.subscribe(&state, move |this, _, event, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    let rgba = hsla_to_u32(*color);
                    if this.settings.annotation_colors()[ix] != rgba {
                        this.settings.set_swatch_color(ix, rgba);
                        this.persist(cx);
                    }
                }
            }));
            state
        });

        let annot = settings.annotation();

        let stroke_width = u8_slider(1, 20, annot.stroke_width, cx, &mut subs, |s, v| {
            let changed = s.annotation().stroke_width != v;
            s.annotation_mut().stroke_width = v;
            changed
        });
        let geometry_radius = u8_slider(0, 48, annot.geometry_radius, cx, &mut subs, |s, v| {
            let changed = s.annotation().geometry_radius != v;
            s.annotation_mut().geometry_radius = v;
            changed
        });
        let highlighter_width = u8_slider(8, 60, annot.highlighter_width, cx, &mut subs, |s, v| {
            let changed = s.annotation().highlighter_width != v;
            s.annotation_mut().highlighter_width = v;
            changed
        });
        let text_size = u8_slider(12, 72, annot.text_size, cx, &mut subs, |s, v| {
            let changed = s.annotation().text_size != v;
            s.annotation_mut().text_size = v;
            changed
        });
        let number_size = u8_slider(16, 64, annot.number_size, cx, &mut subs, |s, v| {
            let changed = s.annotation().number_size != v;
            s.annotation_mut().number_size = v;
            changed
        });
        let mosaic_strength = u8_slider(4, 48, annot.mosaic_strength, cx, &mut subs, |s, v| {
            let changed = s.annotation().mosaic_strength != v;
            s.annotation_mut().mosaic_strength = v;
            changed
        });
        let eraser_size = u8_slider(8, 96, annot.eraser_size, cx, &mut subs, |s, v| {
            let changed = s.annotation().eraser_size != v;
            s.annotation_mut().eraser_size = v;
            changed
        });
        let spotlight_opacity =
            u8_slider(0, 100, annot.spotlight_opacity, cx, &mut subs, |s, v| {
                let changed = s.annotation().spotlight_opacity != v;
                s.annotation_mut().spotlight_opacity = v;
                changed
            });
        let magnifier_zoom = u8_slider(1, 8, annot.magnifier_zoom, cx, &mut subs, |s, v| {
            let changed = s.annotation().magnifier_zoom != v;
            s.annotation_mut().magnifier_zoom = v;
            changed
        });

        let arrow_current = annot.arrow_style;
        let arrow_style = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ARROW_STYLES.map(ArrowStyleItem).to_vec()),
                ARROW_STYLES
                    .iter()
                    .position(|s| *s == arrow_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&arrow_style, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(style)) = event
                && this.settings.annotation().arrow_style != *style
            {
                this.settings.annotation_mut().arrow_style = *style;
                this.persist(cx);
            }
        }));

        let blend_current = annot.highlighter_blend;
        let highlighter_blend = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(HIGHLIGHT_BLENDS.map(HighlightBlendItem).to_vec()),
                HIGHLIGHT_BLENDS
                    .iter()
                    .position(|b| *b == blend_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&highlighter_blend, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(blend)) = event
                && this.settings.annotation().highlighter_blend != *blend
            {
                this.settings.annotation_mut().highlighter_blend = *blend;
                this.persist(cx);
            }
        }));

        let wrap_current = annot.text_wrap;
        let text_wrap = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(TextWrap::ALL.map(TextWrapItem).to_vec()),
                TextWrap::ALL
                    .iter()
                    .position(|m| *m == wrap_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&text_wrap, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(mode)) = event
                && this.settings.annotation_text_wrap() != *mode
            {
                this.settings.set_annotation_text_wrap(*mode);
                this.persist(cx);
            }
        }));

        let fmt_current = annot.number_format;
        let number_format = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(NUMBER_FORMATS.map(NumberFormatItem).to_vec()),
                NUMBER_FORMATS
                    .iter()
                    .position(|f| *f == fmt_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&number_format, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(fmt)) = event
                && this.settings.annotation().number_format != *fmt
            {
                this.settings.annotation_mut().number_format = *fmt;
                this.persist(cx);
            }
        }));

        Self {
            swatches,
            stroke_width,
            geometry_radius,
            arrow_style,
            highlighter_width,
            highlighter_blend,
            text_size,
            text_wrap,
            number_size,
            number_format,
            mosaic_strength,
            eraser_size,
            spotlight_opacity,
            magnifier_zoom,
            _subscriptions: subs,
        }
    }

    pub(super) fn resync(&self, settings: &Settings, window: &mut Window, cx: &mut App) {
        let colors = settings.annotation_colors();
        for (ix, picker) in self.swatches.iter().enumerate() {
            let color = u32_to_hsla(colors[ix]);
            picker.update(cx, |s, cx| s.set_value(color, window, cx));
        }
        let a = settings.annotation();
        self.stroke_width.update(cx, |s, cx| {
            s.set_value(f32::from(a.stroke_width), window, cx)
        });
        self.geometry_radius.update(cx, |s, cx| {
            s.set_value(f32::from(a.geometry_radius), window, cx)
        });
        self.highlighter_width.update(cx, |s, cx| {
            s.set_value(f32::from(a.highlighter_width), window, cx)
        });
        self.text_size
            .update(cx, |s, cx| s.set_value(f32::from(a.text_size), window, cx));
        self.number_size.update(cx, |s, cx| {
            s.set_value(f32::from(a.number_size), window, cx)
        });
        self.mosaic_strength.update(cx, |s, cx| {
            s.set_value(f32::from(a.mosaic_strength), window, cx)
        });
        self.eraser_size.update(cx, |s, cx| {
            s.set_value(f32::from(a.eraser_size), window, cx)
        });
        self.spotlight_opacity.update(cx, |s, cx| {
            s.set_value(f32::from(a.spotlight_opacity), window, cx)
        });
        self.magnifier_zoom.update(cx, |s, cx| {
            s.set_value(f32::from(a.magnifier_zoom), window, cx)
        });

        let arrow_ix = ARROW_STYLES
            .iter()
            .position(|s| *s == a.arrow_style)
            .map(IndexPath::new);
        self.arrow_style
            .update(cx, |s, cx| s.set_selected_index(arrow_ix, window, cx));
        let blend_ix = HIGHLIGHT_BLENDS
            .iter()
            .position(|b| *b == a.highlighter_blend)
            .map(IndexPath::new);
        self.highlighter_blend
            .update(cx, |s, cx| s.set_selected_index(blend_ix, window, cx));
        let wrap_ix = TextWrap::ALL
            .iter()
            .position(|m| *m == a.text_wrap)
            .map(IndexPath::new);
        self.text_wrap
            .update(cx, |s, cx| s.set_selected_index(wrap_ix, window, cx));
        let fmt_ix = NUMBER_FORMATS
            .iter()
            .position(|f| *f == a.number_format)
            .map(IndexPath::new);
        self.number_format
            .update(cx, |s, cx| s.set_selected_index(fmt_ix, window, cx));
    }
}

/// High-contrast glyph color (`#111` or `#FFF`) over an `0xRRGGBBAA` swatch.
fn ink_over_swatch(rgba_u32: u32) -> Rgba {
    let r = (rgba_u32 >> 24) & 0xFF;
    let g = (rgba_u32 >> 16) & 0xFF;
    let b = (rgba_u32 >> 8) & 0xFF;
    let luma = (77 * r + 150 * g + 29 * b) >> 8;
    if luma > 150 {
        rgba(0x1111_11FF)
    } else {
        rgba(0xFFFF_FFFF)
    }
}

impl SettingsWindow {
    fn reset_annotation_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.reset_annotation_defaults();
        self.annotation_page.resync(&self.settings, window, cx);
        self.persist(cx);
    }

    fn bool_annot_switch(
        &self,
        id: &'static str,
        checked: bool,
        title_key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Settings, bool) + 'static,
    ) -> Switch {
        let entity = cx.entity();
        super::layout::settings_switch(
            id,
            checked,
            crate::i18n::t!(title_key).to_string(),
            window,
            cx,
        )
        .on_change(move |checked, _, _, cx| {
            entity.update(cx, |this, cx| {
                apply(&mut this.settings, checked);
                this.persist(cx);
            });
        })
    }

    fn swatch_index_selector(
        &self,
        prefix: &'static str,
        selected_ix: usize,
        cx: &Context<Self>,
        apply: impl Fn(&mut Settings, usize) + Clone + 'static,
    ) -> Div {
        let colors = self.settings.annotation_colors();
        let mut row = div().flex().items_center().gap_1p5();
        for (ix, &color) in colors.iter().enumerate() {
            let is_selected = ix == selected_ix;
            let entity = cx.entity();
            let apply = apply.clone();
            let label = crate::i18n::t!("settings.annotation.swatch_n", n = ix + 1).to_string();
            let check_color = ink_over_swatch(color);
            row = row.child(
                Button::new(SharedString::from(format!("{prefix}-{ix}")))
                    .accessibility_label(label)
                    .selected(is_selected)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_full()
                    .bg(rgba(color))
                    .border_2()
                    .border_color(if is_selected {
                        cx.theme().foreground
                    } else {
                        cx.theme().foreground.opacity(0.28)
                    })
                    .hover(|s| s.border_color(cx.theme().primary))
                    .focus_visible(|s| s.border_color(cx.theme().ring))
                    .children(
                        is_selected
                            .then(|| Icon::new(IconName::Check).size_3().text_color(check_color)),
                    )
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            apply(&mut this.settings, ix);
                            this.persist(cx);
                        });
                    }),
            );
        }
        row
    }

    pub(super) fn page_annotation(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let annot = self.settings.annotation();

        let auto_select = self.bool_annot_switch(
            "annotation-auto-select",
            self.settings.annotation_auto_select(),
            "settings.general.annotation_auto_select.title",
            window,
            cx,
            |s, v| s.set_annotation_auto_select(v),
        );
        let renumber = self.bool_annot_switch(
            "annotation-renumber-on-delete",
            self.settings.annotation_renumber_on_delete(),
            "settings.general.renumber_on_delete.title",
            window,
            cx,
            |s, v| s.set_annotation_renumber_on_delete(v),
        );
        let smooth_pencil = self.bool_annot_switch(
            "annotation-smooth-pencil",
            annot.smooth_pencil,
            "settings.annotation.smooth_pencil.title",
            window,
            cx,
            |s, v| s.annotation_mut().smooth_pencil = v,
        );
        let geometry_fill = self.bool_annot_switch(
            "annotation-geometry-fill",
            annot.geometry_fill,
            "settings.annotation.geometry_fill.title",
            window,
            cx,
            |s, v| s.annotation_mut().geometry_fill = v,
        );
        let text_bold = self.bool_annot_switch(
            "annotation-text-bold",
            annot.text_bold,
            "settings.annotation.text_bold.title",
            window,
            cx,
            |s, v| s.annotation_mut().text_bold = v,
        );
        let text_outline = self.bool_annot_switch(
            "annotation-text-outline",
            annot.text_outline,
            "settings.annotation.text_outline.title",
            window,
            cx,
            |s, v| s.annotation_mut().text_outline = v,
        );
        let spotlight_ellipse = self.bool_annot_switch(
            "annotation-spotlight-ellipse",
            annot.spotlight_ellipse,
            "settings.annotation.spotlight_ellipse.title",
            window,
            cx,
            |s, v| s.annotation_mut().spotlight_ellipse = v,
        );
        let magnifier_ellipse = self.bool_annot_switch(
            "annotation-magnifier-ellipse",
            annot.magnifier_ellipse,
            "settings.annotation.magnifier_ellipse.title",
            window,
            cx,
            |s, v| s.annotation_mut().magnifier_ellipse = v,
        );
        let magnifier_shadow = self.bool_annot_switch(
            "annotation-magnifier-shadow",
            annot.magnifier_shadow,
            "settings.annotation.magnifier_shadow.title",
            window,
            cx,
            |s, v| s.annotation_mut().magnifier_shadow = v,
        );

        let mut palette_row = div().flex().items_center().gap_1p5();
        for (ix, picker) in self.annotation_page.swatches.iter().enumerate() {
            let label = crate::i18n::t!("settings.annotation.swatch_n", n = ix + 1).to_string();
            palette_row = palette_row.child(
                div()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().foreground.opacity(0.22))
                    .child(ColorPicker::new(picker, label)),
            );
        }

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.preferences.annotation",
                        Some("settings.annotation.blurb"),
                    ))
                    .child(super::shell::reset_button(
                        "settings.annotation.reset",
                        cx,
                        {
                            let entity = cx.entity();
                            move |_, window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.reset_annotation_page(window, cx);
                                });
                            }
                        },
                    )),
            )
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.behavior").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.general.annotation_auto_select.title")
                            .to_string(),
                        Some("settings.general.annotation_auto_select.body"),
                        None,
                        auto_select,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.general.renumber_on_delete.title").to_string(),
                        Some("settings.general.renumber_on_delete.body"),
                        None,
                        renumber,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.smooth_pencil.title").to_string(),
                        Some("settings.annotation.smooth_pencil.body"),
                        None,
                        smooth_pencil,
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.colors").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.palette.title").to_string(),
                        Some("settings.annotation.palette.body"),
                        None,
                        palette_row,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.default_color.title").to_string(),
                        None,
                        None,
                        self.swatch_index_selector(
                            "annot-default-swatch",
                            annot.default_color_index,
                            cx,
                            |s, ix| s.annotation_mut().default_color_index = ix,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.highlighter_color.title").to_string(),
                        None,
                        None,
                        self.swatch_index_selector(
                            "annot-highlighter-swatch",
                            annot.highlighter_color_index,
                            cx,
                            |s, ix| s.annotation_mut().highlighter_color_index = ix,
                        ),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.shapes").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.stroke_width.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.stroke_width,
                            format!("{} px", annot.stroke_width),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.arrow_style.title").to_string(),
                        None,
                        None,
                        Select::new(&self.annotation_page.arrow_style).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.geometry_radius.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.geometry_radius,
                            format!("{} px", annot.geometry_radius),
                            cx,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.geometry_fill.title").to_string(),
                        None,
                        None,
                        geometry_fill,
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.highlighter").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.highlighter_width.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.highlighter_width,
                            format!("{} px", annot.highlighter_width),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.highlighter_blend.title").to_string(),
                        None,
                        None,
                        Select::new(&self.annotation_page.highlighter_blend).w_full(),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.text_badge").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.text_size.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.text_size,
                            format!("{} px", annot.text_size),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.general.text_wrap.title").to_string(),
                        Some("settings.general.text_wrap.body"),
                        None,
                        Select::new(&self.annotation_page.text_wrap).w_full(),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.text_bold.title").to_string(),
                        None,
                        None,
                        text_bold,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.text_outline.title").to_string(),
                        Some("settings.annotation.text_outline.body"),
                        None,
                        text_outline,
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.number_size.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.number_size,
                            format!("{} px", annot.number_size),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.number_format_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.annotation_page.number_format).w_full(),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.annotation.group.effects").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.mosaic_strength.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.mosaic_strength,
                            format!("{} px", annot.mosaic_strength),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.eraser_size.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.eraser_size,
                            format!("{} px", annot.eraser_size),
                            cx,
                        ),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.spotlight_opacity.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.spotlight_opacity,
                            format!("{}%", annot.spotlight_opacity),
                            cx,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.spotlight_ellipse.title").to_string(),
                        None,
                        None,
                        spotlight_ellipse,
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.annotation.magnifier_zoom.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.annotation_page.magnifier_zoom,
                            format!("{}×", annot.magnifier_zoom),
                            cx,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.magnifier_ellipse.title").to_string(),
                        None,
                        None,
                        magnifier_ellipse,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.annotation.magnifier_shadow.title").to_string(),
                        None,
                        None,
                        magnifier_shadow,
                    ),
                ],
            ));

        page_body("page-annotation").child(body)
    }
}
