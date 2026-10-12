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
use gpui_kit::base::slider::{
    Slider, SliderEvent, SliderState, SliderThumb, SliderTrack, SliderValue,
};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarHeader, SidebarItem, SidebarMenu, SidebarMenuItem, SidebarToggleButton,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Collapsible, Icon, Root, Sizable, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::settings::{OcrModel, Settings, ToolbarButton, ToolbarCluster, ToolbarItem};
use crate::ui::theme::load::{self, ThemeFile};
use crate::ui::theme::{self, ColorKey};
use crate::ui::toolbar;

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
    Ocr,
    Appearance,
    /// The identity card — a footer entry, not main navigation (the
    /// nav is for working sections; About is reference material).
    About,
}

/// Which config file an action targets (General page's edit buttons).
#[derive(Clone, Copy)]
enum ConfigFile {
    Settings,
    Theme,
}

/// The drag-and-drop payload: what is being carried and where it came
/// from (`None` = palette: a fresh insert; `Some(i)` = the preview
/// bar: a reorder that removes slot i first).
struct DraggedItem {
    item: ToolbarItem,
    from: Option<usize>,
}

/// Presentation mirror of the live drag: the carried item, its origin
/// (`None` = palette) and the insertion slot the pointer last aimed —
/// indexed against the DISPLAYED row (the lifted item has left the
/// flow). Every drag-move listener refreshes it wholesale from the
/// event payload; `commit` clears it. Render gates it on
/// `has_active_drag` — a cancelled drag leaves a stale entry that is
/// invisible until the next drag's first move overwrites it (one
/// frame at worst).
struct DragPreview {
    item: ToolbarItem,
    origin: Option<usize>,
    slot: usize,
}

/// The drag payload marking a scrollbar-rail scrub in flight.
struct RailScrub;

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

/// An empty drag ghost for the scrollbar scrub — the thumb itself is
/// the feedback (it re-renders from the handle on every scrub event),
/// so nothing follows the pointer.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The scrub's ghost constructor (fn item, matching the higher-ranked
/// `impl Fn` bound `on_drag` demands — see `ghost_constructor`).
fn rail_scrub_ghost(
    _: &RailScrub,
    _: Point<Pixels>,
    _: &mut Window,
    cx: &mut App,
) -> Entity<NoGhost> {
    cx.new(|_| NoGhost)
}

