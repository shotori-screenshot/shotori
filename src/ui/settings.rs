//! # Settings window (`shotori settings`)
//!
//! A regular toplevel — the one shotori surface that is NOT a
//! layer-shell overlay: regular `WindowOptions` with a client titlebar,
//! placed by the compositor. The layer-shell rules (display_id pinning,
//! bare `open_window`) do not apply — and unlike the overlays this
//! window CAN host `Root`: its WindowState plugin is what renders
//! managed tooltips (the collapsed sidebar's item labels), so the
//! window wraps its view in one. Root's CSD border decorate is a
//! layer-shell poison (it insets the surface) but harmless under a
//! regular toplevel.
//!
//! Launch: `shotori settings` or the tray menu. The screenshot overlay
//! is a run-and-exit process, so edits cannot reach a live session:
//! every change writes straight through to its config file (see
//! [`crate::model::settings`]) and the NEXT capture picks it up —
//! crash-safe by construction, no OK button, no dirty state.
//!
//! Structure: a keyboard-accessible navigation shell with one page per
//! section (the registry IS the match in `page` — adding a section is
//! an enum variant + a page fn + a sidebar item). The shell's chrome
//! runs on the component design system, synced to the active shotori
//! palette; the pages' preview surfaces (the toolbar editor) render
//! with overlay colors, so what you edit is what the overlay shows.
//!
//! This module is the window shell: the struct, the write-through core
//! (`persist`), the chrome (title bar, sidebar, page dispatcher) and
//! the shared helpers. Each page lives in its own child module —
//! [`toolbar_editor`] (the drag-and-drop layout editor, the biggest
//! piece), [`advanced`], [`keys`], [`ocr`], [`appearance`], [`about`] —
//! the same root + children shape as `ui::overlay`. Child modules
//! reach the private fields directly (descendant privacy); only the
//! `page_*` entry points and the drag-mirror type are `pub(super)`.
//!
//! Preferences are grouped by task: appearance and language, capture and
//! annotation, shortcuts, OCR, and advanced configuration-file access.
//! About remains a reference destination outside the working categories.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::base::AutoScroll;
use gpui_kit::base::Button;
use gpui_kit::base::IndexPath;
use gpui_kit::base::slider::SliderState;
use gpui_kit::component::color_picker::ColorPickerState;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Icon, Root, TITLE_BAR_HEIGHT, TitleBar};
use gpui_kit::*;

use crate::model::settings::{OcrModel, Settings, ToolbarItem};
use crate::ui::e2e;
use crate::ui::theme::load::ThemeFile;
use crate::ui::theme::{self, ColorKey};

use self::toolbar_editor::DragPreview;

gpui_kit::actions!([
    CloseSettings,
    ShowAppearanceSettings,
    ShowCaptureSettings,
    ShowKeySettings,
    ShowOcrSettings,
    ShowAdvancedSettings,
    SearchSettings,
]);

const WINDOW_W: f32 = 1120.;
const WINDOW_H: f32 = 780.;
/// Reading measure. Rows stretch with the window; nothing that has to be
/// READ (prose, paths, a label plus its explanation) goes past this.
const CONTENT_MAX: f32 = 60.;
const SIDEBAR_W: f32 = 14.5;
/// One page of the settings window. The sidebar switches `section`;
/// each page owns the state it needs on [`SettingsWindow`] (today the
/// toolbar editor and the appearance editor each have their own).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Advanced,
    Capture,
    /// The overlay's configurable shortcut commands (the key-cap
    /// recorder; see [`SettingsWindow::page_keys`]).
    Keys,
    Ocr,
    Appearance,
    /// The identity card — a footer entry, not main navigation (the
    /// nav is for working sections; About is reference material).
    About,
}

impl Section {
    /// Sidebar name → section, for the `SHOTORI_DEBUG_SETTINGS_PAGE`
    /// backdoor (docs/headless-e2e.md). Unknown names are ignored so a
    /// stale env var cannot break a normal launch.
    fn from_debug_name(name: &str) -> Option<Self> {
        Some(match name {
            "general" | "advanced" => Self::Advanced,
            "toolbar" | "capture" => Self::Capture,
            "keys" => Self::Keys,
            "ocr" => Self::Ocr,
            "appearance" => Self::Appearance,
            "about" => Self::About,
            _ => return None,
        })
    }
}

/// One row of the language Select. `title` resolves through `t!` on
/// every render, so a live locale flip re-renders the menu (and the
/// trigger) without rebuilding the state.
#[derive(Clone)]
struct LanguageItem(crate::model::settings::Language);

