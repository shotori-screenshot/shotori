//! # Settings window (`shotori settings`)
//!
//! A regular toplevel — the one shotori surface that is NOT a
//! layer-shell overlay: plain `WindowOptions` (system titlebar, the
//! compositor places it). The layer-shell rules (display_id pinning,
//! bare `open_window`) do not apply — and unlike the overlays this
//! window CAN host `Root`: its WindowState plugin is what renders
//! managed tooltips (the collapsed sidebar's item labels), so the
//! window wraps its view in one. Root's CSD border decorate is a
//! layer-shell poison (it insets the surface) but harmless under a
//! system-decorated toplevel.
//!
//! Launch: `shotori settings` or the tray menu. The screenshot overlay
//! is a run-and-exit process, so edits cannot reach a live session:
//! every change writes straight through to its config file (see
//! [`crate::model::settings`]) and the NEXT capture picks it up —
//! crash-safe by construction, no OK button, no dirty state.
//!
//! Structure: a gpui-component [`Sidebar`] shell with one page per
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
//! piece), [`general`], [`keys`], [`ocr`], [`appearance`], [`about`] —
//! the same root + children shape as `ui::overlay`. Child modules
//! reach the private fields directly (descendant privacy); only the
//! `page_*` entry points and the drag-mirror type are `pub(super)`.
//!
//! v1 sections: General (installation facts + the UI language picker),
//! the toolbar layout (drag-and-drop), OCR (model set + speed toggle)
//! and Appearance (theme editor). UI strings resolve through
//! [`crate::i18n`] — the language picker flips the process-global
//! locale and refreshes, so the whole window (including
//! gpui-component's own texts) re-renders translated.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::base::AutoScroll;
use gpui_kit::base::Button;
use gpui_kit::base::IndexPath;
use gpui_kit::base::slider::SliderState;
use gpui_kit::component::color_picker::ColorPickerState;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::sidebar::SidebarToggleButton;
use gpui_kit::component::{ActiveTheme, Icon, Root, TITLE_BAR_HEIGHT, Theme, ThemeMode, TitleBar};
use gpui_kit::*;

use crate::model::settings::{OcrModel, Settings, ToolbarItem};
use crate::ui::e2e;
use crate::ui::theme::load::{self, ThemeFile};
use crate::ui::theme::{self, ColorKey};

use self::toolbar_editor::DragPreview;

gpui_kit::actions!([CloseSettings]);

const WINDOW_W: f32 = 1024.;
const WINDOW_H: f32 = 640.;
/// Page gutter. The content column is capped separately, so this carries
/// breathing room instead of line length.
const PAGE_PAD: f32 = 28.;
/// Reading measure. Rows stretch with the window; nothing that has to be
/// READ (prose, paths, a label plus its explanation) goes past this.
const CONTENT_MAX: f32 = 640.;
/// Group-card anatomy: one radius, one row height, one control column —
/// the reason the pages read as one widget family instead of four.
const CARD_RADIUS: f32 = 10.;
const ROW_MIN_H: f32 = 52.;
const CONTROL_W: f32 = 200.;
const SIDEBAR_W: f32 = 232.;
/// Collapsed navigation rail: wide enough for the 16px icons to sit
/// centered, narrow enough to stay out of the way on a tiled window.
const RAIL_W: f32 = 56.;
const NAV_ROW_H: f32 = 34.;
/// Status / reset strip under the page, shared by every section.
const FOOTER_H: f32 = 36.;

/// One page of the settings window. The sidebar switches `section`;
/// each page owns the state it needs on [`SettingsWindow`] (today the
/// toolbar editor and the appearance editor each have their own).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    General,
    Toolbar,
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
            "general" => Self::General,
            "toolbar" => Self::Toolbar,
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

/// One row of the base palette Select: the shotori default (the orange
/// dark/light pair following the system — the product's own theme)
/// plus every preinstalled community palette (see
/// [`theme::load::BUILT_INS`]). All labels are proper nouns resolved
/// per render, so a live locale flip relabels the menu like everywhere
/// else.
#[derive(Clone)]
struct BaseItem {
    /// The `theme.toml` value: "shotori" or a built-in name.
    name: String,
    display: SharedString,
}

