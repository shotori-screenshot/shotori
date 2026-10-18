//! The Appearance page: the theme.toml editor. Base palette selector,
//! one color picker per [`ColorKey`], the dim-opacity slider — all
//! live-applied to this window's own chrome so the edits preview the
//! next capture's palette, and written through to theme.toml on every
//! change (the write-through contract of [`super::SettingsWindow`],
//! pointed at a different file).

use gpui_kit::base::IndexPath;
use gpui_kit::base::slider::{
    Slider, SliderEvent, SliderIndicator, SliderState, SliderThumb, SliderTrack, SliderValue,
};
use gpui_kit::component::Sizable;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::ui::theme::load::{self, ThemeFile};
use crate::ui::theme::{self, ColorKey};

use super::{
    AppearanceState, BaseItem, CONTENT_MAX, ROW_MIN_H, SettingsWindow, card_group, card_row,
    page_body, page_header, sync_shell_palette,
};

impl SettingsWindow {
    /// Appearance: theme base + per-field colors, live-applied to this
    /// window's own chrome.
    pub(super) fn page_appearance(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .max_w(px(CONTENT_MAX))
            .child(page_header(
                crate::i18n::t!("settings.appearance.title").as_ref(),
                crate::i18n::t!("settings.appearance.blurb").as_ref(),
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.appearance.group.base").as_ref()),
                vec![card_row(
                    crate::i18n::t!("settings.appearance.base_palette").to_string(),
                    None::<String>,
                    div().w_full().child(
                        Select::new(&self.appearance.base_select)
                            .accessibility_label(
                                crate::i18n::t!("settings.appearance.base_palette").to_string(),
                            )
                            .menu_width(px(280.)),
                    ),
                )],
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.appearance.group.dim").as_ref()),
                vec![
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(ROW_MIN_H))
                        .px(px(14.))
                        .py(px(10.))
                        .child(self.opacity_row(cx)),
                ],
            ))
            // The colour grid is the long block, so it is the part that
            // scrolls off the bottom — a clipped slider read as broken.
            .child(card_group(
                Some(crate::i18n::t!("settings.appearance.group.colors").as_ref()),
                vec![div().p(px(12.)).child(self.color_grid(cx))],
            ));
        page_body("page-appearance").child(body)
    }

    /// One row per [`ColorKey`]: label + picker showing the resolved
    /// color. Wraps two per line at the default width.
    fn color_grid(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        let mut grid = div().flex().flex_wrap().gap(px(4.));
        for (key, state) in &self.appearance.pickers {
            // Swatch first, label next, both left-aligned: the picker
            // used to sit 280px away from the name it belongs to, which
            // read as two unrelated columns.
            grid = grid.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .w(px(300.))
                    .py(px(5.))
                    .child(ColorPicker::new(state).small())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(t.btn_text))
                            .child(key.label()),
                    ),
            );
        }
        grid
    }

    /// Dim opacity: 0–1 slider, the value readout as a percentage.
    fn opacity_row(&self, cx: &Context<Self>) -> Div {
        let t = theme::c();
        const TRACK_W: f32 = 180.;
        const THUMB: f32 = 12.;
        let state = &self.appearance.opacity;
        let value = match state.read(cx).value() {
            SliderValue::Single(v) => v,
            SliderValue::Range(s, _) => s,
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(110.))
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.appearance.dim_opacity").to_string()),
            )
            .child(
                div().relative().w(px(TRACK_W)).h(px(18.)).child(
                    Slider::new(state)
                        .horizontal()
                        .absolute()
                        .size_full()
                        .child(
                            SliderTrack::new(state)
                                .absolute()
                                .left(px(THUMB / 2.))
                                .top(px(7.))
                                .w(px(TRACK_W - THUMB))
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgba(t.toolbar_border))
                                // The indicator is the only thing that
                                // records `SliderState::bounds` (see
                                // `SliderIndicator::render`'s on_prepaint).
                                // Without it the state kept a 0x0 rect, so
                                // pointer -> value divided by zero and the
                                // thumb never moved: the slider was dead on
                                // this page while the overlay's copy (which
                                // has this line) worked. It doubles as the
                                // filled portion up to the value.
                                .child(
                                    SliderIndicator::new(state)
                                        .size_full()
                                        .rounded_full()
                                        .bg(rgba(t.accent)),
                                ),
                        )
                        .child(
                            SliderThumb::new(state)
                                .absolute()
                                .top(px(2.))
                                .left(px(value * (TRACK_W - THUMB)))
                                .size(px(THUMB))
                                .rounded_full()
                                .bg(rgba(t.btn_text))
                                .border_1()
                                .border_color(rgba(t.toolbar_bg)),
                        ),
                ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(format!("{:.0}%", theme::c().dim_opacity * 100.)),
            )
    }
}

// ── Appearance: state plumbing ─────────────────────────────────────────────

