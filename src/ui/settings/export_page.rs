//! Save & Export settings page (issue #42): default save folder, direct
//! one-click save without a dialog, dialog folder memory, filename pattern,
//! and image format / compression options.

use std::path::PathBuf;

use gpui_kit::base::IndexPath;
use gpui_kit::base::Switch;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::model::settings::{ExportFormat, ExportSettings, PngCompression, Settings};

use super::{SettingsWindow, card_group, card_row, open_external, page_body, page_header};

#[derive(Clone)]
pub(super) struct ExportFormatItem(ExportFormat);

impl SearchableListItem for ExportFormatItem {
    type Value = ExportFormat;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone)]
pub(super) struct PngCompressionItem(PngCompression);

impl SearchableListItem for PngCompressionItem {
    type Value = PngCompression;

    fn title(&self) -> SharedString {
        self.0.label().into_owned().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

pub(super) struct ExportPageState {
    pub(super) template_input: Entity<InputState>,
    pub(super) template_error: Option<String>,
    format_select: Entity<SelectState<SearchableVec<ExportFormatItem>>>,
    png_compression_select: Entity<SelectState<SearchableVec<PngCompressionItem>>>,
    jpeg_quality_slider: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl ExportPageState {
    pub(super) fn new(
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        let mut subs = Vec::new();
        let exp = settings.export();

        let initial_template = exp.filename_template.clone();
        let template_input =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_template));
        subs.push(cx.subscribe(&template_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let raw = this.export_page.template_input.read(cx).value().to_string();
                let unchanged = this.settings.export().filename_template == raw.trim();
                match this.settings.export_mut().set_filename_template(&raw) {
                    Ok(()) => {
                        let had_error = this.export_page.template_error.take().is_some();
                        if !unchanged {
                            this.persist(cx);
                        } else if had_error {
                            cx.notify();
                        }
                    }
                    Err(reason) => {
                        this.export_page.template_error = Some(reason.to_owned());
                        cx.notify();
                    }
                }
            }
        }));

        let fmt_current = exp.format;
        let format_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ExportFormat::ALL.map(ExportFormatItem).to_vec()),
                ExportFormat::ALL
                    .iter()
                    .position(|f| *f == fmt_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&format_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(fmt)) = event
                && this.settings.export().format != *fmt
            {
                this.settings.export_mut().format = *fmt;
                this.persist(cx);
            }
        }));

        let comp_current = exp.png_compression;
        let png_compression_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(PngCompression::ALL.map(PngCompressionItem).to_vec()),
                PngCompression::ALL
                    .iter()
                    .position(|c| *c == comp_current)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        subs.push(cx.subscribe(&png_compression_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(comp)) = event
                && this.settings.export().png_compression != *comp
            {
                this.settings.export_mut().png_compression = *comp;
                this.persist(cx);
            }
        }));

        let jpeg_quality_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(100.)
                .step(1.)
                .default_value(f32::from(exp.jpeg_quality))
        });
        subs.push(cx.subscribe(&jpeg_quality_slider, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let q = value.round().clamp(1., 100.) as u8;
                if this.settings.export().jpeg_quality != q {
                    this.settings.export_mut().jpeg_quality = q;
                    this.persist(cx);
                }
            }
        }));

        Self {
            template_input,
            template_error: None,
            format_select,
            png_compression_select,
            jpeg_quality_slider,
            _subscriptions: subs,
        }
    }

    pub(super) fn resync(&mut self, settings: &Settings, window: &mut Window, cx: &mut App) {
        let exp = settings.export();
        self.template_error = None;
        let tpl = exp.filename_template.clone();
        self.template_input
            .update(cx, |s, cx| s.set_value(tpl, window, cx));

        let fmt_ix = ExportFormat::ALL
            .iter()
            .position(|f| *f == exp.format)
            .map(IndexPath::new);
        self.format_select
            .update(cx, |s, cx| s.set_selected_index(fmt_ix, window, cx));

        let comp_ix = PngCompression::ALL
            .iter()
            .position(|c| *c == exp.png_compression)
            .map(IndexPath::new);
        self.png_compression_select
            .update(cx, |s, cx| s.set_selected_index(comp_ix, window, cx));

        self.jpeg_quality_slider.update(cx, |s, cx| {
            s.set_value(f32::from(exp.jpeg_quality), window, cx)
        });
    }
}

/// Spawn `rfd::FileDialog::pick_folder` on a dedicated OS thread: portal round
/// trips block, and GPUI's executors are async contexts that must never block.
fn pick_save_folder(entity: Entity<SettingsWindow>, start_dir: Option<PathBuf>, cx: &mut App) {
    let mut dialog = rfd::FileDialog::new()
        .set_title(crate::i18n::t!("settings.export.save_dir.pick_title").as_ref());
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    let (tx, rx) = async_channel::bounded::<Option<PathBuf>>(1);
    std::thread::spawn(move || {
        let chosen = dialog.pick_folder();
        drop(tx.send_blocking(chosen));
    });
    cx.spawn(async move |cx| {
        if let Ok(Some(folder)) = rx.recv().await {
            entity.update(cx, |this, cx| {
                this.settings
                    .export_mut()
                    .set_save_dir(Some(folder.display().to_string()));
                this.persist(cx);
            });
        }
    })
    .detach();
}

