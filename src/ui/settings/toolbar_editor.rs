//! The Toolbar page: the drag-and-drop layout editor. The live
//! preview bar (the real bar container with the configured row), the
//! palette of unplaced controls, and the hand-drawn scrollbar rail —
//! plus the drag vocabulary they share (payload, preview mirror,
//! ghost, insertion slot). The index math for a drop lives in the
//! model (`Settings::apply_drop`); everything here is presentation
//! and gesture plumbing around it.

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use crate::model::settings::{Settings, ToolbarCluster, ToolbarItem};
use crate::ui::theme;
use crate::ui::toolbar;

use super::{Button, SettingsWindow, card_group};

mod rail;
use rail::{edge_scroll_delta, set_strip_scroll_pos, strip_scroll_pos};

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
pub(super) struct DragPreview {
    item: ToolbarItem,
    origin: Option<usize>,
    slot: usize,
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
            .text_sm()
            .text_color(rgba(t.toolbar_text))
            .child(icon_visual(self.item))
            .child(item_name(self.item))
    }
}

impl SettingsWindow {
    /// Restore the factory toolbar layout without changing capture preferences.
    pub(super) fn reset_toolbar(&mut self, cx: &mut Context<Self>) {
        self.commit(Settings::default().row().to_vec(), cx);
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
                    toolbar::preview_control(
                        meta,
                        self.settings.keys(),
                        &self.focus,
                        move |_, cx| {
                            remove.update(cx, |this, cx| this.remove_at(i, cx));
                        },
                    )
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
    /// Chips drag onto the bar or append on click. The palette area is
    /// also the remove target: dropping a bar item here takes it off.
    fn palette(&self, cx: &Context<Self>) -> Stateful<Div> {
        let mut chips = div().flex().flex_wrap().gap_2();
        for cluster in [
            ToolbarCluster::Tools,
            ToolbarCluster::Actions,
            ToolbarCluster::Confirm,
        ] {
            for meta in toolbar::BUTTONS.iter().filter(|meta| {
                meta.button.cluster() == cluster
                    && !self
                        .settings
                        .row()
                        .contains(&ToolbarItem::Button(meta.button))
            }) {
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
            .border_color(cx.theme().transparent)
            .child(chips)
    }

    fn palette_chip(&self, item: ToolbarItem, cx: &Context<Self>) -> Button {
        let entity = cx.entity();
        let payload = DraggedItem { item, from: None };
        Button::new(SharedString::from(format!("pv-chip-{}", item.id())))
            .accessibility_label(item_name(item))
            .flex()
            .items_center()
            .gap_2()
            .h_9()
            .px_3()
            .w(rems(10.5))
            .max_w_full()
            .rounded(cx.theme().radius)
            .text_sm()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().foreground)
            .hover(|s| s.bg(cx.theme().secondary_hover))
            .focus_visible(|s| s.border_1().border_color(cx.theme().ring))
            .on_drag(payload, ghost_constructor)
            .on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| {
                    this.append(item, cx);
                });
            })
            .child(icon_visual(item))
            .child(item_name(item))
    }

    /// Attach the toolbar drag-and-drop page-level fallback and rail scrub
    /// listeners to the Capture & Toolbar page container.
    pub(super) fn attach_toolbar_drag_handlers(
        &self,
        page: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
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
        let rail_scrub = cx.listener(Self::on_rail_scrub);
        page.on_drop(page_fallback).on_drag_move(rail_scrub)
    }

    /// The interactive drag-and-drop toolbar layout card (preview bar + palette).
    pub(super) fn toolbar_editor_card(&self, cx: &mut Context<Self>) -> Div {
        card_group(
            cx,
            None,
            vec![
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().group_box)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .justify_between()
                                    .gap_4()
                                    .child(group_heading(
                                        cx,
                                        "settings.toolbar_page.group.current",
                                        "settings.toolbar_page.hint",
                                    ))
                                    .child(super::shell::reset_button(
                                        "settings.toolbar_page.reset",
                                        cx,
                                        {
                                            let entity = cx.entity();
                                            move |_, _, cx| {
                                                entity
                                                    .update(cx, |this, cx| this.reset_toolbar(cx));
                                            }
                                        },
                                    )),
                            )
                            .child(self.preview_strip(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(group_heading(
                                cx,
                                "settings.toolbar_page.group.palette",
                                "settings.toolbar_page.palette_hint",
                            ))
                            .child(self.palette(cx)),
                    ),
            ],
        )
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
        //
        // The rail's existence is a LAYOUT decision (`h(0)` vs `h(9)`),
        // so it cannot be decided at paint time — but only a re-render
        // can change it, and a window resize re-lays-out without
        // rendering. Compare the freshly laid-out overflow flag with the
        // one the tree was built with and ask for that render; it
        // converges because the second pass matches.
        //
        // The ask goes through `window.defer`, NOT a bare `cx.notify`
        // here: we run inside prepaint, and a notify raised mid-draw is
        // dropped (`WindowInvalidator::invalidate_view` only invalidates
        // an idle window). Inline, the flag was updated but no frame
        // followed, so the gutter stayed expanded/collapsed until an
        // unrelated event (a hover, a scroll) forced the next render
        // (user-reported: "the reserved strip only clears once the mouse
        // moves"). Deferred, it lands right after the draw, before the
        // next frame is requested.
        let live = self.preview_scroll.clone();
        let entity = cx.entity();
        let sync = move |_: Vec<Bounds<Pixels>>, window: &mut Window, cx: &mut App| {
            let overflow = live.max_offset().x > px(0.);
            // `is_none_or`: the first prepaint must SEED the flag,
            // otherwise it stays `None` and the rail keeps deciding from
            // the stale handle value forever.
            let stale = entity
                .read(cx)
                .rail_has_overflow
                .is_none_or(|built| built != overflow);
            if stale {
                let entity = entity.clone();
                window.defer(cx, move |_, cx| {
                    entity.update(cx, |this, cx| {
                        this.rail_has_overflow = Some(overflow);
                        cx.notify();
                    });
                });
            }
        };
        div()
            .on_children_prepainted(sync)
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
            .on_scroll(cx.listener(|this, _: &ScrollEvent, _, cx| {
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

/// A group heading with an inline [`super::help_tooltip`] mark: the
/// usage hint rides in a hover tooltip instead of a permanent caption
/// line, so the page speaks with one voice per card.
fn group_heading(cx: &App, label_key: &'static str, hint_key: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(crate::i18n::t!(label_key).to_string())
        .child(super::help_tooltip(hint_key, cx))
}
