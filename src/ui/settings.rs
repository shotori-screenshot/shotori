//! # Settings window (`shotori settings`)
//!
//! A regular toplevel — the one shotori surface that is NOT a
//! layer-shell overlay: plain `WindowOptions` (system titlebar, the
//! compositor places it; the layer-shell rules — display_id pinning,
//! bare `open_window` — do not apply, and `base::Root` stays banned
//! regardless).
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
use gpui_kit::base::slider::{
    Slider, SliderEvent, SliderState, SliderThumb, SliderTrack, SliderValue,
};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuItem, SidebarToggleButton,
};
use gpui_kit::component::{Collapsible, Icon, Sizable, Theme, ThemeMode};
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
                        cx.new(|cx| SettingsWindow::new(window, cx))
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

/// Align the shell's design-system theme with the active shotori
/// theme: mode from the system appearance (the component theme ships
/// light/dark pairs of its own), accent from the shotori palette — the
/// sidebar then reads as one product with the overlay instead of a
/// foreign default blue. Shotori-side, [`theme::follow_system`] keeps
/// the overlay palettes switching in step.
fn sync_shell_theme(window: &mut Window, cx: &mut App) {
    theme::follow_system(window.appearance());
    let dark = matches!(
        window.appearance(),
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
    );
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    // Second `update`, after the mode load replaced the colors — the
    // documented order (Theme::update's doc).
    sync_shell_accent(cx);
}

