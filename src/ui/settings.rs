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
//! every change writes straight through to `settings.toml` (see
//! [`crate::model::settings`]) and the NEXT capture picks it up —
//! crash-safe by construction, no OK button, no dirty state.
//!
//! v1 section: the selection toolbar's layout, edited by dragging —
//! a live preview bar (the real render pieces: [`crate::ui::toolbar`])
//! plus a palette of the controls not currently placed. Drag between
//! the two in either direction, drag inside the bar to reorder, click
//! to remove/append. gpui's native drag-and-drop carries the payload;
//! the drop slots between the preview items only exist while a drag is
//! in flight, so the bar visibly opens up where an item would land.

use std::path::PathBuf;

use gpui_kit::base::Button;
use gpui_kit::*;

use crate::model::settings::{Settings, ToolbarButton, ToolbarCluster, ToolbarItem};
use crate::ui::theme;
use crate::ui::toolbar;

gpui_kit::actions!([CloseSettings]);

const WINDOW_W: f32 = 800.;
const WINDOW_H: f32 = 560.;
const PAGE_PAD: f32 = 24.;

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
    // Wide enough for the factory row (~670px) to preview without
    // squeezing; the palette below wraps, so height is forgiving.
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
        focus: true,
        ..Default::default()
    }
}

struct SettingsWindow {
    /// Working copy of the user's settings; every change writes
    /// through to `path` immediately.
    settings: Settings,
    /// Where settings.toml lives (None when no config dir resolves).
    path: Option<PathBuf>,
    /// The last save's failure (or load warning), shown inline until
    /// the next successful save.
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // follow the system palette like the overlay does
        theme::follow_system(window.appearance());
        let (settings, warnings) = Settings::load(crate::model::settings::config_path().as_deref());
        for warning in &warnings {
            eprintln!("[shotori] settings: {warning}");
        }
        let subscriptions = vec![window.observe_window_appearance(|window, cx| {
            theme::follow_system(window.appearance());
            cx.refresh_windows();
        })];
        Self {
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Install a row and write it through. Rows built here are valid
    /// by construction (buttons can only come from the palette, which
    /// never offers one that is already placed).
    fn commit(&mut self, row: Vec<ToolbarItem>, cx: &mut Context<Self>) {
        self.settings.set_row(row);
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
    /// (remove); while a drag is in flight, drop slots appear between
    /// the items — the bar visibly opens up where an item would land.
    fn toolbar_preview(&self, cx: &Context<Self>) -> Div {
        let row: Vec<ToolbarItem> = self.settings.row().to_vec();
        let dragging = cx.has_active_drag();
        let mut bar = toolbar::bar().child(grip_end("pv-grip-left"));
        for (i, item) in row.iter().enumerate() {
            if dragging {
                bar = bar.child(self.drop_slot(i, cx));
            }
            bar = bar.child(self.preview_item(i, *item, cx));
        }
        if dragging {
            bar = bar.child(self.drop_slot(row.len(), cx));
        }
        bar.child(grip_end("pv-grip-right"))
    }

    /// One preview item: the real visual, but dragging reorders and
    /// clicking removes — the action itself never fires here.
    fn preview_item(&self, i: usize, item: ToolbarItem, cx: &Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let payload = DraggedItem {
            item,
            from: Some(i),
        };
        match item {
            ToolbarItem::Button(button) => match toolbar::meta_for(button) {
                Some(meta) => {
                    let remove = entity.clone();
                    toolbar::preview_control(meta, &self.focus, move |_, cx| {
                        remove.update(cx, |this, cx| this.remove_at(i, cx));
                    })
                    .on_drag(payload, ghost_constructor())
                    .into_any_element()
                }
                // unreachable: BUTTONS covers every ToolbarButton
                None => div().into_any_element(),
            },
            ToolbarItem::Separator => layout_visual(
                "pv-separator",
                payload,
                entity,
                i,
                div()
                    .w(px(2.))
                    .h(px(18.))
                    .bg(rgba(theme::c().toolbar_border)),
            ),
            ToolbarItem::Spacer => layout_visual(
                "pv-spacer",
                payload,
                entity,
                i,
                div()
                    .flex_1()
                    .min_w(px(56.))
                    .h(px(22.))
                    .rounded(px(4.))
                    .bg(rgba(theme::c().toolbar_hover))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgba(theme::c().hint_text))
                    .child("⟷"),
            ),
        }
    }

    /// An insertion point between preview items. Only rendered while a
    /// compatible drag is active (otherwise it would take layout space
    /// and shift the preview); the whole slot tints when the payload
    /// hovers it — deliberately NOT resized, a hitbox that shrinks
    /// under the pointer drops the drag — and the drop commits at its
    /// index.
    fn drop_slot(&self, i: usize, cx: &Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id(("pv-drop", i))
            .w(px(16.))
            .h(px(crate::model::placement::ROW_H - 8.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.))
            .child(
                div()
                    .w(px(3.))
                    .h(px(20.))
                    .rounded(px(2.))
                    .bg(rgba((theme::c().toolbar_border & 0xFFFFFF00) | 0x50)),
            )
            .drag_over::<DraggedItem>(|s, _, _, _| {
                s.bg(rgba((theme::c().accent & 0xFFFFFF00) | 0x99))
            })
            .on_drop(move |dragged: &DraggedItem, _, cx| {
                entity.update(cx, |this, cx| this.drop_at(i, dragged, cx));
            })
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
        // layout items are always offered — spacers/separators stack
        chips = chips.child(self.palette_chip(ToolbarItem::Spacer, cx));
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
            .on_drag(payload, ghost_constructor())
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

/// A bar-end grip for the preview: the real texture, non-interactive.
fn grip_end(id: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .px(px(crate::model::placement::BAR_PAD))
        .child(toolbar::grip_texture())
}

/// The drag-ghost constructor gpui calls when a drag starts.
fn ghost_constructor()
-> impl Fn(&DraggedItem, Point<Pixels>, &mut Window, &mut App) -> gpui_kit::Entity<DragGhost> + 'static
{
    |item: &DraggedItem, _, _, cx| cx.new(|_| DragGhost { item: item.item })
}

/// Wrap a preview layout item (separator / spacer) in the interaction
/// shell the buttons get from `preview_control`: drag to reorder,
/// click to remove. The visual sits centered in a wider hit target —
/// a 2px separator is un-grabbable on its own.
fn layout_visual(
    id: &'static str,
    payload: DraggedItem,
    entity: gpui_kit::Entity<SettingsWindow>,
    i: usize,
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
        .on_drag(payload, ghost_constructor())
        .on_click(move |_, _, cx| {
            entity.update(cx, |this, cx| this.remove_at(i, cx));
        })
        .child(visual)
        .into_any_element()
}

/// The icon an item shows outside the button chrome (ghost, palette
/// chip): the toolbar's own glyphs for buttons, simple marks for the
/// layout items.
fn icon_visual(item: ToolbarItem) -> AnyElement {
    match item {
        ToolbarItem::Button(button) => match toolbar::meta_for(button) {
            Some(meta) => meta.icon.svg().into_any_element(),
            None => div().into_any_element(),
        },
        ToolbarItem::Spacer => div()
            .w(px(18.))
            .h(px(2.))
            .bg(rgba(theme::c().toolbar_border))
            .into_any_element(),
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
        ToolbarItem::Spacer => "Flexible gap".into(),
        ToolbarItem::Separator => "Separator".into(),
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::c();
        div()
            .id("settings-root")
            .size_full()
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
            .child(
                div()
                    .id("settings-body")
                    .size_full()
                    .flex()
                    .flex_col()
                    .p(px(PAGE_PAD))
                    .gap(px(10.))
                    .overflow_y_scroll()
                    // header
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(rgba(t.btn_text))
                                    .child("Toolbar"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgba(t.hint_text))
                                    .child(
                                        "Arrange the selection toolbar. Changes apply the next \
                                         time you take a screenshot.",
                                    ),
                            ),
                    )
                    // the live preview
                    .child(self.toolbar_preview(cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(t.hint_text))
                            .child(
                                "Drag a control to move it · drop it here or on the palette to \
                                 remove it · click to remove / append · hidden controls keep \
                                 their keyboard shortcuts",
                            ),
                    )
                    // the palette
                    .child(self.palette(cx))
                    .child(div().flex_1())
                    .child(self.footer(cx)),
            )
    }
}