/// The chip that follows the pointer during a drag.
struct DragGhost {
    item: ToolbarItem,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = theme::c();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .h(px(28.))
            .rounded(px(6.))
            .bg(rgba(t.toolbar_bg))
            .border_1()
            .border_color(rgba(t.accent))
            .shadow_md()
            .text_size(px(12.))
            .text_color(rgba(t.toolbar_text))
            .child(icon_visual(self.item))
            .child(item_name(self.item))
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
        titlebar: Some(TitlebarOptions {
            title: Some(crate::i18n::t!("settings.title").to_string().into()),
            ..Default::default()
        }),
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
        ..Default::default()
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
            edge_scroll: AutoScroll::default(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
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

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.commit(Settings::default().row().to_vec(), cx);
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

    /// A drop landed in preview slot `i` (slot i sits before row item
    /// i as displayed during the drag — the lifted item is not in the
    /// count; what the gap preview showed is what lands). The index
    /// math lives in the model, tested there.
    fn drop_at(&mut self, i: usize, dragged: &DraggedItem, cx: &mut Context<Self>) {
        let row = Settings::apply_drop(dragged.item, self.settings.row(), i, dragged.from);
        self.edge_scroll.stop();
        self.commit(row, cx);
    }

    /// Click on a preview item: remove it (drops back into the palette).
    fn remove_at(&mut self, i: usize, cx: &mut Context<Self>) {
        let mut row = self.settings.row().to_vec();
        if i < row.len() {
            row.remove(i);
            self.commit(row, cx);
        }
    }

    /// Click on a palette chip: append to the row's end.
    fn append(&mut self, item: ToolbarItem, cx: &mut Context<Self>) {
        let mut row = self.settings.row().to_vec();
        row.push(item);
        self.commit(row, cx);
    }

    /// Open a config file in the system editor. A MISSING file is
    /// materialized first — settings gets the current state written
    /// out, theme a base-only shotori file — so "edit my config" never
    /// hands the editor a void. Materialization failures land in the
    /// usual error slot; opening failures surface as a notification
    /// from [`open_config_file`]'s thread.
    fn open_config(&mut self, file: ConfigFile, cx: &mut Context<Self>) {
        let Some(path) = (match file {
            ConfigFile::Settings => self.path.clone(),
            ConfigFile::Theme => self.appearance.path.clone(),
        }) else {
            crate::notify::send(
                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                crate::i18n::t!("settings.no_config_dir").as_ref(),
            );
            return;
        };
        if !path.exists() {
            let materialized = match file {
                ConfigFile::Settings => self.settings.save(&path),
                ConfigFile::Theme => load::save(
                    &path,
                    &ThemeFile {
                        base: Some("shotori".into()),
                        ..Default::default()
                    },
                ),
            };
            if let Err(error) = materialized {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        open_external(path.display().to_string());
    }

    /// The live toolbar: the real bar container and grip textures with
    /// the configured row inside. Items drag (reorder) and click
    /// (remove). While a drag is in flight the carried item LEAVES the
    /// flow (the ghost under the cursor is the item) and the aimed
    /// slot renders as a gap the item's own footprint wide — a live
    /// preview of the row after the drop, so the bar's width stays
    /// constant during a reorder (the gap replaces the lifted item).
    /// Drop targets are the items themselves (before/after by pointer
    /// vs the item's center — a full 30px target, not a strip); the
    /// grips pin the two ends.
    fn toolbar_preview(&self, cx: &Context<Self>) -> Div {
        let row: Vec<ToolbarItem> = self.settings.row().to_vec();
        let len = row.len();
        // what the drag carries and where it came from — the mirror is
        // only trusted while a drag is actually active (a cancelled
        // drag leaves it stale but `None`-gated here)
        let drag = if cx.has_active_drag() {
            self.drag.as_ref()
        } else {
            None
        };
        // the displayed row: the lifted item is hidden; the aimed slot
        // is clamped into the displayed range (a stale slot from a
        // shorter previous row must not index past the end)
        let hide = drag.and_then(|d| d.origin);
        let shown: Vec<(usize, ToolbarItem)> = row
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != hide)
            .map(|(i, item)| (i, *item))
            .collect();
        let slot = drag.map(|d| d.slot.min(shown.len()));
        let pin_start = cx.listener(
            move |this: &mut Self,
                  event: &DragMoveEvent<DraggedItem>,
                  _,
                  cx: &mut Context<Self>| {
                // on_drag_move fires for EVERY element that registered a
                // listener during a matching drag — it has no hover filter
                // (it exists for drag sources tracking themselves, "like
                // resizing" per its docs). Self-filter by pointer-in-bounds,
                // or every listener overwrites the others and the gap
                // sticks wherever the last-registered element (the right
                // grip) pins it.
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let (item, from) = drag_payload(event, cx);
                this.aim_drag(item, from, 0, cx);
            },
        );
        let pin_end = cx.listener(
            move |this: &mut Self,
                  event: &DragMoveEvent<DraggedItem>,
                  _,
                  cx: &mut Context<Self>| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let (item, from) = drag_payload(event, cx);
                // `len` counts the FULL row; during a reorder one item
                // is hidden, so this clamps to the displayed end inside
                // `aim_drag`'s consumers (and `apply_drop` clamps too)
                this.aim_drag(item, from, len, cx);
            },
        );
        let bar_fallback = cx.listener(
            move |this: &mut Self, dragged: &DraggedItem, _, cx: &mut Context<Self>| {
                // gaps, padding and the gap preview itself: no item
                // handled the drop — land at the slot the pointer last
                // aimed at
                if let Some(slot) = this.drag.as_ref().map(|d| d.slot) {
                    this.drop_at(slot, dragged, cx);
                }
            },
        );
        let mut bar = toolbar::bar()
            // the strip lays its child out as a flex ROW item: `flex_none`
            // (no grow, no shrink) keeps the bar at its content width —
            // wider rows overflow the strip and scroll, they must never
            // be stretched (gaps inflate) or squeezed (buttons compress)
            .flex_none()
            .on_drop(bar_fallback)
            .child(grip_end("pv-grip-left").on_drag_move(pin_start));
        for (j, (i, item)) in shown.iter().enumerate() {
            // `slot` derives from `drag`, so Some(slot) implies the
            // drag mirror is present — the footprint is the carried
            // item's own
            if slot == Some(j)
                && let Some(d) = drag
            {
                bar = bar.child(insertion_slot(item_footprint(d.item)));
            }
            bar = bar.child(self.preview_item(*i, j, *item, cx));
        }
        if slot == Some(shown.len())
            && let Some(d) = drag
        {
            bar = bar.child(insertion_slot(item_footprint(d.item)));
        }
        bar.child(grip_end("pv-grip-right").on_drag_move(pin_end))
    }

    /// Record what the drag carries and the slot it aims; notifies
    /// only on change (drag-move fires on every mouse move). The slot
    /// is indexed against the DISPLAYED row — the lifted item is not
    /// in it (see [`Settings::apply_drop`]).
    fn aim_drag(
        &mut self,
        item: ToolbarItem,
        origin: Option<usize>,
        slot: usize,
        cx: &mut Context<Self>,
    ) {
        if self
            .drag
            .as_ref()
            .is_some_and(|d| d.origin == origin && d.slot == slot)
        {
            return;
        }
        self.drag = Some(DragPreview { item, origin, slot });
        cx.notify();
    }

    /// One preview item: the real visual, but dragging reorders and
    /// clicking removes — the action itself never fires here. `i` is
    /// the ORIGINAL row index (the payload's removal source and the
    /// click-to-remove target); `j` is the DISPLAYED index while a
    /// drag hides the lifted item — insertion slots count against the
    /// displayed row, so the gap preview and the eventual drop agree.
    fn preview_item(
        &self,
        i: usize,
        j: usize,
        item: ToolbarItem,
        cx: &Context<Self>,
    ) -> AnyElement {
        let payload = DraggedItem {
            item,
            from: Some(i),
        };
        // the whole item is the drop target: before me / after me by
        // the pointer against my own center (the event carries this
        // element's hitbox bounds; the contains() guard is the hover
        // filter on_drag_move lacks — see the grips' note)
        let hover = cx.listener(
            move |this: &mut Self,
                  event: &DragMoveEvent<DraggedItem>,
                  _,
                  cx: &mut Context<Self>| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                // the slot this item aims at; the payload says what is
                // being carried (hovering A while dragging B aims B's
                // gap around A, not A's own)
                let (item, from) = drag_payload(event, cx);
                let center = event.bounds.origin.x + event.bounds.size.width / 2.;
                let slot = usize::from(event.event.position.x > center) + j;
                this.aim_drag(item, from, slot, cx);
            },
        );
        let drop = cx.listener(
            move |this: &mut Self, dragged: &DraggedItem, _, cx: &mut Context<Self>| {
                let slot = this.drag.as_ref().map(|d| d.slot).unwrap_or(j);
                this.drop_at(slot, dragged, cx);
            },
        );
        match item {
            ToolbarItem::Button(button) => match toolbar::meta_for(button) {
                Some(meta) => {
                    let remove = cx.entity();
                    toolbar::preview_control(meta, &self.focus, move |_, cx| {
                        remove.update(cx, |this, cx| this.remove_at(i, cx));
                    })
                    .on_drag(payload, ghost_constructor)
                    .on_drag_move(hover)
                    .on_drop(drop)
                    .into_any_element()
                }
                // unreachable: BUTTONS covers every ToolbarButton
                None => div().into_any_element(),
            },
            ToolbarItem::Separator => layout_visual(
                // UNIQUE per instance: stateful element state (the
                // pending mouse-down that arms a drag) is keyed by
                // element id. A shared "pv-separator" made one press
                // arm EVERY separator's drag listener at once, and
                // bubble dispatch (right-to-left) let the RIGHTMOST
                // separator win the drag start — grabbing any of them
                // always picked up the rightmost one (user-reported).
                format!("pv-separator-{i}").into(),
                payload,
                cx,
                i,
                hover,
                drop,
                div()
                    .w(px(2.))
                    .h(px(18.))
                    .bg(rgba(theme::c().toolbar_border)),
            ),
        }
    }