/// Keep the shell accent in step with the shotori accent — called at
/// startup by [`sync_shell_theme`] and after every Appearance edit
/// (the accent may have just changed).
fn sync_shell_accent(cx: &mut App) {
    let accent: Hsla = rgba(theme::c().accent).into();
    let accent_text: Hsla = rgba(theme::c().accent_text()).into();
    Theme::update(cx, |t| {
        t.colors.accent = accent;
        t.colors.accent_foreground = accent_text;
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
    /// Collapsed navigation sidebar (icon-only rail). Session state —
    /// window-local, intentionally not persisted.
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
/// state each row renders — one picker entity per [`ColorKey`] and per
/// annotation swatch, one slider for dim opacity. Picker values are
/// the RESOLVED palette (what a capture would show), so a base switch
/// re-seeds every swatch through [`SettingsWindow::appearance_resync`].
struct AppearanceState {
    file: ThemeFile,
    path: Option<PathBuf>,
    error: Option<String>,
    pickers: Vec<(ColorKey, Entity<ColorPickerState>)>,
    annotation: Vec<Entity<ColorPickerState>>,
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
        Self {
            section: Section::Toolbar,
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            drag: None,
            appearance,
            sidebar_collapsed: false,
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

    /// OCR page: flip the upright-assume toggle. Flipping re-renders the
    /// readiness chips (the classifier file counts as needed or not).
    fn toggle_ocr_upright(&mut self, cx: &mut Context<Self>) {
        let next = !self.settings.ocr_assume_upright();
        self.settings.set_ocr_assume_upright(next);
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

    fn location(&self) -> String {
        match &self.path {
            Some(path) => {
                crate::i18n::t!("settings.location", path = path.display().to_string()).to_string()
            }
            None => crate::i18n::t!("settings.no_config_dir").to_string(),
        }
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
                    .text_color(if self.error.is_some() {
                        rgba(theme::c().accent)
                    } else {
                        rgba(theme::c().hint_text)
                    })
                    .child(self.error.clone().unwrap_or_else(|| self.location())),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.sidebar(cx))
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

/// The radio circle of an OCR model row: ring always, center dot only
/// when selected (hand-drawn like the scrollbar rail — a 14px visual
/// with explicit geometry cannot drift).
fn radio_dot(selected: bool) -> Div {
    let t = theme::c();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(rgba(if selected { t.accent } else { t.toolbar_border }))
        .when(selected, |s| {
            s.child(div().size(px(6.)).rounded_full().bg(rgba(t.accent)))
        })
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

impl SettingsWindow {
    /// The navigation sidebar: one item per section, the active page
    /// highlighted. Fixed-width for now — collapsing is a shell nicety
    /// the settings window does not need at three sections.
    fn sidebar(&self, cx: &mut Context<Self>) -> Sidebar<SidebarMenu> {
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
                this.sidebar_collapsed = !this.sidebar_collapsed;
                cx.notify();
            }));
        let mut identity = SidebarHeader::new().collapsed(collapsed);
        if collapsed {
            // icon-rail width only fits one control — keep the toggle,
            // drop the identity block
            identity = identity.child(toggle);
        } else {
            identity = identity
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(Icon::new(IconName::Camera).size_4())
                        .child(div().text_size(px(13.)).child("Shotori")),
                )
                .child(toggle);
        }
        Sidebar::new("settings-sidebar")
            .collapsible(true)
            .collapsed(collapsed)
            .header(identity)
            .footer(
                SidebarFooter::new()
                    .collapsed(collapsed)
                    .when(!collapsed, |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        )
                    }),
            )
            .child(
                SidebarMenu::new()
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
                    )),
            )
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
        };
        page = page.flex_1().min_w(px(0.));
        page
    }

    /// General: read-only facts about this installation plus the UI
    /// language picker (the one control here that changes something).
    fn page_general(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let theme_path = theme::load::config_path();
        let row = |label: String, value: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgba(t.hint_text))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgba(t.btn_text))
                        .child(value),
                )
        };
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
            .child(row(
                crate::i18n::t!("settings.general.version").to_string(),
                format!("shotori {}", env!("CARGO_PKG_VERSION")),
            ))
            .child(row(
                crate::i18n::t!("settings.general.settings_file").to_string(),
                self.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| crate::i18n::t!("settings.no_config_dir").to_string()),
            ))
            .child(row(
                crate::i18n::t!("settings.general.theme_file").to_string(),
                theme_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| {
                        crate::i18n::t!("settings.general.theme_builtin").to_string()
                    }),
            ))
            .child(self.language_rows(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.write_through").to_string()),
            )
    }

    /// The language picker: one radio row per [`Language`]. Picking one
    /// writes through AND flips this window's locale live — `t!` reads
    /// the process-global locale on every call, so the re-render alone
    /// switches every label (including the sidebar and the
    /// component-library texts gpui-component translates itself).
    fn language_rows(&self, cx: &Context<Self>) -> Div {
        let t = theme::c();
        let mut rows = div().flex().flex_col().gap(px(6.));
        for language in crate::model::settings::Language::ALL {
            let selected = self.settings.language() == language;
            let entity = cx.entity();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("lang-{}", language.id())))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(if selected { t.accent } else { t.toolbar_border }))
                    .when(selected, |s| s.bg(rgba(t.toolbar_hover)))
                    .hover(|s| s.bg(rgba(t.toolbar_hover)))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| this.select_language(language, cx));
                    })
                    .child(radio_dot(selected))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
                            .text_color(rgba(t.btn_text))
                            .child(language.label()),
                    ),
            );
        }
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
            .child(rows)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.language.blurb").to_string()),
            )
    }

    /// OCR: the model-set picker + the upright-assume toggle. Readiness
    /// chips are computed from the cache directory per render (a few
    /// stat calls — cheap), so they track downloads other processes
    /// make. Selecting a model without its files is allowed: the next
    /// OCR run offers the existing setup dialog with progress and
    /// cancellation (that hardened flow is reused, not duplicated).
    fn page_ocr(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let mut list = div().flex().flex_col().gap(px(6.));
        for model in OcrModel::ALL {
            list = list.child(self.ocr_model_row(model, cx));
        }
        div()
            .id("page-ocr")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                "OCR",
                crate::i18n::t!("settings.ocr_page.blurb").as_ref(),
            ))
            .child(list)
            .child(self.ocr_upright_row(cx))
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
            .child(
                div()
                    .text_size(px(11.))
                    .text_right()
                    .text_color(if self.error.is_some() {
                        rgba(t.accent)
                    } else {
                        rgba(t.hint_text)
                    })
                    .child(self.error.clone().unwrap_or_else(|| self.location())),
            )
    }

    /// One selectable model row: radio dot, name, languages · tier, and
    /// the readiness chip under the CURRENT upright toggle (a set that
    /// only lacks the classifier counts as ready when the toggle is on).
    fn ocr_model_row(&self, model: OcrModel, cx: &Context<Self>) -> Stateful<Div> {
        let selected = self.settings.ocr_model() == model;
        let ready = crate::ocr::set_ready(model, self.settings.ocr_assume_upright());
        let t = theme::c();
        let entity = cx.entity();
        div()
            .id(SharedString::from(format!("ocr-model-{}", model.id())))
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(if selected { t.accent } else { t.toolbar_border }))
            .when(selected, |s| s.bg(rgba(t.toolbar_hover)))
            .hover(|s| s.bg(rgba(t.toolbar_hover)))
            .on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| this.select_ocr_model(model, cx));
            })
            .child(radio_dot(selected))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgba(t.btn_text))
                            .child(model.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(t.hint_text))
                            .child(format!("{} · {}", model.languages(), model.tier())),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgba(if ready { t.hint_text } else { t.accent }))
                    .child(if ready {
                        crate::i18n::t!("settings.ocr_page.ready").to_string()
                    } else {
                        crate::i18n::t!("settings.ocr_page.downloads_first").to_string()
                    }),
            )
    }

    /// The upright-assume toggle row (checkbox visual, click to flip).
    fn ocr_upright_row(&self, cx: &Context<Self>) -> Stateful<Div> {
        let on = self.settings.ocr_assume_upright();
        let t = theme::c();
        let entity = cx.entity();
        div()
            .id("ocr-upright")
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(t.toolbar_border))
            .hover(|s| s.bg(rgba(t.toolbar_hover)))
            .on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| this.toggle_ocr_upright(cx));
            })
            .child(
                Icon::new(if on {
                    IconName::SquareCheckBig
                } else {
                    IconName::Square
                })
                .size_4()
                .text_color(rgba(if on { t.accent } else { t.hint_text })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgba(t.btn_text))
                            .child(crate::i18n::t!("settings.ocr_page.upright.title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(t.hint_text))
                            .child(crate::i18n::t!("settings.ocr_page.upright.body").to_string()),
                    ),
            )
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
                    let offset = handle.offset();
                    let max = handle.max_offset();
                    let next = (offset.x + delta).clamp(px(0.), max.x);
                    if next != offset.x {
                        handle.set_offset(point(next, offset.y));
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
            // Wheel over the strip + rail: notify so the drawn thumb
            // follows whatever the container applied, and consume the
            // event while there is x-overflow so nothing above this
            // block can grow a scroll and carry the bar away again
            // (defensive — the page itself no longer scrolls; see
            // `page_toolbar`).
            //
            // The clamp-writeback also NORMALIZES the stored offset:
            // the native wheel application accumulates raw deltas
            // without clamping (paint clamps visually, nothing writes
            // back), so overshooting an end parks the handle at, say,
            // -150px — and the first 150px of reverse scrolling are
            // DEAD (bar and thumb both refuse to move while the stored
            // value climbs back through the overshoot). This listener
            // runs after the application in bubble order; rewriting the
            // clamped value keeps storage, paint and the thumb locked
            // together, notch for notch.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                let handle = this.preview_scroll.clone();
                let max = handle.max_offset().x;
                if max > px(0.) {
                    cx.stop_propagation();
                }
                let offset = handle.offset();
                let clamped = point(offset.x.clamp(px(0.), max), offset.y);
                if clamped != offset {
                    handle.set_offset(clamped);
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
        // The offset is clamped to [0, max] FIRST: gpui's native wheel
        // application stores the raw delta without clamping (clamping
        // happens at paint, never written back), so wheeling past
        // either end leaves a NEGATIVE or over-max offset in the
        // handle — an unclamped ratio sent the thumb sliding out of
        // the rail, over the sidebar ("the scrollbar scrolled away").
        let (thumb_w, usable) = rail_thumb_geometry(handle);
        let thumb_x = if max_x > px(0.) {
            let effective = handle.offset().x.clamp(px(0.), max_x);
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
        let next = px(fraction * max_x.as_f32());
        handle.set_offset(point(next, handle.offset().y));
        cx.notify();
    }

    /// Appearance: theme base + per-field colors with a live mockup.
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
            // the fixed-geometry mockup would otherwise stretch the
            // column to its own 640px and every text above would lay
            // out (and clip) at that width — the scroll wrapper caps
            // its contribution, narrower panes pan (the vertical axis
            // never actually scrolls: the child is its own height)
            .child(
                div()
                    .id("mockup-wrap")
                    .self_start()
                    .overflow_x_scroll()
                    .child(appearance_mockup()),
            )
            .child(self.base_row(cx))
            .child(self.color_grid(cx))
            .child(self.opacity_row(cx))
            .child(self.annotation_row(cx))
            .child(self.appearance_footer(cx))
    }

    /// The base palette selector: four mutually exclusive buttons, the
    /// active one highlighted.
    fn base_row(&self, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        let current_base = self
            .appearance
            .file
            .base
            .clone()
            .unwrap_or_else(|| "auto".into());
        // wraps at narrow pane widths — the window has no min size, and
        // a row of fixed buttons must not stretch the page horizontally
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
        row = row.child(
            div()
                .w(px(110.))
                .text_size(px(12.))
                .text_color(rgba(t.hint_text))
                .child(crate::i18n::t!("settings.appearance.base_palette").to_string()),
        );
        for (name, label_key) in [
            ("auto", "settings.appearance.auto"),
            ("dark", "settings.appearance.dark"),
            ("light", "settings.appearance.light"),
            ("high_contrast", "settings.appearance.high_contrast"),
        ] {
            let active = current_base == name;
            let name = name.to_string();
            let entity = entity.clone();
            row = row.child(
                Button::new(SharedString::from(format!("base-{name}")))
                    .accessibility_label(format!(
                        "{}: {}",
                        crate::i18n::t!("settings.appearance.base_palette"),
                        crate::i18n::t!(label_key)
                    ))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(rgba(if active { t.accent_text() } else { t.btn_text }))
                    .when(active, |b| {
                        b.bg(rgba(t.accent)).border_1().border_color(rgba(t.accent))
                    })
                    .when(!active, |b| {
                        b.border_1().border_color(rgba(t.toolbar_border))
                    })
                    .when(!active, |b| b.hover(|s| s.bg(rgba(t.toolbar_hover))))
                    .focus_visible(|s| s.border_color(rgba(t.accent)))
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.appearance_set_base(&name, window, cx));
                    })
                    .child(crate::i18n::t!(label_key).to_string()),
            );
        }
        row
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

    /// The seven annotation colors (paint order, exported into PNGs).
    fn annotation_row(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        // wraps at narrow pane widths (no window min size)
        let mut row = div().flex().flex_wrap().items_center().gap(px(8.));
        row = row.child(
            div()
                .w(px(110.))
                .text_size(px(12.))
                .text_color(rgba(t.hint_text))
                .child(crate::i18n::t!("settings.appearance.annotations").to_string()),
        );
        for (name, state) in crate::ui::theme::PALETTE_NAMES
            .iter()
            .zip(&self.appearance.annotation)
        {
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(ColorPicker::new(state).small())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgba(t.hint_text))
                            .child(*name),
                    ),
            );
        }
        row
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
                    .text_color(if self.appearance.error.is_some() {
                        rgba(t.accent)
                    } else {
                        rgba(t.hint_text)
                    })
                    .child(self.appearance.error.clone().unwrap_or_else(|| {
                        match &self.appearance.path {
                            Some(p) => {
                                crate::i18n::t!("settings.location", path = p.display().to_string())
                                    .to_string()
                            }
                            None => crate::i18n::t!("settings.no_config_dir").to_string(),
                        }
                    })),
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

        let mut pickers = Vec::new();
        let mut annotation = Vec::new();
        let mut subs = Vec::new();
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
        for (ix, color) in current.annotation_colors.into_iter().enumerate() {
            let state =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value(u32_to_hsla(color)));
            let sub = cx.subscribe(&state, move |this, _, event, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    this.set_annotation(ix, hsla_to_u32(*color));
                    this.appearance_apply(cx);
                }
            });
            annotation.push(state);
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
                this.appearance.file.dim_opacity = Some(*value);
                this.appearance_apply(cx);
            }
        }));
        Self {
            file,
            path,
            error: errors.first().cloned(),
            pickers,
            annotation,
            opacity,
            _subscriptions: subs,
        }
    }
}

