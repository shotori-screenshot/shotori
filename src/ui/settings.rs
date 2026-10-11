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
//! v1 sections: General (read-only facts) and the selection toolbar's
//! layout, edited by dragging — a live preview bar (the real render
//! pieces: [`crate::ui::toolbar`]) plus a palette of the controls not
//! currently placed. Drag between the two in either direction, drag
//! inside the bar to reorder, click to remove/append. gpui's native
//! drag-and-drop carries the payload; the drop slots between the
//! preview items only exist while a drag is in flight, so the bar
//! visibly opens up where an item would land.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::base::AutoScroll;
use gpui_kit::base::Button;
use gpui_kit::base::slider::{
    Slider, SliderEvent, SliderState, SliderThumb, SliderTrack, SliderValue,
};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuItem, SidebarToggleButton,
};
use gpui_kit::component::{Collapsible, Icon, Sizable, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::settings::{Settings, ToolbarButton, ToolbarCluster, ToolbarItem};
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
    Appearance,
}

/// The drag-and-drop payload: what is being carried and where it came
/// from (`None` = palette: a fresh insert; `Some(i)` = the preview
/// bar: a reorder that removes slot i first).
struct DraggedItem {
    item: ToolbarItem,
    from: Option<usize>,
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
            title: Some("Shotori Settings".into()),
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
    /// Where a drag in flight would land (insertion slot), for the
    /// accent caret. `None` outside drags.
    drop_target: Option<usize>,
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
            drop_target: None,
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
    /// invalidates a recorded drop target.
    fn commit(&mut self, row: Vec<ToolbarItem>, cx: &mut Context<Self>) {
        self.settings.set_row(row);
        self.drop_target = None;
        self.persist(cx);
    }

    /// Write-through save: the file is the single source of truth.
    fn persist(&mut self, cx: &mut Context<Self>) {
        self.error = match &self.path {
            Some(path) => self.settings.save(path).err(),
            None => Some("no config directory (is HOME set?)".into()),
        };
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.commit(Settings::default().row().to_vec(), cx);
    }