    /// The palette: every control not currently on the bar, plus the
    /// two layout items (any number of those may live on the bar).
    /// Chips drag onto the bar, click appends. The palette area is
    /// also the remove target: dropping a bar item here takes it off.
    fn palette(&self, cx: &Context<Self>) -> Stateful<Div> {
        let placed: std::collections::BTreeSet<ToolbarButton> = self
            .settings
            .row()
            .iter()
            .filter_map(|item| match item {
                ToolbarItem::Button(button) => Some(*button),
                _ => None,
            })
            .collect();
        let mut chips = div().flex().flex_wrap().gap(px(6.));
        for cluster in [
            ToolbarCluster::Tools,
            ToolbarCluster::Actions,
            ToolbarCluster::Confirm,
        ] {
            for meta in toolbar::BUTTONS
                .iter()
                .filter(|meta| meta.button.cluster() == cluster)
            {
                if placed.contains(&meta.button) {
                    continue;
                }
                chips = chips.child(self.palette_chip(ToolbarItem::Button(meta.button), cx));
            }
        }
        // layout items are always offered — separators stack
        chips = chips.child(self.palette_chip(ToolbarItem::Separator, cx));

        let entity = cx.entity();
        div()
            .id("pv-palette")
            .flex()
            .flex_col()
            .gap(px(6.))
            // dropping a bar item back here removes it; palette-origin
            // drops just return home (no-op)
            .on_drop(move |dragged: &DraggedItem, _, cx| {
                if let Some(src) = dragged.from {
                    entity.update(cx, |this, cx| {
                        this.edge_scroll.stop();
                        this.remove_at(src, cx)
                    });
                }
            })
            .drag_over::<DraggedItem>(|s, _, _, _| s.border_color(rgba(theme::c().accent)))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(theme::c().toolbar_border))
            .p(px(10.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(theme::c().hint_text))
                    .child(crate::i18n::t!("settings.toolbar_page.palette_hint").to_string()),
            )
            .child(chips)
    }

    fn palette_chip(&self, item: ToolbarItem, cx: &Context<Self>) -> Stateful<Div> {
        let entity = cx.entity();
        let payload = DraggedItem { item, from: None };
        let click_entity = entity.clone();
        div()
            .id(SharedString::from(format!("pv-chip-{}", item.id())))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(theme::c().toolbar_border))
            .text_size(px(12.))
            .text_color(rgba(theme::c().toolbar_text))
            .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
            .on_drag(payload, ghost_constructor)
            .on_click(move |_, _, cx| {
                click_entity.update(cx, |this, cx| this.append(item, cx));
            })
            .child(icon_visual(item))
            .child(item_name(item))
    }

    fn footer(&self, cx: &Context<Self>) -> Div {
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(12.))
            .child(
                Button::new("settings-reset")
                    .accessibility_label(crate::i18n::t!("settings.toolbar_page.reset").to_string())
                    .px(px(12.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(rgba(theme::c().btn_text))
                    .border_1()
                    .border_color(rgba(theme::c().toolbar_border))
                    .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
                    .focus_visible(|s| s.border_color(rgba(theme::c().accent)))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| this.reset(cx));
                    })
                    .child(crate::i18n::t!("settings.toolbar_page.reset").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .text_right()
                    .text_size(px(11.))
                    // errors only — where the file lives is one click
                    // away (General's config buttons); an always-on
                    // path line was clutter
                    .when_some(self.error.clone(), |el, error| {
                        el.text_color(rgba(theme::c().accent)).child(error)
                    }),
            )
    }
}

/// A bar-end grip for the preview: the real texture, non-interactive
/// as a grip (dragging the PREVIEW bar around would be a lie — it has
/// no placement), but it carries the drag-move target that pins the
/// insertion point to its end of the bar.
fn grip_end(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .px(px(crate::model::placement::BAR_PAD))
        .child(toolbar::grip_texture())
}

/// The drag-ghost constructor gpui calls when a drag starts. A fn
/// item (not a closure) so it coerces to the higher-ranked `impl Fn`
/// bound `on_drag` demands.
fn ghost_constructor(
    item: &DraggedItem,
    _: Point<Pixels>,
    _: &mut Window,
    cx: &mut App,
) -> gpui_kit::Entity<DragGhost> {
    cx.new(|_| DragGhost { item: item.item })
}

/// The live-insertion preview: a reserved gap the carried item's own
/// footprint wide, so the row reads exactly as it will after the drop
/// (a bar-origin reorder keeps the bar's width constant — the gap
/// replaces the lifted item; a palette insert grows it by one
/// footprint, bounded, unlike the old per-slot strips that grew the
/// bar ~350px and pushed the right grip off the window).
fn insertion_slot(footprint: Pixels) -> Div {
    div()
        .w(footprint)
        .h(px(26.))
        .rounded(px(5.))
        .flex_shrink_0()
        .border_1()
        .border_color(rgba(theme::c().accent))
        .bg(rgba(theme::c().accent).opacity(0.15))
}