impl SettingsWindow {
    /// Set one annotation color override. The file format is
    /// all-or-nothing (a partial list does not load), so the first
    /// edit seeds the whole array from the resolved palette and this
    /// edit lands inside it.
    fn set_annotation(&mut self, ix: usize, color: u32) {
        let current = current_palette(&self.appearance.file);
        let list = self
            .appearance
            .file
            .annotation_colors
            .get_or_insert_with(|| {
                current
                    .annotation_colors
                    .iter()
                    .map(|c| load::to_hex(*c))
                    .collect()
            });
        list[ix] = load::to_hex(color);
    }

    /// Apply the working theme live — the settings window's own chrome
    /// re-skins, previewing the next capture's palette — and write
    /// theme.toml through.
    fn appearance_apply(&mut self, cx: &mut Context<Self>) {
        let (dark, light, palette_errors) = self.appearance.file.palettes();
        theme::set(dark, light);
        sync_shell_accent(cx);
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
        for (ix, state) in self.appearance.annotation.iter().enumerate() {
            state.update(cx, |state, cx| {
                state.set_value(u32_to_hsla(current.annotation_colors[ix]), window, cx)
            });
        }
        self.appearance.opacity.update(cx, |state, cx| {
            state.set_value(current.dim_opacity, window, cx)
        });
    }