impl SettingsWindow {
    fn reset_export_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        *self.settings.export_mut() = ExportSettings::default();
        self.export_page.resync(&self.settings, window, cx);
        self.persist(cx);
    }

    fn bool_export_switch(
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

    pub(super) fn page_export(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let exp = self.settings.export();
        let resolved_dir = exp.resolved_save_dir().ok();
        let has_custom_dir = exp.save_dir.is_some();

        let mut dir_actions = div().flex().items_center().gap_1().flex_shrink_0();
        {
            let resolved_for_open = exp.resolved_save_dir();
            let resolved_open_str = resolved_for_open
                .as_ref()
                .ok()
                .map(|p| p.display().to_string());
            let resolved_open_err = resolved_for_open.err().map(|e| format!("{e:#}"));
            dir_actions = dir_actions.child(super::layout::action_button(
                cx,
                "export-save-dir-open",
                crate::i18n::t!("settings.general.file.open").to_string(),
                move |_, _, _| match (&resolved_open_str, &resolved_open_err) {
                    (Some(dir_str), _) => {
                        if let Err(err) = std::fs::create_dir_all(dir_str) {
                            crate::notify::send(
                                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                                crate::i18n::t!(
                                    "settings.general.open_failed.body",
                                    error = err.to_string()
                                )
                                .as_ref(),
                            );
                        } else {
                            open_external(dir_str.clone());
                        }
                    }
                    (_, Some(err)) => crate::notify::send(
                        crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                        crate::i18n::t!("settings.general.open_failed.body", error = err.as_str())
                            .as_ref(),
                    ),
                    _ => {}
                },
            ));
        }
        {
            let entity = cx.entity();
            let start_dir = resolved_dir.filter(|p| p.exists());
            dir_actions = dir_actions.child(super::layout::action_button(
                cx,
                "export-save-dir-browse",
                crate::i18n::t!("settings.export.save_dir.browse").to_string(),
                move |_, _, cx| pick_save_folder(entity.clone(), start_dir.clone(), cx),
            ));
        }
        if has_custom_dir {
            let entity = cx.entity();
            dir_actions = dir_actions.child(super::layout::action_button(
                cx,
                "export-save-dir-reset",
                crate::i18n::t!("settings.export.save_dir.reset").to_string(),
                move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.settings.export_mut().set_save_dir(None);
                        this.persist(cx);
                    });
                },
            ));
        }

        let auto_save = self.bool_export_switch(
            "export-auto-save",
            exp.auto_save,
            "settings.export.auto_save.title",
            window,
            cx,
            |s, v| s.export_mut().auto_save = v,
        );
        let remember_last_dir = self.bool_export_switch(
            "export-remember-last-dir",
            exp.remember_last_dir,
            "settings.export.remember_last_dir.title",
            window,
            cx,
            |s, v| s.export_mut().remember_last_dir = v,
        );
        let copy_on_save = self.bool_export_switch(
            "export-copy-on-save",
            exp.copy_on_save,
            "settings.export.copy_on_save.title",
            window,
            cx,
            |s, v| s.export_mut().copy_on_save = v,
        );

        let template_desc = match &self.export_page.template_error {
            Some(reason) => crate::i18n::t!(
                "settings.export.filename_template.invalid",
                reason = reason.as_str()
            )
            .to_string(),
            None => {
                let preview = crate::model::export::suggested_filename_for(
                    exp,
                    chrono::Local::now(),
                    1920,
                    1080,
                );
                crate::i18n::t!(
                    "settings.export.filename_template.preview",
                    name = preview.as_str()
                )
                .to_string()
            }
        };

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.preferences.export",
                        Some("settings.export.blurb"),
                    ))
                    .child(super::shell::reset_button("settings.export.reset", cx, {
                        let entity = cx.entity();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.reset_export_page(window, cx);
                            });
                        }
                    })),
            )
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.export.group.location").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.export.save_dir.title").to_string(),
                        None,
                        Some(exp.display_save_dir().to_owned()),
                        dir_actions,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.export.auto_save.title").to_string(),
                        Some("settings.export.auto_save.body"),
                        None,
                        auto_save,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.export.remember_last_dir.title").to_string(),
                        Some("settings.export.remember_last_dir.body"),
                        None,
                        remember_last_dir,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.export.copy_on_save.title").to_string(),
                        Some("settings.export.copy_on_save.body"),
                        None,
                        copy_on_save,
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.export.group.naming").as_ref()),
                vec![card_row(
                    cx,
                    crate::i18n::t!("settings.export.filename_template.title").to_string(),
                    Some("settings.export.filename_template.hint"),
                    Some(template_desc),
                    Input::new(&self.export_page.template_input).w_full(),
                )],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.export.group.encoding").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.export.format_row.title").to_string(),
                        Some("settings.export.format_row.body"),
                        None,
                        Select::new(&self.export_page.format_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.export.png_compression_row.title").to_string(),
                        None,
                        None,
                        Select::new(&self.export_page.png_compression_select).w_full(),
                    ),
                    card_row(
                        cx,
                        crate::i18n::t!("settings.export.jpeg_quality.title").to_string(),
                        None,
                        None,
                        super::layout::slider_control(
                            &self.export_page.jpeg_quality_slider,
                            format!("{}%", exp.jpeg_quality),
                            cx,
                        ),
                    ),
                ],
            ));

        page_body("page-export").child(body)
    }
}
