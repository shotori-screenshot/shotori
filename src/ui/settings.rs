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
use gpui_kit::base::Button;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{Icon, Theme, ThemeMode};
use gpui_kit::*;

use crate::model::settings::{Settings, ToolbarButton, ToolbarCluster, ToolbarItem};
use crate::ui::theme;
use crate::ui::toolbar;

gpui_kit::actions!([CloseSettings]);

const WINDOW_W: f32 = 1024.;
const WINDOW_H: f32 = 640.;
const PAGE_PAD: f32 = 24.;

/// One page of the settings window. The sidebar switches `section`;
/// each page owns the state it needs on [`SettingsWindow`] (today only
/// the toolbar editor has any — future sections add fields alongside).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    General,
    Toolbar,
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
        // Compositors that tile new toplevels (niri's default) may
        // still squeeze below the requested bounds; the min size keeps
        // the toolbar page usable, and its horizontal scroll is the
        // floor below that.
        window_min_size: Some(size(px(940.), px(540.))),
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
    focus: FocusHandle,
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
        Self {
            section: Section::Toolbar,
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            drop_target: None,
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
            // hug the row like the real toolbar does — the page column
            // would otherwise stretch the bar full-width, inflating the
            // flexible gaps into wide slabs the real bar never shows
            .self_start()
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
                    entity.update(cx, |this, cx| this.remove_at(src, cx));
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
        Sidebar::new("settings-sidebar").collapsible(false).child(
            SidebarMenu::new()
                .child(item("General", IconName::Settings2, Section::General))
                .child(item("Toolbar", IconName::PanelTop, Section::Toolbar)),
        )
    }

    /// The content pane: the active section's page. This match is the
    /// section registry — a new section is a variant, a page fn and a
    /// sidebar item above.
    fn page(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        match self.section {
            Section::General => self.page_general(cx),
            Section::Toolbar => self.page_toolbar(cx),
        }
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

    /// Toolbar: the drag-and-drop editor. Both-axis scroll: the preview
    /// bar hugs its content (703px at the factory row) and a tiled
    /// window narrower than that must pan, not clip.
    fn page_toolbar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        div()
            .id("page-toolbar")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_scroll()
            // header
            .child(page_header(
                "Toolbar",
                "Arrange the selection toolbar. Changes apply the next time you take a \
                 screenshot.",
            ))
            // the live preview
            .child(self.toolbar_preview(cx))
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
}