impl AppearanceState {
    /// Load the working copy (config file or a blank auto base) and
    /// build the interactive rows — every picker seeded with the
    /// resolved color, the slider with the resolved opacity. The
    /// subscriptions route Change events back into the write-through
    /// pipeline; `set_value` (the resync path) emits nothing, so a
    /// base switch cannot loop.
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let path = theme::load::config_path();
        let (file, errors) = load::load_editable(path.as_deref());
        let current = current_palette(&file);

        // Base palette selector: seeded from the file (None = shotori);
        // a Confirm swaps the base, re-seeds the pickers below and
        // writes through (see `appearance_set_base`). The legacy
        // "auto" spelling seeds as "shotori" — the same pair, and
        // "shotori" is what the editor writes back.
        let current_base = match file.base.as_deref() {
            None | Some("shotori") | Some("auto") => "shotori".to_string(),
            Some(base) => base.to_string(),
        };
        let items = BaseItem::all();
        let selected = items
            .iter()
            .position(|item| item.name == current_base)
            .map(IndexPath::new);
        let base_select =
            cx.new(|cx| SelectState::new(SearchableVec::new(items), selected, window, cx));
        let mut subs = Vec::new();
        // subscribe_in (not subscribe): the base swap must re-seed the
        // pickers, and `set_value` needs the window.
        subs.push(
            cx.subscribe_in(&base_select, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(base)) = event {
                    this.appearance_set_base(base, window, cx);
                }
            }),
        );

        let mut pickers = Vec::new();
        for key in ColorKey::ALL {
            let value = u32_to_hsla(key.get(&current));
            let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(value));
            let sub = cx.subscribe(&state, move |this, _, event, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    this.appearance
                        .file
                        .set_color(key, Some(load::to_hex(hsla_to_u32(*color))));
                    this.appearance_apply(cx);
                }
            });
            pickers.push((key, state));
            subs.push(sub);
        }
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(current.dim_opacity)
        });
        subs.push(cx.subscribe(&opacity, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                // The base slider step-snaps AFTER its own clamping —
                // (value / step).round() * step can land just past max
                // (0.05 has no exact binary form; the top of the range
                // yields ~1.0000001), and the loader would reject the
                // file with "dim_opacity must be between 0 and 1" on
                // every drag to the end. Clamp at the boundary and round
                // to the step's two decimals.
                let value = (value.clamp(0., 1.) * 100.).round() / 100.;
                this.appearance.file.dim_opacity = Some(value);
                this.appearance_apply(cx);
            }
        }));
        Self {
            file,
            path,
            error: errors.first().cloned(),
            base_select,
            pickers,
            opacity,
            _subscriptions: subs,
        }
    }
}

impl SettingsWindow {
    /// Apply the working theme live — the settings window's own chrome
    /// re-skins, previewing the next capture's palette — and write
    /// theme.toml through.
    fn appearance_apply(&mut self, cx: &mut Context<Self>) {
        let (dark, light, palette_errors) = self.appearance.file.palettes();
        theme::set(dark, light);
        // appearance edits re-skin the whole shell, not just accent
        sync_shell_palette(cx);
        self.appearance.error = match palette_errors.first() {
            Some(error) => Some(error.clone()),
            None => match &self.appearance.path {
                Some(path) => load::save(path, &self.appearance.file).err(),
                None => Some(crate::i18n::t!("settings.no_config_dir").to_string()),
            },
        };
        cx.notify();
    }

    /// Re-seed every picker and the slider from the resolved palette —
    /// after a base switch or a reset the swatches must show what a
    /// capture would show. `set_value` emits nothing, so no loop.
    fn appearance_resync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = current_palette(&self.appearance.file);
        for (key, state) in &self.appearance.pickers {
            state.update(cx, |state, cx| {
                state.set_value(u32_to_hsla(key.get(&current)), window, cx)
            });
        }
        self.appearance.opacity.update(cx, |state, cx| {
            state.set_value(current.dim_opacity, window, cx)
        });
    }

    fn appearance_set_base(&mut self, base: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance.file.base.as_deref().unwrap_or("shotori") == base {
            return;
        }
        self.appearance.file.base = Some(base.to_string());
        self.appearance_resync(window, cx);
        self.appearance_apply(cx);
    }

    /// Drop every override, keep the chosen base.
    pub(super) fn appearance_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base = self.appearance.file.base.clone();
        self.appearance.file = ThemeFile {
            base,
            ..Default::default()
        };
        self.appearance_resync(window, cx);
        self.appearance_apply(cx);
    }
}

/// The palette a capture would show right now for this editing state
/// (the auto base follows the system appearance, exactly like
/// [`theme::c`]).
fn current_palette(file: &ThemeFile) -> crate::ui::theme::Theme {
    let (dark, light, _) = file.palettes();
    if theme::system_dark() {
        dark
    } else {
        light.unwrap_or(dark)
    }
}

use crate::ui::toolbar::{hsla_to_u32, u32_to_hsla};