    fn appearance_set_base(&mut self, base: &str, window: &mut Window, cx: &mut Context<Self>) {
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

/// gpui's 0xRRGGBBAA → picker color.
fn u32_to_hsla(color: u32) -> Hsla {
    rgba(color).into()
}

/// Picker color → theme storage. Alpha forced opaque: the file format
/// demands opaque colors and the pickers cannot meaningfully edit a
/// theme alpha (only dim_opacity is a knob, and it has its own slider).
fn hsla_to_u32(color: Hsla) -> u32 {
    let c: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (byte(c.r) << 24) | (byte(c.g) << 16) | (byte(c.b) << 8) | 0xFF
}

// ── Appearance: the virtual-screenshot mockup ──────────────────────────────

/// The Appearance preview: a hand-drawn capture scene — fake desktop,
/// dim + selection cutout with accent chrome, sample annotations, a
/// schematic toolbar with the swatch strip, an OCR chip and a pin —
/// painted with the ACTIVE (working) palette. Every themed surface of
/// the overlay appears once, so any picker drag shows up somewhere in
/// the scene the same frame. Fixed geometry keeps the pickers from
/// reflowing under the preview while dragging.
const MOCK_W: f32 = 640.;
const MOCK_H: f32 = 340.;
const SEL_X: f32 = 190.;
const SEL_Y: f32 = 92.;
const SEL_W: f32 = 250.;
const SEL_H: f32 = 148.;

fn appearance_mockup() -> Stateful<Div> {
    let t = theme::c();
    // absolute helper: a positioned box
    fn at(x: f32, y: f32, w: f32, h: f32) -> Div {
        div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
    }
    // one resize handle: accent square with a hair of contrast rim
    let handle = |x: f32, y: f32| {
        at(x, y, 7., 7.)
            .bg(rgba(t.accent))
            .border_1()
            .border_color(rgba(t.toolbar_bg))
    };
    div()
        .id("appearance-mockup")
        .relative()
        .self_start()
        .w(px(MOCK_W))
        .h(px(MOCK_H))
        .rounded(px(10.))
        .overflow_hidden()
        .border_1()
        .border_color(rgba(t.toolbar_border))
        // ── the fake desktop (theme-independent: it is the wallpaper,
        //    not UI) …
        .child(
            div()
                .absolute()
                .size_full()
                .bg(rgb(0x5F6F82FF))
                .child(at(28., 34., 130., 86.).rounded(px(6.)).bg(rgb(0x748699FF)))
                .child(
                    at(28., 34., 130., 16.)
                        .rounded_t(px(6.))
                        .bg(rgb(0x445467FF)),
                )
                .child(
                    at(470., 26., 150., 110.)
                        .rounded(px(6.))
                        .bg(rgb(0x6B7D90FF))
                        .border_1()
                        .border_color(rgb(0x556879FF)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .w_full()
                        .h(px(24.))
                        .bg(rgb(0x42515FFF)),
                ),
        )
        // … dimmed outside the selection (theme: dim_color @ dim_opacity)
        .child(div().absolute().size_full().bg(rgba(t.dim())))
        // ── the selection: a bright "cutout" with accent chrome …
        .child(
            at(SEL_X, SEL_Y, SEL_W, SEL_H)
                .bg(rgb(0x8CA0B3FF))
                .border_2()
                .border_color(rgba(t.accent))
                // sample annotations: rectangle (green), stroke (orange),
                // number badge (blue) — three palette colors visible
                .child(
                    at(18., 20., 92., 58.)
                        .border_2()
                        .border_color(rgba(t.annotation_colors[3])),
                )
                .child(
                    at(130., 96., 92., 4.)
                        .rounded(px(2.))
                        .bg(rgba(t.annotation_colors[1])),
                )
                .child(
                    at(150., 26., 22., 22.)
                        .rounded_full()
                        .bg(rgba(t.annotation_colors[4]))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(rgb(0xFFFFFFFF))
                        .child("1"),
                )
                // eight resize handles on the rim
                .child(handle(-4., -4.))
                .child(handle(SEL_W / 2. - 3.5, -4.))
                .child(handle(SEL_W - 3., -4.))
                .child(handle(-4., SEL_H / 2. - 3.5))
                .child(handle(SEL_W - 3., SEL_H / 2. - 3.5))
                .child(handle(-4., SEL_H - 3.))
                .child(handle(SEL_W / 2. - 3.5, SEL_H - 3.))
                .child(handle(SEL_W - 3., SEL_H - 3.)),
        )
        // size chip above the selection (accent + accent_text)
        .child(
            at(SEL_X, SEL_Y - 22., 76., 18.)
                .rounded(px(4.))
                .bg(rgba(t.accent))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(rgba(t.accent_text()))
                .child(format!("{} × {}", SEL_W as u32, SEL_H as u32)),
        )
        // schematic toolbar under the selection (toolbar_* + swatches)
        .child(at(SEL_X, SEL_Y + SEL_H + 10., 430., 38.).child(mock_toolbar()))
        // OCR setup chip (chip_bg / hint_text / btn_*)
        .child(
            div()
                .absolute()
                .left(px(24.))
                .bottom(px(40.))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(rgba(t.chip_bg))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgba(t.hint_text))
                        .child(crate::i18n::t!("ocr.setup.downloading").to_string()),
                )
                .child(
                    div()
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(rgba(t.btn_hover_bg))
                        .text_size(px(11.))
                        .text_color(rgba(t.btn_text))
                        .child(crate::i18n::t!("ocr.setup.retry").to_string()),
                ),
        )
        // pin thumbnail (pin_border)
        .child(
            at(MOCK_W - 96., 26., 72., 48.)
                .rounded(px(4.))
                .bg(rgb(0x8CA0B3FF))
                .border_1()
                .border_color(rgba(t.pin_border)),
        )
}

/// The mockup's toolbar: schematic chrome, real palette — grips, two
/// plain buttons, a selected one, a separator, then all seven
/// annotation swatches with their border.
fn mock_toolbar() -> Div {
    let t = theme::c();
    let dot = |color: u32| div().size(px(8.)).rounded_full().bg(rgba(color));
    let mut bar = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .h(px(38.))
        .px(px(8.))
        .rounded(px(9.))
        .bg(rgba(t.toolbar_bg))
        .border_1()
        .border_color(rgba(t.toolbar_border))
        .shadow_md()
        .child(dot(t.toolbar_border))
        .child(dot(t.toolbar_border))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .child(dot(t.toolbar_text)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .bg(rgba(t.toolbar_selected))
                .border_1()
                .border_color(rgba(t.accent))
                .child(dot(t.accent)),
        )
        .child(div().w(px(1.)).h(px(16.)).bg(rgba(t.toolbar_border)));
    for color in t.annotation_colors {
        bar = bar.child(
            div()
                .size(px(11.))
                .rounded(px(3.))
                .bg(rgba(color))
                .border_1()
                .border_color(rgba(t.swatch_border)),
        );
    }
    bar.child(dot(t.toolbar_border))
        .child(dot(t.toolbar_border))
}