impl BaseItem {
    /// The shotori default first, then every community theme in
    /// registry order.
    fn all() -> Vec<Self> {
        std::iter::once(Self {
            name: "shotori".into(),
            display: "Shotori".into(),
        })
        .chain(load::BUILT_INS.iter().map(|built_in| Self {
            name: built_in.name.into(),
            display: built_in.display.into(),
        }))
        .collect()
    }
}

impl SearchableListItem for BaseItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.display.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.name
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
            cx.bind_keys([KeyBinding::new(
                "escape",
                CloseSettings,
                Some("ShotoriSettings"),
            )]);
            // Open the window from a spawned task: displays() is empty
            // during synchronous startup (zed#46378) — the same trap
            // the overlay path guards against. The compositor places
            // this toplevel; the first event-loop pass is still the
            // safe place to create it.
            cx.spawn(async move |cx| {
                cx.update(|cx| {
                    cx.open_window(window_options(), |window, cx| {
                        let view = cx.new(|cx| SettingsWindow::new(window, cx));
                        // Root hosts the WindowState plugin — without it
                        // managed tooltips (collapsed-sidebar item
                        // labels) never render. Layer-shell overlays
                        // must stay bare; a regular toplevel wants this.
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                    .expect("failed to open the settings window");
                });
            })
            .detach();
        });
}

fn window_options() -> WindowOptions {
    // The sidebar (255px) plus content wide enough for the factory row
    // (~703px) to preview without squeezing; the palette below wraps,
    // so height is forgiving.
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
        focus: true,
        ..TitleBar::window_options()
    }
}

/// Align the component design system (the settings shell — sidebar,
/// dropdowns, popovers, scrollbars, everything reading `cx.theme()`)
/// with the ACTIVE shotori palette. The palette's inherent appearance
/// picks the mode: `auto` already follows the system through
/// [`theme::follow_system`], and a fixed base (say catppuccin-latte)
/// must not inherit dark chrome from a dark system. Called by the
/// settings window at startup and on appearance flips, by the overlay
/// (its inline swatch editor's popover needs it too) and after every
/// Appearance edit.
pub(crate) fn sync_shell_theme(window: &mut Window, cx: &mut App) {
    theme::follow_system(window.appearance());
    sync_shell_palette(cx);
}

