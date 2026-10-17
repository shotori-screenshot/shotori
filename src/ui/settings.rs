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
use gpui_kit::base::IndexPath;
use gpui_kit::base::slider::SliderState;
use gpui_kit::component::color_picker::ColorPickerState;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarHeader, SidebarItem, SidebarMenu, SidebarMenuItem, SidebarToggleButton,
};
use gpui_kit::component::{
    ActiveTheme, Collapsible, Icon, Root, TITLE_BAR_HEIGHT, Theme, ThemeMode, TitleBar,
};
use gpui_kit::*;

use crate::model::settings::{OcrModel, Settings, ToolbarItem};
use crate::ui::theme::load::{self, ThemeFile};
use crate::ui::theme::{self, ColorKey};

use self::toolbar_editor::DragPreview;

gpui_kit::actions!([CloseSettings]);

const WINDOW_W: f32 = 1024.;
const WINDOW_H: f32 = 640.;
const PAGE_PAD: f32 = 24.;

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
            section: Section::Toolbar,
            sidebar_collapsed: settings.sidebar_collapsed(),
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            drag: None,
            appearance,
            language_select,
            ocr_select,
            preview_scroll: ScrollHandle::new(),
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
            .child(self.title_bar(window, cx))
            .child(
                // The navigation column and the page share the height
                // BELOW the title bar; min_h_0 lets the page scroll in
                // a short window instead of overflowing the column.
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar(window, cx))
                    .child(self.page(cx)),
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

/// One page's heading.
fn page_header(title: &str, blurb: &str) -> Div {
    let t = theme::c();
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(16.))
                .text_color(rgba(t.btn_text))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgba(t.hint_text))
                .child(blurb.to_string()),
        )
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
    fn title_bar(&self, window: &Window, cx: &App) -> TitleBar {
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
        // `TitleBar` hides its own control cluster when the compositor
        // claims server-side decorations — it must not duplicate a
        // system title bar. But some compositors claim that while
        // drawing nothing (niri with `prefer-no-csd`), so OUR bar is the
        // only chrome the window ever gets and has to carry the
        // controls itself. In client-decorated mode the component's
        // cluster handles it (outside the drag region, same as here).
        let own_controls = matches!(window.window_decorations(), Decorations::Server)
            .then(|| window_controls(window, cx));
        TitleBar::new().child(brand).children(own_controls)
    }

    /// The navigation sidebar: one item per section, the active page
    /// highlighted, About pinned to the footer. Takes the window:
    /// SidebarMenuItem only becomes an element through
    /// [`SidebarItem::render`], which needs one.
    fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> Sidebar<SidebarMenu> {
        let active = self.section;
        let collapsed = self.sidebar_collapsed;
        // `label` is a translation KEY (settings.general.title etc.) —
        // the sidebar translates on render, so a live locale flip
        // relabels the navigation with everything else. Icons ride only
        // on the collapsed rail: expanded, the label text carries the
        // meaning and an icon column next to it read as clutter.
        let item = |key: &'static str, icon: IconName, target: Section| {
            let selected = active == target;
            let click = cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                if this.section != target {
                    this.section = target;
                    cx.notify();
                }
            });
            let item = SidebarMenuItem::new(crate::i18n::t!(key).to_string());
            let item = if collapsed {
                item.icon(Icon::new(icon))
            } else {
                item
            };
            item.active(selected).on_click(click)
        };
        // Header: the expand/collapse control and nothing else. The
        // branding moved to the window title bar (the one strip whose
        // width does not change when the rail collapses), so the header
        // is the same one-button row whether the rail is open or shut.
        let toggle = |collapsed_state: bool| {
            let label = if collapsed_state {
                crate::i18n::t!("settings.nav.expand")
            } else {
                crate::i18n::t!("settings.nav.collapse")
            };
            SidebarToggleButton::new()
                .collapsed(collapsed_state)
                .accessibility_label(label.to_string())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.toggle_sidebar(cx);
                }))
        };
        let mut identity = SidebarHeader::new().collapsed(collapsed);
        if collapsed {
            // The 48px rail centers the expand button over the icons.
            identity = identity.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .child(toggle(true)),
            );
        } else {
            // Expanded: the button sits in the item column's leading
            // edge — the same x it occupies centered on the collapsed
            // rail. A trailing-edge button jumps ~200px sideways when
            // the rail opens and reads as unrelated chrome.
            identity = identity.child(
                div()
                    .flex()
                    .items_center()
                    .justify_start()
                    .w_full()
                    .child(toggle(false)),
            );
        }
        // Built BEFORE the sidebar chain: the `item` helper holds a
        // shared cx borrow (cx.listener), while the footer's
        // SidebarItem::render below needs cx mutably — the menu must
        // be done borrowing before the chain evaluates the footer.
        let menu = SidebarMenu::new()
            .child(item(
                "settings.general.title",
                IconName::Settings2,
                Section::General,
            ))
            .child(item(
                "settings.toolbar_page.title",
                IconName::PanelTop,
                Section::Toolbar,
            ))
            .child(item(
                "settings.keys_page.title",
                IconName::Keyboard,
                Section::Keys,
            ))
            .child(item(
                "settings.ocr_page.title",
                IconName::ScanText,
                Section::Ocr,
            ))
            .child(item(
                "settings.appearance.title",
                IconName::Palette,
                Section::Appearance,
            ));
        // About (the footer item): same shape as every nav item, and
        // like them it carries an icon only on the collapsed rail —
        // expanded is text-only.
        let about = SidebarMenuItem::new(crate::i18n::t!("settings.about.nav").to_string())
            .collapsed(collapsed)
            .active(active == Section::About)
            .on_click(cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
                if this.section != Section::About {
                    this.section = Section::About;
                    cx.notify();
                }
            }));
        let about = if collapsed {
            about.icon(Icon::new(IconName::Info))
        } else {
            about
        };
        Sidebar::new("settings-sidebar")
            .collapsible(true)
            .collapsed(collapsed)
            .header(identity)
            .footer(
                // About lives in the footer SLOT (reference material,
                // not a working section) but is the SAME element as
                // every nav item: a bare SidebarMenuItem, no
                // SidebarFooter wrapper. That wrapper is a user-card
                // shaped box — double padding, plus its own
                // hover/selected states stacked on the item's — which
                // made the entry read as a different kind of thing and
                // sat its text 8px off the nav grid. The slot itself
                // contributes only the bottom pinning and the shared
                // px_3 insets; collapsed it still degrades to the icon
                // + managed tooltip. The icon rides only on the
                // collapsed rail (same rule as the nav items —
                // expanded is text-only).
                about.render("sidebar-about", window, cx),
            )
            .child(menu)
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
