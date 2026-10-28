//! The pinned scene's element tree: one canvas (input region + drag
//! handling) plus one framed image per pin, the zoom/pan interactions
//! and the context menu. Split from pin.rs to keep the board/window
//! plumbing separate from what each frame paints.
use super::*;

impl Render for PinSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let board = self.board.clone();
        let output = self.output.bounds;
        let menu = board
            .read(cx)
            .menu
            .filter(|(_, position)| output.contains(position));
        let any_menu = board.read(cx).menu.is_some();
        let sink = canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                let regions: Vec<_> = if any_menu {
                    vec![Bounds::new(point(px(0.), px(0.)), output.size)]
                } else {
                    board
                        .read(cx)
                        .pins
                        .iter()
                        .filter_map(|pin| {
                            let slice = pin.rect.intersect(&output);
                            (slice.size.width > px(0.) && slice.size.height > px(0.))
                                .then_some(Bounds::new(slice.origin - output.origin, slice.size))
                        })
                        .collect()
                };
                window.set_input_region(Some(&regions));
                let moving = board.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && moving.read(cx).drag.is_some() {
                        moving.update(cx, |board, cx| {
                            board.move_drag(event.position + output.origin);
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble
                        && event.button == MouseButton::Left
                        && board.read(cx).drag.is_some()
                    {
                        board.update(cx, |board, cx| {
                            board.move_drag(event.position + output.origin);
                            board.drag = None;
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let pins: Vec<_> = self
            .board
            .read(cx)
            .pins
            .iter()
            .filter_map(|pin| {
                let slice = pin.rect.intersect(&output);
                if slice.size.width <= px(0.) || slice.size.height <= px(0.) {
                    return None;
                }
                let id = pin.id;
                let local = pin.rect.origin - output.origin;
                // The frame is drawn AROUND the image, not inside it: the
                // div is grown by the 1px border width on every side, so
                // the border box's CONTENT area is exactly the pin rect
                // and the bitmap lands pixel-perfect over the desktop it
                // froze. A border drawn inside the rect insets and
                // resamples the image — visibly dimmer, softened text
                // (border-box sizing).
                let frame = px(1.);
                Some(
                    div()
                        .id(("pin-image", id))
                        .debug_selector(move || format!("pin-image-{id}"))
                        .absolute()
                        .left(local.x - frame)
                        .top(local.y - frame)
                        .w(pin.rect.size.width + frame * 2.)
                        .h(pin.rect.size.height + frame * 2.)
                        .overflow_hidden()
                        // a thin ACCENT frame distinguishes the pin from
                        // the desktop beneath — the same orange as the
                        // overlay's selection border, solid because a pin
                        // must read against ANY content (palette-specific
                        // borders vanish on half the wallpapers). On the
                        // div itself: an absolute overlay child never
                        // painted (measured), the plain border box does
                        .border_1()
                        .border_color(rgba(crate::ui::theme::c().accent))
                        // neon halo: the accent blurred around the frame —
                        // a tight hot ring hugging the border plus a wide
                        // soft wash. Derived from the accent so custom
                        // themes glow in their own color. Drop shadows are
                        // painted before the element's own content mask
                        // (Style::paint), so the overflow_hidden above
                        // does not clip them
                        .shadow(vec![
                            BoxShadow::new(
                                px(0.),
                                px(0.),
                                rgba((crate::ui::theme::c().accent & 0xFFFFFF00) | 0x8C).into(),
                            )
                            .blur_radius(px(6.)),
                            BoxShadow::new(
                                px(0.),
                                px(0.),
                                rgba((crate::ui::theme::c().accent & 0xFFFFFF00) | 0x47).into(),
                            )
                            .blur_radius(px(24.)),
                        ])
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus_handle, cx);
                                this.board.update(cx, |board, cx| {
                                    board.raise(id);
                                    // raise(id) moves `id` to the back, so the back
                                    // exists here — but an input-path unwrap violates
                                    // the no-panic rule; bail on the impossible empty.
                                    let Some(pin) = board.pins.last() else { return };
                                    board.drag = Some((
                                        id,
                                        event.position + output.origin - pin.rect.origin,
                                    ));
                                    board.menu = None;
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus_handle, cx);
                                this.board.update(cx, |board, cx| {
                                    board.raise(id);
                                    board.drag = None;
                                    board.menu = Some((id, event.position + output.origin));
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }),
                        )
                        .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                            let lines = match event.delta {
                                ScrollDelta::Lines(lines) => lines.y,
                                ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 40.,
                            };
                            // zoom around the cursor, not the pin's
                            // corner — the point under the pointer
                            // stays under the pointer
                            let cursor = event.position + output.origin;
                            this.board.update(cx, |board, cx| {
                                board.zoom_at(id, lines, cursor);
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }))
                        .child(
                            // probe + pixel-alignment guard: this wrapper
                            // fills the border box's CONTENT area, which
                            // must equal the pin rect exactly — the frozen
                            // bitmap over the desktop it copied
                            div()
                                .size_full()
                                .debug_selector(move || format!("pin-img-{id}"))
                                .child(
                                    img(pin.image.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Fill),
                                ),
                        ),
                )
            })
            .collect();

        let menu_el = menu.map(|(_, position)| {
            let menu_size = size(
                px(MENU_W).min(output.size.width),
                px(MENU_H).min(output.size.height),
            );
            let local = position - output.origin;
            let origin = point(
                local.x.max(px(0.)).min(output.size.width - menu_size.width),
                local
                    .y
                    .max(px(0.))
                    .min(output.size.height - menu_size.height),
            );
            let item =
                |id: &'static str, label: &str, action: fn() -> Box<dyn gpui_kit::Action>| {
                    gpui_kit::base::Button::new(id)
                        .debug_selector(move || id.into())
                        .accessibility_label(label)
                        .w_full()
                        .h(px(MENU_ITEM_H))
                        .px_2()
                        .rounded_sm()
                        .text_sm()
                        .text_color(rgba(crate::ui::theme::c().toolbar_text))
                        .hover(|s| s.bg(rgba(crate::ui::theme::c().toolbar_hover)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                        .child(label.to_string())
                };
            div()
                .id("pin-menu")
                .debug_selector(|| "pin-menu".into())
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(menu_size.width)
                .h(menu_size.height)
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(rgba(crate::ui::theme::c().toolbar_border))
                .bg(rgba(crate::ui::theme::c().toolbar_bg))
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(item(
                    "pin-copy",
                    crate::i18n::t!("common.copy").as_ref(),
                    || Box::new(CopyPin),
                ))
                .child(item(
                    "pin-zoom",
                    crate::i18n::t!("pin.menu.reset_zoom").as_ref(),
                    || Box::new(ResetPinZoom),
                ))
                .child(item(
                    "pin-close",
                    crate::i18n::t!("common.close").as_ref(),
                    || Box::new(ClosePin),
                ))
        });
        // All outputs consume the dismissing click, including outputs without the menu.
        let backdrop = any_menu.then(|| {
            div()
                .id("pin-menu-backdrop")
                .absolute()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.board.update(cx, |board, cx| {
                            board.menu = None;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.board.update(cx, |board, cx| {
                            board.menu = None;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }),
                )
        });

        div()
            .id("shotori-pin")
            .overflow_hidden()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context(if menu.is_some() {
                "ShotoriPinMenu"
            } else {
                "ShotoriPin"
            })
            .on_action(cx.listener(|this, _: &DismissPinMenu, _, cx| {
                this.board.update(cx, |board, cx| {
                    board.menu = None;
                    cx.notify();
                });
            }))
            .on_action(cx.listener(move |this, _: &OpenPinMenu, _, cx| {
                this.board.update(cx, |board, cx| {
                    if let Some(pin) = board.pins.last() {
                        let visible = pin.rect.intersect(&output);
                        if visible.size.width > px(0.) && visible.size.height > px(0.) {
                            board.menu = Some((pin.id, visible.origin));
                            cx.notify();
                        }
                    }
                });
            }))
            .on_action(
                cx.listener(|this, _: &ClosePin, window, cx| this.close_selected(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyPin, _, cx| this.copy_selected(cx)))
            .on_action(cx.listener(|this, _: &ResetPinZoom, _, cx| {
                this.board.update(cx, |board, cx| {
                    if let Some(id) = board.selected_id() {
                        board.menu = None;
                        board.reset_zoom(id);
                        cx.notify();
                    }
                });
            }))
            .child(sink)
            .children(pins)
            .children(backdrop)
            .children(menu_el)
    }
}