/// Paint the shotori palette onto the component theme's tokens — the
/// library ships only "Default Light/Dark", so without this the shell
/// stays gray while the pages re-skin. Surface roles map directly
/// (toolbar_* IS our chrome vocabulary); the shadcn-style semantic
/// roles (primary/accent/muted/…) take the closest palette field.
/// Status colors (danger/success/warning/info) stay library defaults:
/// the shotori palette has no equivalents.
///
/// `Theme::change` reloads the registered defaults and would wipe this
/// paint, so it runs only when the mode actually flips — same-mode
/// edits (picker drags fire per pixel) repaint in place.
fn sync_shell_palette(cx: &mut App) {
    let t = theme::c();
    let mode = if t.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
    // the palette's chrome vocabulary, converted once
    let bg: Hsla = rgba(t.toolbar_bg).into();
    let hover: Hsla = rgba(t.toolbar_hover).into();
    let selected: Hsla = rgba(t.toolbar_selected).into();
    let text: Hsla = rgba(t.toolbar_text).into();
    let hint: Hsla = rgba(t.hint_text).into();
    let border: Hsla = rgba(t.toolbar_border).into();
    let chip: Hsla = rgba(t.chip_bg).into();
    let accent: Hsla = rgba(t.accent).into();
    let accent_text: Hsla = rgba(t.accent_text()).into();
    let btn_text: Hsla = rgba(t.btn_text).into();
    let btn_hover: Hsla = rgba(t.btn_hover_bg).into();
    Theme::update(cx, |theme| {
        let c = &mut theme.colors;
        // surfaces & chrome
        c.background = bg;
        c.foreground = text;
        c.border = border;
        c.title_bar = bg;
        c.title_bar_border = border;
        c.status_bar = bg;
        c.status_bar_border = border;
        // the kit's own derivation (theme/schema.rs apply_config):
        // background blended with a 15% border tint — a distinct panel,
        // not a second coat of the page color
        c.sidebar = bg.blend(border.opacity(0.15));
        c.sidebar_foreground = text;
        c.sidebar_border = border;
        c.sidebar_accent = selected;
        c.sidebar_accent_foreground = text;
        c.sidebar_primary = accent;
        c.sidebar_primary_foreground = accent_text;
        c.popover = chip;
        c.popover_foreground = text;
        c.group_box = bg;
        c.group_box_foreground = text;
        c.accordion = bg;
        // lists, tables, tabs
        c.list = bg;
        c.list_hover = hover;
        c.list_active = selected;
        c.list_active_border = border;
        c.list_head = bg;
        c.list_even = bg;
        c.table = bg;
        c.table_hover = hover;
        c.table_active = selected;
        c.table_active_border = border;
        c.table_even = bg;
        c.table_head = hover;
        c.table_head_foreground = text;
        c.table_foot = bg;
        c.table_foot_foreground = text;
        c.table_row_border = border;
        c.tab = bg;
        c.tab_active = selected;
        c.tab_active_foreground = text;
        c.tab_bar = bg;
        c.tab_bar_segmented = hover;
        c.tab_foreground = hint;
        // the shadcn-style semantic roles
        c.accent = hover; // shadcn "accent" = menu/list hover, NOT brand
        c.accent_foreground = text;
        c.muted = hover;
        c.muted_foreground = hint;
        c.secondary = hover;
        c.secondary_foreground = text;
        c.secondary_hover = selected;
        c.secondary_active = selected;
        c.primary = accent; // the brand color (Switch, links, focus)
        c.primary_foreground = accent_text;
        c.primary_hover = accent;
        c.primary_active = accent;
        // inputs & controls
        c.input = border; // shadcn "input" = the input border color
        c.ring = accent;
        c.caret = text;
        c.selection = selected;
        c.link = accent;
        c.link_hover = accent;
        c.link_active = accent;
        c.button = bg;
        c.button_foreground = btn_text;
        c.button_hover = btn_hover;
        c.button_active = selected;
        c.switch = hover;
        c.switch_thumb = text;
        c.slider_bar = border;
        c.slider_thumb = text;
        c.scrollbar = bg;
        // the kit's own fallbacks: the thumb IS the accent color
        c.scrollbar_thumb = accent;
        c.scrollbar_thumb_hover = accent;
        c.drop_target = selected;
        c.drag_border = accent;
        c.skeleton = hover;
        c.description_list_label = hint;
        c.description_list_label_foreground = text;
    });
}