    /// A drop landed in preview slot `i` (slot i sits before row item
    /// i; slot n = after the last item — slots are indexed against the
    /// row WITH the dragged item still in place, hence the -1 when
    /// moving right). The index math lives in the model, tested there.
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
            Some(path) => format!("Saves to {}", path.display()),
            None => "no config directory (is HOME set?)".into(),
        }
    }

    /// The live toolbar: the real bar container and grip textures with
    /// the configured row inside. Items drag (reorder) and click
    /// (remove). Drop targets are the items themselves (before/after
    /// by pointer vs the item's center — a full 30px target, not a
    /// strip); the grips pin the two ends, and the insertion point
    /// renders as a single accent caret. Nothing widens the bar while
    /// dragging — inserted slot strips once grew it by ~350px and
    /// pushed the right grip off the window (user-reported).
    fn toolbar_preview(&self, cx: &Context<Self>) -> Div {
        let row: Vec<ToolbarItem> = self.settings.row().to_vec();
        let dragging = cx.has_active_drag();
        let caret_at = if dragging { self.drop_target } else { None };
        let len = row.len();
        let pin_start = cx.listener(
            move |this: &mut Self,
                  event: &DragMoveEvent<DraggedItem>,
                  _,
                  cx: &mut Context<Self>| {
                // on_drag_move fires for EVERY element that registered a
                // listener during a matching drag — it has no hover filter
                // (it exists for drag sources tracking themselves, "like
                // resizing" per its docs). Self-filter by pointer-in-bounds,
                // or every listener overwrites the others and the caret
                // sticks wherever the last-registered element (the right
                // grip) pins it.
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                this.set_drop_target(0, cx);
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
                this.set_drop_target(len, cx);
            },
        );
        let bar_fallback = cx.listener(
            move |this: &mut Self, dragged: &DraggedItem, _, cx: &mut Context<Self>| {
                // gaps, padding and the caret itself: no item handled
                // the drop — land at the slot the pointer last aimed at
                if let Some(slot) = this.drop_target {
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
        for (i, item) in row.iter().enumerate() {
            if caret_at == Some(i) {
                bar = bar.child(insertion_caret());
            }
            bar = bar.child(self.preview_item(i, *item, cx));
        }
        if caret_at == Some(len) {
            bar = bar.child(insertion_caret());
        }
        bar.child(grip_end("pv-grip-right").on_drag_move(pin_end))
    }

    /// Record which insertion slot the pointer hovers; notifies only
    /// on change (this fires on every mouse move during a drag).
    fn set_drop_target(&mut self, slot: usize, cx: &mut Context<Self>) {
        if self.drop_target != Some(slot) {
            self.drop_target = Some(slot);
            cx.notify();
        }
    }

    /// One preview item: the real visual, but dragging reorders and
    /// clicking removes — the action itself never fires here.
    fn preview_item(&self, i: usize, item: ToolbarItem, cx: &Context<Self>) -> AnyElement {
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
                let center = event.bounds.origin.x + event.bounds.size.width / 2.;
                let slot = usize::from(event.event.position.x > center) + i;
                this.set_drop_target(slot, cx);
            },
        );
        let drop = cx.listener(
            move |this: &mut Self, dragged: &DraggedItem, _, cx: &mut Context<Self>| {
                let slot = this.drop_target.unwrap_or(i);
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
                "pv-separator",
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
                    .child("Not on the toolbar — drag onto the bar, or click to append"),
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
                    .accessibility_label("Reset to defaults")
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
                    .child("Reset to defaults"),
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

/// The accent caret marking where a drag would land. Purely visual —
/// 3px wide, so a drag in flight cannot push the bar's right edge off
/// the window (the slot strips it replaced cost ~16px per gap).
fn insertion_caret() -> Div {
    div()
        .w(px(3.))
        .h(px(26.))
        .rounded(px(2.))
        .bg(rgba(theme::c().accent))
        .flex_shrink_0()
}

/// Wrap a preview layout item (the separator) in the interaction
/// shell the buttons get from `preview_control`: drag to reorder,
/// hover to aim the caret, click to remove. The visual sits centered
/// in a wider hit target — a 2px separator is un-grabbable on its
/// own.
fn layout_visual(
    id: &'static str,
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
            Some(meta) => meta.name.to_string(),
            None => button.id().to_string(),
        },
        ToolbarItem::Separator => "Separator".into(),
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
        let item = |label: &'static str, icon: IconName, target: Section| {
            let selected = active == target;
            let click = cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                if this.section != target {
                    this.section = target;
                    cx.notify();
                }
            });
            SidebarMenuItem::new(label)
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
                "Expand navigation"
            } else {
                "Collapse navigation"
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
                    .child(item("General", IconName::Settings2, Section::General))
                    .child(item("Toolbar", IconName::PanelTop, Section::Toolbar))
                    .child(item("Appearance", IconName::Palette, Section::Appearance)),
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
            Section::Appearance => self.page_appearance(cx),
        };
        page = page.flex_1().min_w(px(0.));
        page
    }

    /// General: read-only facts about this installation. No toggles
    /// yet — every behavior shotori has lives in a real file with a
    /// purpose-built editor elsewhere in this window; placeholders
    /// pretending to be settings would be worse than none.
    fn page_general(&self, _: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let theme_path = theme::load::config_path();
        let row = |label: &str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgba(t.hint_text))
                        .child(label.to_string()),
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
                "General",
                "About this installation and where its configuration lives.",
            ))
            .child(row(
                "Version",
                format!("shotori {}", env!("CARGO_PKG_VERSION")),
            ))
            .child(row(
                "Settings file",
                self.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "no config directory (is HOME set?)".into()),
            ))
            .child(row(
                "Theme file",
                theme_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "built-in theme (no file)".into()),
            ))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(
                        "Edits anywhere in this window write straight through to the files above — \
                     the next screenshot picks them up.",
                    ),
            )
    }

    /// Toolbar: the drag-and-drop editor. Only the preview strip scrolls
    /// (horizontally, when the bar outgrows the pane); the header, hint,
    /// palette and footer stay put. The page itself keeps a vertical
    /// scroll for genuinely short windows.
    fn page_toolbar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        div()
            .id("page-toolbar")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            // header
            .child(page_header(
                "Toolbar",
                "Arrange the selection toolbar. Changes apply the next time you take a \
                 screenshot.",
            ))
            // the live preview, in its own horizontally-scrolling strip
            .child(self.preview_strip(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(
                        "Drag a control to move it · drop it here or on the palette to remove \
                         it · click to remove / append · hidden controls keep their keyboard \
                         shortcuts",
                    ),
            )
            // the palette
            .child(self.palette(cx))
            .child(div().flex_1())
            .child(self.footer(cx))
    }

    /// The preview bar's scroll viewport, with its scrollbar BELOW it —
    /// outside the scrolled content, never over the icons. The bar hugs
    /// its content (the factory row is ~703px — the box follows the
    /// element, never the pane) and pans when the pane is narrower;
    /// dragging into an edge zone pans with the drag (classic edge
    /// auto-scroll: park the item at an edge and the strip keeps moving
    /// under it).
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
        // The scrollbar lays itself out as an absolute overlay of its
        // parent (relative(1.) sizing — see its Element impl), so this
        // row is its mounting box. Always mounted and exactly one thumb
        // thick: the pill rides flush against the bar's bottom edge —
        // attached to the OUTSIDE of the toolbar, never over its icons —
        // and being unconditional, it cannot get out of step with the
        // layout (a conditionally-mounted row only re-evaluated on the
        // next unrelated render, so after a resize the scrollbar would
        // go missing until something else repainted the page).
        div()
            .flex()
            .flex_col()
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
            .child(
                div()
                    .id("preview-scrollbar-row")
                    .relative()
                    .w_full()
                    .h(px(8.))
                    // viewport comes from the scroll handle itself (the
                    // default): exactly the strip above
                    .child(Scrollbar::horizontal(&self.preview_scroll).id("preview-scrollbar")),
            )
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
                "Appearance",
                "Color the capture overlay — the mockup previews the palette a screenshot \
                 would use; every change applies live and writes theme.toml.",
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
                .child("Base palette"),
        );
        for (name, label) in [
            ("auto", "Auto"),
            ("dark", "Dark"),
            ("light", "Light"),
            ("high_contrast", "High contrast"),
        ] {
            let active = current_base == name;
            let name = name.to_string();
            let entity = entity.clone();
            row = row.child(
                Button::new(SharedString::from(format!("base-{name}")))
                    .accessibility_label(format!("Base palette: {label}"))
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
                    .child(label),
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
                    .child("Dim opacity"),
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
                .child("Annotations"),
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
                    .child("Reset to base"),
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
                            Some(p) => format!("Saves to {}", p.display()),
                            None => "no config directory (is HOME set?)".into(),
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
                None => Some("no config directory (is HOME set?)".into()),
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
                        .child("Downloading OCR model…"),
                )
                .child(
                    div()
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(rgba(t.btn_hover_bg))
                        .text_size(px(11.))
                        .text_color(rgba(t.btn_text))
                        .child("Retry"),
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