/// The width an item occupies on the bar — what the insertion gap
/// reserves. Must track the real visuals' footprints (`control`'s
/// 30px buttons, `layout_visual`'s 18px separator hit shell) or the
/// bar visibly changes width when a drag lifts an item.
fn item_footprint(item: ToolbarItem) -> Pixels {
    match item {
        ToolbarItem::Button(_) => px(30.),
        ToolbarItem::Separator => px(18.),
    }
}

/// Copy the drag payload out of a drag-move event (the whole payload,
/// not just the slot: hovering A while carrying B must aim B's gap
/// around A). Returns copies — the payload borrows `cx`, which the
/// caller needs mutably right after.
fn drag_payload(event: &DragMoveEvent<DraggedItem>, cx: &App) -> (ToolbarItem, Option<usize>) {
    let drag = event.drag(cx);
    (drag.item, drag.from)
}

/// Wrap a preview layout item (the separator) in the interaction
/// shell the buttons get from `preview_control`: drag to reorder,
/// hover to aim the caret, click to remove. The visual sits centered
/// in a wider hit target — a 2px separator is un-grabbable on its
/// own.
fn layout_visual(
    id: SharedString,
    payload: DraggedItem,
    cx: &Context<SettingsWindow>,
    i: usize,
    hover: impl Fn(&DragMoveEvent<DraggedItem>, &mut Window, &mut App) + 'static,
    drop: impl Fn(&DraggedItem, &mut Window, &mut App) + 'static,
    visual: impl IntoElement,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(18.))
        .h(px(crate::model::placement::ROW_H - 8.))
        .rounded(px(4.))
        .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
        .on_drag(payload, ghost_constructor)
        .on_drag_move(hover)
        .on_drop(drop)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.remove_at(i, cx);
        }))
        .child(visual)
        .into_any_element()
}

/// The icon an item shows outside the button chrome (ghost, palette
/// chip): the toolbar's own glyphs for buttons, a small bar for the
/// separator — the palette chip previews exactly what lands.
fn icon_visual(item: ToolbarItem) -> AnyElement {
    match item {
        ToolbarItem::Button(button) => match toolbar::meta_for(button) {
            Some(meta) => meta.icon.svg().into_any_element(),
            None => div().into_any_element(),
        },
        ToolbarItem::Separator => div()
            .w(px(2.))
            .h(px(14.))
            .bg(rgba(theme::c().toolbar_border))
            .into_any_element(),
    }
}

fn item_name(item: ToolbarItem) -> String {
    match item {
        ToolbarItem::Button(button) => match toolbar::meta_for(button) {
            Some(meta) => meta.label().to_string(),
            None => button.id().to_string(),
        },
        ToolbarItem::Separator => crate::i18n::t!("settings.toolbar_page.separator").to_string(),
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::c();
        div()
            .id("settings-root")
            .size_full()
            .flex()
            .bg(rgba(t.toolbar_bg))
            .text_color(rgba(t.toolbar_text))
            .track_focus(&self.focus)
            .key_context("ShotoriSettings")
            // Esc mirrors the titlebar close (the binding is registered
            // in `run`; handled here — app-level backstops never see
            // window-dispatched actions)
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| {
                window.remove_window();
            }))
            .child(self.sidebar(window, cx))
            .child(self.page(cx))
    }
}

/// Thumb display geometry for the rail: `(thumb_w, usable_track)`,
/// derived from the handle — viewport/content ratio, clamped to a
/// grabbable 24px minimum so a long row keeps a handle-sized thumb.
/// One helper so the render and the scrub cannot disagree.
fn rail_thumb_geometry(handle: &ScrollHandle) -> (Pixels, Pixels) {
    let viewport_w = handle.bounds().size.width;
    let max_x = handle.max_offset().x;
    let content_w = (viewport_w + max_x).max(px(1.));
    let ratio = (viewport_w / content_w).clamp(0., 1.);
    let thumb = px(ratio * viewport_w.as_f32()).max(px(24.));
    let usable = (viewport_w - thumb).max(px(0.));
    (thumb, usable)
}

/// The preview strip's scroll position in THIS file's domain: the
/// distance from the start, `0..=max_offset().x`, positive toward
/// the end of the row.
///
/// gpui stores a scroll container's offset as the ELEMENT offset —
/// `0` at the start and NEGATIVE `max_offset()` at the end (content
/// shifts left; paint clamps into `[-max, 0]`). The library's own
/// scrollbar negates on both sides of that boundary
/// (`ThumbGeometry::start` reads `-offset`; its drag write-back is
/// `-extent * fraction`). These helpers are the single place the
/// sign flips: every read/write of `preview_scroll` must go through
/// them. Writing a positive-domain value straight into the handle is
/// a silent no-op — paint clamps it back to "start" — which is how
/// the strip once lost every horizontal scroll path at once (wheel,
/// thumb, scrub, edge auto-scroll; the sign-domain bug).
fn strip_scroll_pos(handle: &ScrollHandle) -> Pixels {
    -handle.offset().x
}

/// Write `pos` (this file's positive domain; the CALLER clamps to
/// `[0, max]` — a bare handle cannot seed `max_offset` for a unit
/// test, and every call site already owns a max) back into the
/// handle, negated into gpui's element-offset domain.
fn set_strip_scroll_pos(handle: &ScrollHandle, pos: Pixels) {
    handle.set_offset(point(-pos, handle.offset().y));
}