struct SettingsWindow {
    /// Which page is showing (sidebar-driven).
    section: Section,
    /// Working copy of the user's settings; every change writes
    /// through to `path` immediately.
    settings: Settings,
    /// Where settings.toml lives (None when no config dir resolves).
    path: Option<PathBuf>,
    /// The last save's failure (or load warning), shown inline until
    /// the next successful save.
    error: Option<String>,
    /// The live drag's presentation mirror (carried item, origin,
    /// aimed insertion slot) — see [`DragPreview`]. `None` outside
    /// drags.
    drag: Option<DragPreview>,
    /// The Appearance editor's state (theme.toml working copy + the
    /// picker/slider entities its rows render).
    appearance: AppearanceState,
    /// The language picker's Select state (General page).
    language_select: gpui_kit::Entity<SelectState<SearchableVec<LanguageItem>>>,
    /// The OCR model picker's Select state (OCR page). Selection
    /// commits through the Confirm subscription; the detail line below
    /// renders from `settings` — the file stays the single truth.
    ocr_select: gpui_kit::Entity<SelectState<SearchableVec<OcrModelItem>>>,
    /// Collapsed navigation sidebar (icon-only rail) — persisted in
    /// settings.toml, written through on every toggle.
    sidebar_collapsed: bool,
    /// Drives the preview strip's horizontal scroll (see
    /// [`SettingsWindow::preview_strip`]).
    preview_scroll: ScrollHandle,
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
    /// it); see [`edge_scroll_delta`] for the curve.
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
    /// The base palette selector (auto + the preinstalled themes).
    base_select: gpui_kit::Entity<SelectState<SearchableVec<BaseItem>>>,
    pickers: Vec<(ColorKey, Entity<ColorPickerState>)>,
    opacity: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        sync_shell_theme(window, cx);
        let (settings, warnings) = Settings::load(crate::model::settings::config_path().as_deref());
        for warning in &warnings {
            eprintln!("[shotori] settings: {warning}");
        }
        let subscriptions = vec![window.observe_window_appearance(|window, cx| {
            sync_shell_theme(window, cx);
            cx.refresh_windows();
        })];
        let appearance = AppearanceState::new(window, cx);
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
        subscriptions.push(cx.subscribe(&language_select, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(language)) = event {
                this.select_language(*language, cx);
            }
        }));
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
        Self {
            section: e2e::debug_settings_page()
                .and_then(|page| Section::from_debug_name(&page))
                .unwrap_or(Section::Toolbar),
            sidebar_collapsed: settings.sidebar_collapsed(),
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            drag: None,
            appearance,
            language_select,
            ocr_select,
            preview_scroll: ScrollHandle::new(),
            rail_has_overflow: None,
            rail_hovered: false,
            rail_grab: None,
            recording: None,
            record_error: None,
            record_focus: cx.focus_handle(),
            edge_scroll: AutoScroll::default(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The logo click: expand/collapse the navigation sidebar
    /// (write-through; the next `shotori settings` restores the rail).
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.settings.set_sidebar_collapsed(self.sidebar_collapsed);
        self.persist(cx);
    }

    /// Install a row and write it through. Rows built here are valid
    /// by construction (buttons can only come from the palette, which
    /// never offers one that is already placed). Any row change
    /// retires the drag mirror.
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
    /// General page: pick the UI language. Writes through, flips the
    /// process-global locale (our table and gpui-component's switch
    /// together) and refreshes every window — `t!` resolves on render,
    /// so no restart, no per-element invalidation.
    fn select_language(
        &mut self,
        language: crate::model::settings::Language,
        cx: &mut Context<Self>,
    ) {
        if self.settings.language() == language {
            return;
        }
        self.settings.set_language(language);
        self.persist(cx);
        crate::i18n::set(language);
        cx.refresh_windows();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::c();
        div()
            .id("settings-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(t.toolbar_bg))
            .text_color(rgba(t.toolbar_text))
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
            .child(self.header(window, cx))
            .child(
                // Navigation column and the page share the height below
                // the header; min_h_0 lets the page scroll in a short
                // window instead of overflowing the column. The footer
                // is INSIDE the content column so the navigation keeps
                // its full height and the reset action sits under the
                // page it resets.
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.page(cx))
                            .child(self.status_bar(window, cx)),
                    ),
            )
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

/// One page's heading: a 20px title with a short blurb under it. Every
/// page opens the same way — the old 16px title next to a same-size label
/// was indistinguishable from the row labels below it.
fn page_header(title: &str, blurb: &str) -> Div {
    let t = theme::c();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pb(px(18.))
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba(t.toolbar_text))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgba(t.hint_text))
                .child(blurb.to_string()),
        )
}

/// Re-tint a palette token. Theme colours are `0xRRGGBBAA` (alpha in the
/// LOW byte — see `--print-theme`), so the wash keeps the RGB and swaps
/// the alpha byte: hairlines and card backgrounds are shades of the
/// palette that already exists, not new palette entries.
fn alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xffff_ff00) | u32::from(alpha)
}

/// A grouped card: an optional caption plus rows separated by hairlines.
/// Design direction A — every page is a stack of these, so the same
/// "label, explanation, control on the right" anatomy appears on all six
/// pages instead of four different container idioms.
fn card_group(caption: Option<&str>, rows: impl IntoIterator<Item = impl IntoElement>) -> Div {
    let t = theme::c();
    let mut card = div()
        .flex()
        .flex_col()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(rgba(alpha(t.toolbar_border, 0x59)))
        .bg(rgba(alpha(t.toolbar_text, 0x0c)));
    // No `overflow_hidden` here: it turns the card into a clipping
    // (scrollable) container, which focusable/hit-testable children —
    // the toolbar editor's drag targets, the opacity slider — have to
    // survive. Nothing in a card needs corner clipping: rows carry no
    // background and the hairlines are inset.
    let rows: Vec<_> = rows.into_iter().collect();
    let last = rows.len().saturating_sub(1);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 && index <= last {
            card = card.child(
                div()
                    .h(px(1.))
                    .mx(px(14.))
                    .bg(rgba(alpha(t.toolbar_border, 0x40))),
            );
        }
        card = card.child(row);
    }
    let mut group = div().flex().flex_col().gap(px(8.));
    if let Some(caption) = caption {
        group = group.child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba(t.hint_text))
                .child(caption.to_string()),
        );
    }
    group.child(card)
}