impl SearchableListItem for LanguageItem {
    type Value = crate::model::settings::Language;

    fn title(&self) -> SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

/// One row of the OCR model Select (title = the model's localized
/// label; the langs/tier detail line renders outside the dropdown).
#[derive(Clone)]
struct OcrModelItem(OcrModel);

impl SearchableListItem for OcrModelItem {
    type Value = OcrModel;

    fn title(&self) -> SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

/// Run the settings window; returns when the window closes.
pub fn run() {
    application()
        .with_assets(crate::ui::toolbar::ToolbarSource)
        .run(|cx| {
            cx.set_app_identity(crate::APP_ID, "Shotori");
            gpui_kit::base::init(cx);
            // the component design system (sidebar, its theme registry)
            gpui_kit::component::init(cx);
            cx.bind_keys([
                KeyBinding::new("escape", CloseSettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-1", ShowAppearanceSettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-2", ShowCaptureSettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-3", ShowKeySettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-4", ShowOcrSettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-5", ShowAdvancedSettings, Some("ShotoriSettings")),
                KeyBinding::new("ctrl-f", SearchSettings, Some("ShotoriSettings")),
            ]);
            // Open the window from a spawned task: displays() is empty
            // during synchronous startup (zed#46378) — the same trap
            // the overlay path guards against. The compositor places
            // this toplevel; the first event-loop pass is still the
            // safe place to create it.
            cx.spawn(async move |cx| {
                let opened = cx.update(|cx| {
                    cx.open_window(window_options(), |window, cx| {
                        let view = cx.new(|cx| SettingsWindow::new(window, cx));
                        // Root hosts the WindowState plugin — without it
                        // managed tooltips (collapsed-sidebar item
                        // labels) never render. Layer-shell overlays
                        // must stay bare; a regular toplevel wants this.
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                });
                if let Err(err) = opened {
                    // App already shutting down before the task ran, or the
                    // compositor going away / window limit reached: report
                    // and let the loop end naturally instead of panicking
                    // from a detached task — the process has nothing left
                    // to show.
                    eprintln!("shotori settings: failed to open the settings window: {err}");
                    crate::notify::send(
                        "Shotori",
                        crate::i18n::t!("settings.open_failed").as_ref(),
                    );
                }
            })
            .detach();
        });
}

fn window_options() -> WindowOptions {
    // Initial bounds are a platform boundary; the content below uses rem
    // spacing and reflows independently of this preferred opening size.
    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        // Same title (for the compositor's task list), but the BAR is
        // ours: `title_bar` draws it, so the platform must not stack a
        // system title bar on top (`WindowDecorations::Client`) and must
        // not treat it as a native move region (`app_owns_titlebar_drag`
        // from `TitleBar::window_options`).
        titlebar: Some(TitlebarOptions {
            title: Some(crate::i18n::t!("settings.title").to_string().into()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(WINDOW_W), px(WINDOW_H)),
        })),
        // No min size on purpose: compositors that tile (niri's
        // default) refuse to shrink a window below it, which reads as
        // the window fighting the user. Narrow windows are legitimate —
        // the sidebar collapses to an icon rail and the toolbar preview
        // pans in its own strip.
        is_resizable: true,
        focus: true,
        ..TitleBar::window_options()
    }
}

/// Align the component design system (the settings shell — sidebar,
/// dropdowns, popovers, scrollbars, everything reading `cx.theme()`)
/// with the ACTIVE shotori palette. The palette's inherent appearance
/// picks the mode: every palette is fixed now, so this is purely a
/// palette→chrome mapping (a light base like catppuccin-latte gets
/// light chrome regardless of the system setting). Called by the
/// settings window at startup and on appearance flips, by the overlay
/// (its inline swatch editor's popover needs it too) and after every
/// Appearance edit.
pub(crate) fn sync_shell_theme(cx: &mut App) {
    sync_shell_palette(cx);
}

/// Preview the active palette throughout the editor with opaque surfaces.
fn sync_shell_palette(cx: &mut App) {
    palette::apply(cx, &theme::c());
}

struct SettingsWindow {
    /// Which page is showing (sidebar-driven).
    section: Section,
    search: Entity<InputState>,
    custom_colors: bool,
    /// Working copy of the user's settings; every change writes
    /// through to `path` immediately.
    settings: Settings,
    /// Where settings.toml lives (None when no config dir resolves).
    path: Option<PathBuf>,
    /// The last save's failure (or load warning), shown inline until
    /// the next successful save.
    error: Option<String>,
    config_error_expanded: bool,
    /// The live drag's presentation mirror (carried item, origin,
    /// aimed insertion slot) — see [`DragPreview`]. `None` outside
    /// drags.
    drag: Option<DragPreview>,
    /// The Appearance editor's state (theme.toml working copy + the
    /// picker/slider entities its rows render).
    appearance: AppearanceState,
    /// The language picker's Select state (Appearance page).
    language_select: gpui_kit::Entity<SelectState<SearchableVec<LanguageItem>>>,
    /// The OCR model picker's Select state (OCR page). Selection
    /// commits through the Confirm subscription; the detail line below
    /// renders from `settings` — the file stays the single truth.
    ocr_select: gpui_kit::Entity<SelectState<SearchableVec<OcrModelItem>>>,
    /// Collapsed navigation sidebar (icon-only rail) — persisted in
    /// settings.toml, written through on every toggle.
    sidebar_collapsed: bool,
    /// Transient adaptation; resizing never overwrites the saved sidebar choice.
    compact: bool,
    /// Drives the preview strip's horizontal scroll (see
    /// [`SettingsWindow::preview_strip`]).
    preview_scroll: ScrollHandle,
    /// Each page keeps its own vertical position across navigation.
    page_scroll: [ScrollHandle; 6],
    /// Whether the preview bar overflows its pane right now. The rail's
    /// height is layout state, and layout only changes on a re-render —
    /// which a window resize does not trigger. The strip's prepaint
    /// reports the freshly laid-out flag here and asks for the render
    /// that applies it (`window.defer` + notify — see
    /// [`SettingsWindow::preview_strip`]).
    rail_has_overflow: Option<bool>,
    /// Pointer over the preview rail. The thumb is painted by a
    /// `canvas`, so its hover brightening cannot ride an element
    /// `.hover()` style (a style repaints its own box, not canvas
    /// quads drawn from a closure): the hover listener records the
    /// state and the render re-binds the color. See
    /// [`SettingsWindow::scrollbar_rail`].
    rail_hovered: bool,
    /// Where inside the preview rail's thumb the arming press landed
    /// (px from the thumb's left edge), recorded by the press listener
    /// and held for the whole scrub — the thumb moves WITH the pointer
    /// from that contact point instead of re-centering under it. See
    /// [`SettingsWindow::scrub_rail_to`].
    rail_grab: Option<Pixels>,
    /// The shortcut row currently recording a new keystroke (the
    /// configurable action's registry id), `None` outside recordings.
    recording: Option<String>,
    /// Why the last captured chord was refused (still recording — the
    /// message shows inside the badge until the next attempt).
    record_error: Option<String>,
    /// The recording badge's focus target: the keydown listener lives
    /// on that element, and focusing it routes the chord there (where
    /// stop_propagation keeps the window's own bindings out of the way).
    record_focus: FocusHandle,
    /// Drag-edge auto-scroll for the preview strip — the shared
    /// gpui-base manager (ramped speed, ~60fps ticks, `stop()` retires
    /// it); see [`toolbar_editor::edge_scroll_delta`] for the curve.
    edge_scroll: AutoScroll,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// The Appearance editor: a working [`ThemeFile`] (base + overrides,
/// written through to theme.toml on every change) plus the interactive
/// state each row renders — the base palette selector, one picker
/// entity per [`ColorKey`] and one slider for dim opacity. Picker
/// values are the RESOLVED palette (what a capture would show), so a
/// base switch re-seeds every control through
/// [`SettingsWindow::appearance_resync`]. The annotation swatch board
/// is NOT edited here — it is toolbar configuration, living in
/// settings.toml (see the Toolbar section).
struct AppearanceState {
    file: ThemeFile,
    path: Option<PathBuf>,
    error: Option<String>,
    pickers: Vec<(ColorKey, Entity<ColorPickerState>)>,
    opacity: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        sync_shell_theme(cx);
        if let Some(font) = e2e::debug_settings_font() {
            gpui_kit::component::Theme::global_mut(cx).font_size = px(font);
            gpui_kit::component::Theme::sync_base(cx);
            window.set_rem_size(px(font));
        }
        let (settings, warnings) = Settings::load(crate::model::settings::config_path().as_deref());
        for warning in &warnings {
            eprintln!("[shotori] settings: {warning}");
        }
        let subscriptions = vec![window.observe_window_appearance(|_, cx| {
            // re-align the component design system after a system flip
            // (the shotori palette itself is fixed)
            sync_shell_theme(cx);
            cx.refresh_windows();
        })];
        let compact = window.viewport_size().width < window.rem_size() * 48.;
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(e2e::debug_settings_query())
                .placeholder(crate::i18n::t!("settings.preferences.search").to_string())
        });
        let appearance = AppearanceState::new(window, cx);
        if let Some((key, hsla)) = e2e::debug_settings_color()
            && let Some((_, state)) = appearance
                .pickers
                .iter()
                .find(|(color, _)| color.key() == key)
        {
            state.update(cx, |state, cx| {
                state.set_active_tab(usize::from(hsla), cx);
                state.set_open(true, cx);
            });
        }
        // Language picker: seeds from the loaded settings; a Confirm
        // writes through, flips the process-global locale and
        // refreshes (see `select_language`).
        let language_select = cx.new(|cx| {
            let current = settings.language();
            let selected = crate::model::settings::Language::ALL
                .iter()
                .position(|l| *l == current)
                .map(IndexPath::new);
            SelectState::new(
                SearchableVec::new(
                    crate::model::settings::Language::ALL
                        .map(LanguageItem)
                        .to_vec(),
                ),
                selected,
                window,
                cx,
            )
        });
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.subscribe(&search, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
            let compact = window.viewport_size().width < window.rem_size() * 48.;
            if compact != this.compact {
                this.compact = compact;
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe_in(
            &language_select,
            window,
            |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(language)) = event {
                    this.select_language(*language, window, cx);
                }
            },
        ));
        // OCR model picker: same shape, without the locale flip.
        let ocr_current = settings.ocr_model();
        let ocr_select = cx.new(|cx| {
            let selected = OcrModel::ALL
                .iter()
                .position(|m| *m == ocr_current)
                .map(IndexPath::new);
            SelectState::new(
                SearchableVec::new(OcrModel::ALL.map(OcrModelItem).to_vec()),
                selected,
                window,
                cx,
            )
        });
        subscriptions.push(cx.subscribe(&ocr_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(model)) = event {
                this.select_ocr_model(*model, cx);
            }
        }));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            section: e2e::debug_settings_page()
                .and_then(|page| Section::from_debug_name(&page))
                .unwrap_or(Section::Appearance),
            search,
            custom_colors: e2e::debug_settings_custom_colors()
                || e2e::debug_settings_color().is_some(),
            sidebar_collapsed: settings.sidebar_collapsed(),
            compact,
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            config_error_expanded: false,
            drag: None,
            appearance,
            language_select,
            ocr_select,
            preview_scroll: ScrollHandle::new(),
            page_scroll: std::array::from_fn(|_| ScrollHandle::new()),
            rail_has_overflow: None,
            rail_hovered: false,
            rail_grab: None,
            recording: e2e::debug_settings_recording(),
            record_error: None,
            record_focus: cx.focus_handle(),
            edge_scroll: AutoScroll::default(),
            focus,
            _subscriptions: subscriptions,
        }
    }

    fn navigate_from_key(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            self.select_section(section, cx);
        }
    }

    fn select_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.recording = None;
        self.record_error = None;
        cx.notify();
    }

    /// The logo click: expand/collapse the navigation sidebar
    /// (write-through; the next `shotori settings` restores the rail).
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.settings.set_sidebar_collapsed(self.sidebar_collapsed);
        self.persist(cx);
    }

    /// Install a row and write it through. Rows built here are valid
    /// by construction: click toggles and drop operations use the
    /// model's uniqueness rules. Any row change retires the drag mirror.
    fn commit(&mut self, row: Vec<ToolbarItem>, cx: &mut Context<Self>) {
        self.settings.set_row(row);
        self.drag = None;
        self.persist(cx);
    }

    /// Write-through save: the file is the single source of truth.
    fn persist(&mut self, cx: &mut Context<Self>) {
        self.error = match &self.path {
            Some(path) => self.settings.save(path).err(),
            None => Some(crate::i18n::t!("settings.no_config_dir").to_string()),
        };
        cx.notify();
    }
    /// OCR page: pick a model set and write through. Distinct from the
    /// toolbar commits — it must not retire a toolbar drag (none can be
    /// active on this page anyway).
    fn select_ocr_model(&mut self, model: OcrModel, cx: &mut Context<Self>) {
        if self.settings.ocr_model() == model {
            return;
        }
        self.settings.set_ocr_model(model);
        self.persist(cx);
    }
    /// Pick the UI language. Writes through, flips the
    /// process-global locale (our table and gpui-component's switch
    /// together) and refreshes every window — `t!` resolves on render,
    /// so no restart, no per-element invalidation.
    fn select_language(
        &mut self,
        language: crate::model::settings::Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.language() == language {
            return;
        }
        self.settings.set_language(language);
        self.persist(cx);
        crate::i18n::set(language);
        self.search.update(cx, |search, cx| {
            search.set_placeholder(
                crate::i18n::t!("settings.preferences.search").to_string(),
                window,
                cx,
            );
        });
        cx.refresh_windows();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-root")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .track_focus(&self.focus)
            .key_context("ShotoriSettings")
            // Esc mirrors the titlebar close (the binding is registered
            // in `run`; handled here — app-level backstops never see
            // window-dispatched actions). While the key-cap recorder is
            // listening, Esc belongs to it (cancel) — and the recorder's
            // stop_propagation can't keep THIS action from firing (gpui
            // collects the context bindings before running key
            // listeners, measured), so the guard lives here.
            .on_action(cx.listener(|this, _: &CloseSettings, window, cx| {
                if this.recording.take().is_some() {
                    this.record_error = None;
                    window.blur(cx);
                    cx.notify();
                    return;
                }
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &ShowAppearanceSettings, _, cx| {
                this.navigate_from_key(Section::Appearance, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowCaptureSettings, _, cx| {
                this.navigate_from_key(Section::Capture, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowKeySettings, _, cx| {
                this.navigate_from_key(Section::Keys, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowOcrSettings, _, cx| {
                this.navigate_from_key(Section::Ocr, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAdvancedSettings, _, cx| {
                this.navigate_from_key(Section::Advanced, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchSettings, window, cx| {
                if this.recording.is_some() {
                    return;
                }
                if this.compact || this.sidebar_collapsed {
                    this.sidebar_collapsed = false;
                    this.compact = false;
                }
                this.search.focus_handle(cx).focus(window, cx);
                cx.notify();
            }))
            .child(self.header(window, cx))
            .child(
                // Navigation column and the page share the height below
                // the header; min_h_0 lets the page scroll in a short
                // window instead of overflowing the column. Save errors
                // remain visible above the page while its form scrolls.
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.sidebar(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .children(self.save_error(cx))
                            .child(self.page(window, cx)),
                    ),
            )
            .children(resize_handles(window))
    }
}
/// Hand a file path or URL to the session (xdg-open) — the config
/// editor buttons and the About page's links share this. Reaped on a
/// plain thread so the UI never blocks and no zombie lingers.
/// Failures surface as a notification — the click has no other
/// natural error surface, and a silent no-op reads as broken.
fn open_external(target: String) {
    std::thread::spawn(move || {
        let outcome = std::process::Command::new("xdg-open").arg(&target).status();
        let detail = match outcome {
            Ok(status) if status.success() => return,
            Ok(status) => status.to_string(),
            Err(error) => error.to_string(),
        };
        crate::notify::send(
            crate::i18n::t!("settings.general.open_failed.title").as_ref(),
            crate::i18n::t!("settings.general.open_failed.body", error = detail).as_ref(),
        );
    });
}

/// Decode an embedded PNG into a RenderImage via the img() BGRA path.
fn png_render_image(bytes: &'static [u8]) -> std::sync::Arc<RenderImage> {
    let png = image::load_from_memory(bytes).expect("an embedded PNG asset must decode");
    crate::ui::image_util::rgba_to_render_image(
        png.to_rgba8().into_raw(),
        png.width(),
        png.height(),
    )
}

/// The settings sidebar header's logo: the app icon master (the same
/// 64px asset the tray embeds), decoded once. 64px stays crisp at the
/// header's 16px logical size on HiDPI.
static SIDEBAR_LOGO: std::sync::LazyLock<std::sync::Arc<RenderImage>> =
    std::sync::LazyLock::new(|| {
        png_render_image(include_bytes!("../../assets/app/shotori-64.png"))
    });

mod layout;
mod palette;
mod shell;
mod theme_cards;
use layout::{card_group, card_row, page_body, page_header};
use shell::resize_handles;

mod about;
mod advanced;
mod appearance;
mod color_picker;
mod keys;
mod ocr;
mod toolbar_editor;