/// Ramped auto-scroll delta for a drag parked at the strip's edge:
/// nothing across the middle of the strip, then speed ramps from
/// ~6px/frame at the zone boundary to ~36px/frame at the very edge
/// (the same idea as gpui-base's vertical curve). Positive scrolls
/// right — toward the end of the row.
fn edge_scroll_delta(local_x: Pixels, width: Pixels) -> Option<Pixels> {
    const ZONE: f32 = 48.;
    const MIN_SPEED: f32 = 6.;
    const MAX_SPEED: f32 = 36.;
    let speed = |distance: Pixels| {
        let t = 1. - (distance / px(ZONE)).clamp(0., 1.);
        px(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))
    };
    let from_left = local_x;
    let from_right = width - local_x;
    if from_left < px(ZONE) {
        Some(-speed(from_left))
    } else if from_right < px(ZONE) {
        Some(speed(from_right))
    } else {
        None
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

/// The About page's hero logo (256px master — plenty for a 128px
/// display size on HiDPI).
static ABOUT_LOGO: std::sync::LazyLock<std::sync::Arc<RenderImage>> =
    std::sync::LazyLock::new(|| {
        png_render_image(include_bytes!("../../assets/app/shotori-256.png"))
    });

impl SettingsWindow {
    /// The navigation sidebar: one item per section, the active page
    /// highlighted, About pinned to the footer. Takes the window:
    /// SidebarMenuItem only becomes an element through
    /// [`SidebarItem::render`], which needs one.
    fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> Sidebar<SidebarMenu> {
        let active = self.section;
        let collapsed = self.sidebar_collapsed;
        // `label` is a translation KEY (settings.general.title etc.) —
        // the sidebar translates on render, so a live locale flip
        // relabels the navigation with everything else.
        let item = |key: &'static str, icon: IconName, target: Section| {
            let selected = active == target;
            let click = cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                if this.section != target {
                    this.section = target;
                    cx.notify();
                }
            });
            SidebarMenuItem::new(crate::i18n::t!(key).to_string())
                .icon(Icon::new(icon))
                .active(selected)
                .on_click(click)
        };
        // The header/footer/toggle wiring is the component's demo shape:
        // the Sidebar itself pushes `collapsed` into the menu items, but
        // the header, footer and toggle button are plain elements — the
        // app owns the bool and threads it into each of them.
        let toggle = SidebarToggleButton::new()
            .collapsed(collapsed)
            .accessibility_label(if collapsed {
                crate::i18n::t!("settings.nav.expand").to_string()
            } else {
                crate::i18n::t!("settings.nav.collapse").to_string()
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                // write-through like every other control in this window
                this.sidebar_collapsed = !this.sidebar_collapsed;
                this.settings.set_sidebar_collapsed(this.sidebar_collapsed);
                this.persist(cx);
            }));
        let mut identity = SidebarHeader::new().collapsed(collapsed);
        // The logo: the app icon master (the same 64px asset the tray
        // embeds), decoded once — 64px stays crisp at the header's 16px
        // logical size on HiDPI.
        let logo = || img(ImageSource::Render(SIDEBAR_LOGO.clone())).size_4();
        if collapsed {
            // icon rail: logo (branding survives collapsing) stacked
            // over the toggle — the header is an h_flex, so wrap the
            // pair to keep them centered in the narrow rail
            identity = identity.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .child(logo())
                    .child(toggle),
            );
        } else {
            identity = identity
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(logo())
                        .child(div().text_size(px(13.)).child("Shotori")),
                )
                .child(toggle);
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
                "settings.ocr_page.title",
                IconName::ScanText,
                Section::Ocr,
            ))
            .child(item(
                "settings.appearance.title",
                IconName::Palette,
                Section::Appearance,
            ));
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
                // + managed tooltip.
                SidebarMenuItem::new(crate::i18n::t!("settings.about.nav").to_string())
                    .icon(Icon::new(IconName::Info))
                    .collapsed(collapsed)
                    .active(active == Section::About)
                    .on_click(cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
                        if this.section != Section::About {
                            this.section = Section::About;
                            cx.notify();
                        }
                    }))
                    .render("sidebar-about", window, cx),
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
            Section::Ocr => self.page_ocr(cx),
            Section::Appearance => self.page_appearance(cx),
            Section::About => self.page_about(cx),
        };
        page = page.flex_1().min_w(px(0.));
        page
    }

    /// General: read-only facts about this installation plus the UI
    /// language picker (the one control here that changes something).
    fn page_general(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        div()
            .id("page-general")
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                crate::i18n::t!("settings.general.title").as_ref(),
                crate::i18n::t!("settings.general.blurb").as_ref(),
            ))
            .child(self.config_files_row(cx))
            .child(self.language_control(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.write_through").to_string()),
            )
    }

    /// The config-file row (General page): one compact button per
    /// file, opening it in the system editor. Replaces the old
    /// read-only path rows — the location is discovered by opening the
    /// file, and always-on paths everywhere was clutter.
    fn config_files_row(&self, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        let button = |label: &'static str, file: ConfigFile| {
            Button::new(SharedString::from(label))
                .accessibility_label(format!(
                    "{}: {label}",
                    crate::i18n::t!("settings.general.config_files")
                ))
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(rgba(t.btn_text))
                .border_1()
                .border_color(rgba(t.toolbar_border))
                .hover(|s| s.bg(rgba(t.toolbar_hover)))
                .focus_visible(|s| s.border_color(rgba(t.accent)))
                .on_click({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| this.open_config(file, cx));
                    }
                })
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.config_files").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(button("settings.toml", ConfigFile::Settings))
                    .child(button("theme.toml", ConfigFile::Theme)),
            )
    }

    /// The language picker (General page). A Confirm writes through,
    /// flips the process-global locale and refreshes every window —
    /// `t!` resolves on render, so the whole window including the
    /// component-library texts switches live, no restart.
    fn language_control(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.language.label").to_string()),
            )
            .child(
                div().w(px(280.)).child(
                    Select::new(&self.language_select)
                        .accessibility_label(
                            crate::i18n::t!("settings.general.language.label").to_string(),
                        )
                        .menu_width(px(280.)),
                ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.language.blurb").to_string()),
            )
    }
    /// OCR: the model-set dropdown + the upright-assume switch. The
    /// dropdown rows stay single-line (the model's localized label);
    /// the current choice's langs · tier · readiness render as one
    /// detail line under it, from `settings` — the file stays the
    /// single truth, the Select is just the picker.
    fn page_ocr(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let model = self.settings.ocr_model();
        let ready = crate::ocr::set_ready(model, self.settings.ocr_assume_upright());
        let entity = cx.entity();
        div()
            .id("page-ocr")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                crate::i18n::t!("settings.ocr_page.title").as_ref(),
                crate::i18n::t!("settings.ocr_page.blurb").as_ref(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(t.hint_text))
                            .child(crate::i18n::t!("settings.ocr_page.model").to_string()),
                    )
                    .child(
                        div().w(px(360.)).child(
                            Select::new(&self.ocr_select)
                                .accessibility_label(
                                    crate::i18n::t!("settings.ocr_page.model").to_string(),
                                )
                                .menu_width(px(420.)),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_size(px(11.))
                                    .text_color(rgba(t.hint_text))
                                    .child(format!("{} · {}", model.languages(), model.tier())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(rgba(if ready { t.hint_text } else { t.accent }))
                                    .child(if ready {
                                        crate::i18n::t!("settings.ocr_page.ready").to_string()
                                    } else {
                                        crate::i18n::t!("settings.ocr_page.downloads_first")
                                            .to_string()
                                    }),
                            ),
                    ),
            )
            // upright toggle: a real Switch (controlled — the owner
            // writes the requested value back and notifies)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(t.toolbar_border))
                    .child(
                        Switch::new("ocr-upright")
                            .checked(self.settings.ocr_assume_upright())
                            .on_click(move |checked, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.settings.set_ocr_assume_upright(*checked);
                                    this.persist(cx);
                                });
                            })
                            .color(rgba(t.accent)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().text_size(px(13.)).text_color(rgba(t.btn_text)).child(
                                crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                            ))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgba(t.hint_text))
                                    .child(
                                        crate::i18n::t!("settings.ocr_page.upright.body")
                                            .to_string(),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(
                        crate::i18n::t!(
                            "settings.ocr_page.cache_hint",
                            dir = crate::ocr::model_dir_display()
                        )
                        .to_string(),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                // errors only — the file's location is a button away
                // on the General page, an always-on path line was
                // clutter
                el.child(
                    div()
                        .text_size(px(11.))
                        .text_right()
                        .text_color(rgba(t.accent))
                        .child(error),
                )
            })
    }

    /// Toolbar: the drag-and-drop editor. The page itself NEVER
    /// scrolls — the header, the preview strip (+ its scrollbar rail),
    /// the hint and the footer are pinned, and only the palette below
    /// pans vertically in its own area. (A scrolling page carried the
    /// bar away with it — the bar must stay put under any scrolling;
    /// "only the toolbar scrolls, the rest does not move", horizontally
    /// AND vertically.)
    fn page_toolbar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        // Releasing anywhere on the page confirms the PREVIEWED slot:
        // the gap highlight is a promise, not a hint. Drop targets
        // that physically sit under the pointer win by bubbling
        // inner-first (items and the bar land, the palette removes);
        // this fallback catches every other spot the gap was still
        // visible from — the rail, the strip around the bar, the
        // header, the margins — where a release used to be silently
        // swallowed and the drag died with the preview lit (the
        // highlight/drop mismatch, user-reported).
        let page_fallback = cx.listener(
            move |this: &mut Self, dragged: &DraggedItem, _, cx: &mut Context<Self>| {
                if let Some(slot) = this.drag.as_ref().map(|d| d.slot) {
                    this.drop_at(slot, dragged, cx);
                }
            },
        );
        div()
            .id("page-toolbar")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .on_drop(page_fallback)
            // header
            .child(page_header(
                "Toolbar",
                crate::i18n::t!("settings.toolbar_page.blurb").as_ref(),
            ))
            // the live preview, in its own horizontally-scrolling strip
            .child(self.preview_strip(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.toolbar_page.hint").to_string()),
            )
            // the palette — the only vertically scrolling part; `min_h(0)`
            // lets the flex item shrink below its content (an
            // overflow:visible flex item refuses to, pushing the footer
            // off instead)
            .child(
                div()
                    .id("palette-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(self.palette(cx)),
            )
            .child(self.footer(cx))
    }

    /// The preview bar's scroll viewport. The bar hugs its content (the
    /// factory row is ~703px — the box follows the element, never the
    /// pane) and pans when the pane is narrower; its scrollbar is a
    /// separately drawn rail below, outside the bar's box ([`Self::
    /// scrollbar_rail`]). Dragging into an edge zone pans with the drag
    /// (classic edge auto-scroll: park the item at an edge and the
    /// strip keeps moving under it).
    fn preview_strip(&self, cx: &Context<Self>) -> Div {
        let edge = cx.listener(
            move |this: &mut Self,
                  event: &DragMoveEvent<DraggedItem>,
                  _,
                  cx: &mut Context<Self>| {
                // on_drag_move has no hover filter (see the grips' note
                // in `toolbar_preview`) — self-filter by pointer-in-bounds
                if !event.bounds.contains(&event.event.position) {
                    this.edge_scroll.stop();
                    return;
                }
                let local_x = event.event.position.x - event.bounds.origin.x;
                let delta = edge_scroll_delta(local_x, event.bounds.size.width);
                this.edge_scroll.set(delta, cx, |delta, this, cx| {
                    // the drag may have ended unseen (cancelled) — skip
                    // rather than scroll a dead gesture
                    if !cx.has_active_drag() {
                        return;
                    }
                    let handle = this.preview_scroll.clone();
                    let pos = strip_scroll_pos(&handle);
                    let max = handle.max_offset().x;
                    let next = (pos + delta).clamp(px(0.), max);
                    if next != pos {
                        set_strip_scroll_pos(&handle, next);
                        cx.notify();
                    }
                });
            },
        );
        // Wheel over the strip + rail: notify so the drawn thumb
        // follows whatever the container applied, and consume the
        // event while there is x-overflow so nothing above this
        // block can grow a scroll and carry the bar away again
        // (defensive — the page itself no longer scrolls; see
        // `page_toolbar`).
        div()
            .flex()
            .flex_col()
            // The clamp-writeback runs in OUR positive domain through
            // the boundary helpers (`strip_scroll_pos`): clamping the
            // RAW stored offset against [0, max] rewrote every real
            // scroll position (which gpui stores negated) back to 0 —
            // the strip could not be wheel-scrolled at all. Clamped in
            // the right domain it also normalizes any overshoot the
            // native application leaves behind before paint's own
            // clamp-writeback lands on the next frame.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                let handle = this.preview_scroll.clone();
                let max = handle.max_offset().x;
                if max > px(0.) {
                    cx.stop_propagation();
                }
                let pos = strip_scroll_pos(&handle);
                let clamped = pos.clamp(px(0.), max);
                if clamped != pos {
                    set_strip_scroll_pos(&handle, clamped);
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("preview-strip")
                    // row direction: the bar's width is its own content
                    // width, never stretched to fill the pane (the
                    // user-reported "box wider than the buttons" bug)
                    .flex()
                    .flex_row()
                    .overflow_x_scroll()
                    .track_scroll(&self.preview_scroll)
                    .on_drag_move(edge)
                    .child(self.toolbar_preview(cx)),
            )
            // the scrollbar, drawn outside the bar's box (see
            // [`SettingsWindow::scrollbar_rail`])
            .child(self.scrollbar_rail(cx))
    }

    /// The preview strip's scrollbar: HAND-DRAWN, below and outside the
    /// bar's box, with its geometry linked straight to the scroll
    /// handle — thumb width = viewport/content, thumb x = offset/max.
    ///
    /// The gpui-base `Scrollbar` component was tried here first and
    /// read as "inside the toolbar" twice: it lays itself out as an
    /// absolute overlay (relative(1.) sizing) whose faded pill sits
    /// flush against the bar's bottom edge, merging with the bar's
    /// airspace. A drawn rail with explicit coordinates cannot drift
    /// from where it is placed; scrubbing maps pointer x to offset
    /// Slider-track-style (the thumb centers under the pointer, so a
    /// click lands as a 1px drag and jumps).
    fn scrollbar_rail(&self, cx: &Context<Self>) -> Stateful<Div> {
        let handle = &self.preview_scroll;
        let max_x = handle.max_offset().x;
        let overflow = max_x > px(0.);
        let t = theme::c();

        // display geometry: thumb clamped to a grabbable 24px minimum.
        // The position goes through `strip_scroll_pos` (negated out of
        // gpui's element-offset domain) and is clamped to [0, max]
        // FIRST — before paint's own clamp-writeback lands, the stored
        // value can sit outside [-max, 0], and an unclamped ratio sent
        // the thumb sliding out of the rail, over the sidebar ("the
        // scrollbar scrolled away").
        let (thumb_w, usable) = rail_thumb_geometry(handle);
        let thumb_x = if max_x > px(0.) {
            // negated into the positive domain first: the stored
            // offset is <= 0, so clamping the raw value against
            // [0, max] pinned the thumb to the rail's left end
            let effective = strip_scroll_pos(handle).clamp(px(0.), max_x);
            usable * (effective / max_x)
        } else {
            px(0.)
        };

        let scrub = cx.listener(
            move |this: &mut Self, event: &DragMoveEvent<RailScrub>, _, cx: &mut Context<Self>| {
                // on_drag_move has no hover filter — self-filter by
                // pointer-in-bounds (the grips' note in
                // `toolbar_preview`)
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let pointer = event.event.position;
                let rail = event.bounds;
                this.scrub_rail_to(rail, pointer, cx);
            },
        );

        div()
            .id("preview-scrollbar")
            // zero-height when the bar fits — no empty gutter under a
            // full-width row; the first scroll event reveals the rail
            .when(overflow, |rail| rail.mt(px(4.)).h(px(6.)))
            .when(!overflow, |rail| rail.h(px(0.)))
            .relative()
            .w_full()
            .on_drag(RailScrub, rail_scrub_ghost)
            .on_drag_move(scrub)
            .when(overflow, |rail| {
                rail.child(
                    // faint full-width track
                    div()
                        .absolute()
                        .left_0()
                        .top(px(2.))
                        .w_full()
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(rgba((t.toolbar_border & 0xFFFFFF00) | 0x59)),
                )
                .child(
                    // the thumb: btn_text at 60% alpha, full on hover
                    div()
                        .id("preview-scrollbar-thumb")
                        .absolute()
                        .top(px(0.))
                        .left(thumb_x)
                        .w(thumb_w)
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(rgba((t.btn_text & 0xFFFFFF00) | 0x99))
                        .hover(|s| s.bg(rgba(t.btn_text))),
                )
            })
    }

    /// Pointer → scroll offset, Slider-track semantics: the thumb
    /// centers under the pointer.
    fn scrub_rail_to(
        &mut self,
        rail: Bounds<Pixels>,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let handle = self.preview_scroll.clone();
        let max_x = handle.max_offset().x;
        if max_x <= px(0.) {
            return;
        }
        let (thumb_w, usable) = rail_thumb_geometry(&handle);
        let local_x = pointer.x - rail.origin.x;
        let fraction = ((local_x - thumb_w / 2.) / usable).clamp(0., 1.);
        set_strip_scroll_pos(&handle, px(fraction * max_x.as_f32()));
        cx.notify();
    }

    /// Appearance: theme base + per-field colors, live-applied to this
    /// window's own chrome.
    fn page_appearance(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("page-appearance")
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                crate::i18n::t!("settings.appearance.title").as_ref(),
                crate::i18n::t!("settings.appearance.blurb").as_ref(),
            ))
            .child(self.base_control(cx))
            .child(self.color_grid(cx))
            .child(self.opacity_row(cx))
            .child(self.appearance_footer(cx))
    }

    /// About: the identity card — hero logo, name and version, what
    /// this is, where it lives, who helped. One calm centered column;
    /// everything a support conversation needs.
    fn page_about(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let entity = cx.entity();
        // ghost link button — same vocabulary as the config-file
        // buttons, each handing a URL to the session
        let link = |label: String, url: String, entity: &gpui_kit::Entity<Self>| {
            let entity = entity.clone();
            Button::new(SharedString::from(url.clone()))
                .accessibility_label(label.clone())
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(rgba(t.btn_text))
                .border_1()
                .border_color(rgba(t.toolbar_border))
                .hover(|s| s.bg(rgba(t.toolbar_hover)))
                .focus_visible(|s| s.border_color(rgba(t.accent)))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |_, _| {
                        open_external(url.clone());
                    });
                })
                .child(label)
        };
        let repository = env!("CARGO_PKG_REPOSITORY");
        div()
            .id("page-about")
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .pt(px(48.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(img(ImageSource::Render(ABOUT_LOGO.clone())).size(px(128.)))
            .child(
                div()
                    .text_size(px(22.))
                    .text_color(rgba(t.btn_text))
                    .child("Shotori"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgba(t.btn_text))
                    .max_w(px(440.))
                    .text_center()
                    .line_height(relative(1.5))
                    .child(crate::i18n::t!("settings.about.tagline").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(6.))
                    .pt(px(6.))
                    .child(link(
                        crate::i18n::t!("settings.about.repository").to_string(),
                        repository.to_string(),
                        &entity,
                    ))
                    .child(link(
                        crate::i18n::t!("settings.about.issues").to_string(),
                        format!("{repository}/issues"),
                        &entity,
                    )),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .pt(px(4.))
                    .child(crate::i18n::t!("settings.about.license").to_string()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .max_w(px(440.))
                    .text_center()
                    .line_height(relative(1.5))
                    .pt(px(6.))
                    .child(crate::i18n::t!("settings.about.credits").to_string()),
            )
    }

    /// The base palette selector: Auto plus every preinstalled theme
    /// (see [`theme::load::BUILT_INS`]). A Confirm writes through and
    /// re-seeds the rows below — the pickers always show what the
    /// active base resolves to.
    fn base_control(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.appearance.base_palette").to_string()),
            )
            .child(
                div().w(px(280.)).child(
                    Select::new(&self.appearance.base_select)
                        .accessibility_label(
                            crate::i18n::t!("settings.appearance.base_palette").to_string(),
                        )
                        .menu_width(px(280.)),
                ),
            )
    }

    /// One row per [`ColorKey`]: label + picker showing the resolved
    /// color. Wraps two per line at the default width.
    fn color_grid(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        let mut grid = div().flex().flex_wrap().gap(px(4.));
        for (key, state) in &self.appearance.pickers {
            grid = grid.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .w(px(300.))
                    .py(px(3.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(t.btn_text))
                            .child(key.label()),
                    )
                    .child(ColorPicker::new(state).small()),
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
                                .bg(rgba(t.toolbar_border)),
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

    fn appearance_footer(&self, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(6.))
            .child(
                Button::new("appearance-reset")
                    .accessibility_label("Reset colors to the base palette")
                    .px(px(12.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(rgba(t.btn_text))
                    .border_1()
                    .border_color(rgba(t.toolbar_border))
                    .hover(|s| s.bg(rgba(t.toolbar_hover)))
                    .focus_visible(|s| s.border_color(rgba(t.accent)))
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.appearance_reset(window, cx));
                    })
                    .child(crate::i18n::t!("settings.appearance.reset_colors").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .text_right()
                    .text_size(px(11.))
                    // errors only — where the file lives is one click
                    // away (General's config buttons); an always-on
                    // path line was clutter
                    .when_some(self.appearance.error.clone(), |el, error| {
                        el.text_color(rgba(t.accent)).child(error)
                    }),
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
    fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
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
    fn appearance_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

#[cfg(test)]
mod tests {
    // NOT `use super::*` — the parent's `use gpui_kit::*` glob pulls in
    // gpui's test-support `test` macro, and re-globbing it here shadows
    // Rust's built-in #[test] and expands to the recursion limit (the
    // #[gpui_kit::test] trap; the kit's own docs say to import test
    // modules' types explicitly). ui/toolbar.rs dodges this the same way.
    use super::{set_strip_scroll_pos, strip_scroll_pos};
    use gpui_kit::{ScrollHandle, point, px};

    /// gpui stores a scroll container's offset NEGATED (0 = start,
    /// -max = end); the strip's wheel/thumb/scrub/edge-scroll code all
    /// reason in the positive domain. Pin the boundary helpers so a
    /// future refactor cannot quietly drop the sign flip — the strip
    /// once lost every horizontal scroll path at once that way.
    #[test]
    fn strip_scroll_helpers_flip_the_sign_domain() {
        let handle = ScrollHandle::new();
        // what the container natively holds after scrolling 120px in
        handle.set_offset(point(px(-120.), px(0.)));
        assert_eq!(strip_scroll_pos(&handle), px(120.));
        // and what a scrub/edge-scroll writes back out of 60px in
        set_strip_scroll_pos(&handle, px(60.));
        assert_eq!(handle.offset().x, px(-60.));
        assert_eq!(handle.offset().y, px(0.));
    }
}