/// One row of a group card: the label (with an optional explanation under
/// it) on the left, a fixed-width control column on the right. The fixed
/// column is what stops three Selects from ending at three different x.
fn card_row(
    label: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
    control: impl IntoElement,
) -> Div {
    let t = theme::c();
    let label = label.into();
    let mut left = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .flex_1()
        .min_w(px(0.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgba(t.btn_text))
                .child(label),
        );
    if let Some(description) = description {
        left = left.child(
            div()
                .text_size(px(11.))
                .text_color(rgba(t.hint_text))
                .child(description.into()),
        );
    }
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .min_h(px(ROW_MIN_H))
        .px(px(14.))
        .py(px(10.))
        .child(left)
        .child(
            div()
                .flex()
                .justify_end()
                .items_center()
                .flex_shrink_0()
                .w(px(CONTROL_W))
                .child(control),
        )
}

/// A card that hosts a custom block instead of rows (the toolbar editor,
/// the colour grid): same frame, inset padding. Wired up as those two
/// pages move onto the group system.
#[allow(dead_code)]
fn card_block(child: impl IntoElement) -> Div {
    let t = theme::c();
    div()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(rgba(alpha(t.toolbar_border, 0x59)))
        .bg(rgba(alpha(t.toolbar_text, 0x0c)))
        .p(px(14.))
        .child(child)
}

/// The scrolling column every page renders into: gutter, capped measure.
fn page_body(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(18.))
        .p(px(PAGE_PAD))
        .size_full()
        .min_w(px(0.))
        .overflow_y_scroll()
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
/// One title-bar control (34px wide, full height), styled like the
/// component's cluster: the quiet secondary hover for minimize /
/// maximize, the theme's danger tint for close.
fn window_control(
    id: &'static str,
    icon: IconName,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let hover_bg = if danger {
        theme.danger
    } else {
        theme.secondary_hover
    };
    let hover_fg = if danger {
        theme.danger_foreground
    } else {
        theme.secondary_foreground
    };
    div()
        .id(id)
        .flex()
        .w(TITLE_BAR_HEIGHT)
        .h_full()
        .flex_shrink_0()
        .justify_center()
        .items_center()
        .text_color(theme.foreground)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        // Keep the bar's drag handler from treating a control press as
        // the start of a window move.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(Icon::new(icon).size_4())
}

/// The footer's page-reset action. One button, one place, every page —
/// it used to exist only inside two pages, at two different heights.
fn reset_button(on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Button {
    let t = theme::c();
    Button::new("footer-reset")
        .accessibility_label(crate::i18n::t!("settings.footer.reset_page").to_string())
        .px(px(10.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(rgba(t.btn_text))
        .border_1()
        .border_color(rgba(t.toolbar_border))
        .hover(|s| s.bg(rgba(t.toolbar_hover)))
        .focus_visible(|s| s.border_color(rgba(t.accent)))
        .on_click(on_click)
        .child(crate::i18n::t!("settings.footer.reset_page").to_string())
}

/// The title bar's control cluster for a server-decorated window that
/// has no system bar (see [`SettingsWindow::title_bar`]). Only the
/// controls the compositor advertises through `wm_capabilities` are
/// offered; close is always ours.
fn window_controls(window: &Window, cx: &App) -> impl IntoElement {
    let supported = window.window_controls();
    let mut controls = div()
        .id("title-bar-controls")
        .flex()
        .items_center()
        .h_full()
        .flex_shrink_0();
    if supported.minimize {
        controls = controls.child(window_control(
            "title-bar-minimize",
            IconName::WindowMinimize,
            false,
            |_, window, _| window.minimize_window(),
            cx,
        ));
    }
    if supported.maximize {
        let icon = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };
        controls = controls.child(window_control(
            "title-bar-maximize",
            icon,
            false,
            |_, window, _| window.zoom_window(),
            cx,
        ));
    }
    controls.child(window_control(
        "title-bar-close",
        IconName::WindowClose,
        true,
        |_, window, _| window.remove_window(),
        cx,
    ))
}

impl SettingsWindow {
    /// The self-drawn title bar: brand mark plus window title on the
    /// left, the window controls on the right. The app owns it because
    /// the window asks for client-side decorations; the branding lives
    /// here (not in the sidebar header) so it never moves or shrinks
    /// when the navigation rail collapses. `TitleBar` supplies the drag
    /// / double-click-zoom behavior and the component's control cluster
    /// wherever the compositor honors client decorations.
    fn header(&self, window: &Window, cx: &mut Context<Self>) -> TitleBar {
        let brand = div()
            .flex()
            .items_center()
            .gap(px(8.))
            // The app icon master (the same 64px asset the tray
            // embeds), decoded once — 64px stays crisp at the title
            // bar's 16px logical size on HiDPI.
            .child(img(ImageSource::Render(SIDEBAR_LOGO.clone())).size_4())
            .child(
                div()
                    .text_size(px(12.))
                    .child(crate::i18n::t!("settings.title").to_string()),
            );
        // The rail toggle lives here, next to the brand, instead of in
        // the navigation column: the logo used to double as the toggle,
        // which read as "click the app icon" rather than "collapse the
        // sidebar".
        let toggle = SidebarToggleButton::new()
            .collapsed(self.sidebar_collapsed)
            .accessibility_label(
                if self.sidebar_collapsed {
                    crate::i18n::t!("settings.nav.expand")
                } else {
                    crate::i18n::t!("settings.nav.collapse")
                }
                .to_string(),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_sidebar(cx)));
        let leading = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(brand)
            .child(toggle);
        // `TitleBar` hides its own control cluster when the compositor
        // claims server-side decorations — it must not duplicate a
        // system title bar. But some compositors claim that while
        // drawing nothing (niri with `prefer-no-csd`), so OUR bar is the
        // only chrome the window ever gets and has to carry the
        // controls itself. In client-decorated mode the component's
        // cluster handles it (outside the drag region, same as here).
        let own_controls = matches!(window.window_decorations(), Decorations::Server)
            .then(|| window_controls(window, cx));
        TitleBar::new().child(leading).children(own_controls)
    }

    /// One navigation entry. Hand-built rather than `SidebarMenuItem`:
    /// the component's row metrics (a 44px pitch, a highlight running
    /// flush into the divider, icons only on the collapsed rail) are what
    /// made the column read as a list of links instead of navigation.
    fn nav_item(
        &self,
        key: &'static str,
        icon: IconName,
        target: Section,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme::c();
        let active = self.section == target;
        let label = crate::i18n::t!(key).to_string();
        let fill = if active {
            // The accent wash marks the current section without turning
            // the row into a loud primary button.
            rgba(alpha(t.accent, 0x2b))
        } else {
            rgba(0x00000000)
        };
        let item = div()
            .id(SharedString::from(format!("nav-{key}")))
            .flex()
            .items_center()
            .h(px(NAV_ROW_H))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .bg(fill)
            .text_color(rgba(if active { t.toolbar_text } else { t.btn_text }))
            .aria_label(label.clone())
            .on_click(cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                if this.section != target {
                    this.section = target;
                    cx.notify();
                }
            }))
            .child(Icon::new(icon).size_4().text_color(rgba(if active {
                t.accent
            } else {
                t.hint_text
            })));
        // Hover tints only inactive rows. `.hover` REPLACES the
        // background, so leaving it on the active row wiped its accent
        // wash: the pointer over the current section made it read as
        // unselected. The pointer cursor still advertises clickability.
        let item = if active {
            item
        } else {
            item.hover(|this| this.bg(rgba(alpha(t.toolbar_hover, 0x66))))
        };
        if collapsed {
            item.justify_center()
        } else {
            item.gap(px(10.)).px(px(10.)).child(label)
        }
    }

    /// The navigation column: the working sections, a divider, then the
    /// About entry with the version under it — reference material is
    /// visibly a different kind of thing from a section, but still one
    /// click away (it used to be the same row style, pinned at the
    /// bottom with nothing saying why).
    fn sidebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let collapsed = self.sidebar_collapsed;
        let mut nav = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(8.))
            .pt(px(8.))
            .flex_1()
            .min_h(px(0.));
        for (key, icon, target) in [
            (
                "settings.general.title",
                IconName::Settings2,
                Section::General,
            ),
            (
                "settings.toolbar_page.title",
                IconName::PanelTop,
                Section::Toolbar,
            ),
            (
                "settings.keys_page.title",
                IconName::Keyboard,
                Section::Keys,
            ),
            ("settings.ocr_page.title", IconName::ScanText, Section::Ocr),
            (
                "settings.appearance.title",
                IconName::Palette,
                Section::Appearance,
            ),
        ] {
            nav = nav.child(self.nav_item(key, icon, target, collapsed, cx));
        }
        let about = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(8.))
            .pb(px(10.))
            .child(
                div()
                    .h(px(1.))
                    .mx(px(6.))
                    .mb(px(6.))
                    .bg(rgba(alpha(t.toolbar_border, 0x59))),
            )
            .child(self.nav_item(
                "settings.about.nav",
                IconName::Info,
                Section::About,
                collapsed,
                cx,
            ));
        let about = if collapsed {
            about
        } else {
            about.child(
                div()
                    .px(px(10.))
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
        };
        div()
            .flex()
            .flex_col()
            .h_full()
            .flex_shrink_0()
            .w(px(if collapsed { RAIL_W } else { SIDEBAR_W }))
            .border_r_1()
            .border_color(rgba(alpha(t.toolbar_border, 0x80)))
            .child(nav)
            .child(about)
    }

    /// The status strip under every page: "changes save immediately" (or
    /// the last save failure) on the left, the page's reset action on the
    /// right. Before this, the reset button lived inside two pages at two
    /// different heights and every page ended in dead space.
    fn status_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        let (status, color) = match self.error.clone() {
            Some(error) => (
                crate::i18n::t!("settings.footer.not_saved", error = error).to_string(),
                t.accent,
            ),
            None => (
                crate::i18n::t!("settings.footer.saved").to_string(),
                t.hint_text,
            ),
        };
        let reset = match self.section {
            Section::Toolbar => Some(reset_button({
                let entity = entity.clone();
                move |_: &ClickEvent, _, cx: &mut App| {
                    entity.update(cx, |this, cx| this.reset_toolbar(cx));
                }
            })),
            Section::Keys => Some(reset_button({
                let entity = entity.clone();
                move |_: &ClickEvent, _, cx: &mut App| {
                    entity.update(cx, |this, cx| {
                        this.settings.reset_keys();
                        this.persist(cx);
                    });
                }
            })),
            Section::Appearance => Some(reset_button({
                let entity = entity.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    entity.update(cx, |this, cx| this.appearance_reset(window, cx));
                }
            })),
            _ => None,
        };
        let _ = window;
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .h(px(FOOTER_H))
            .flex_shrink_0()
            .px(px(PAGE_PAD))
            .border_t_1()
            .border_color(rgba(alpha(t.toolbar_border, 0x59)))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(color))
                    .child(status),
            )
            .children(reset)
    }

    /// The content pane: the active section's page. This match is the
    /// section registry — a new section is a variant, a page fn and a
    /// sidebar item above. `min_w(0)` lets the pane shrink below its
    /// content (a tiled window can be narrower than the widest page)
    /// so text wraps at the VISIBLE width instead of laying out at
    /// content width and clipping.
    fn page(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut page = match self.section {
            Section::General => self.page_general(cx),
            Section::Toolbar => self.page_toolbar(cx),
            Section::Keys => self.page_keys(cx),
            Section::Ocr => self.page_ocr(cx),
            Section::Appearance => self.page_appearance(cx),
            Section::About => self.page_about(cx),
        };
        page = page.flex_1().min_w(px(0.));
        page
    }
}

mod about;
mod appearance;
mod general;
mod keys;
mod ocr;
mod toolbar_editor;
