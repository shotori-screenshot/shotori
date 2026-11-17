//! The pinned scene's element tree: dynamic Wayland input regions (with
//! native click-through exclusion for ghost pins), framed images, collapsed
//! thumbnail cards, in-pin annotation overlays, HUD status badges, and the
//! context menu.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::actions::*;
use crate::annotation::ShapeKind;
use crate::model::settings::{PinDoubleClick, PinWheelAction, resolve_pin_wheel_action};
use crate::ui::theme;

use super::menu::render_hud_pill;
use super::{GHOST_CHIP_SIDE, PinDrag, PinSurface, ghost_chip_bounds};

impl Render for PinSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let board = self.board.clone();
        let output = self.output.bounds;
        let active = window.is_window_active();

        let (focused, any_menu, hidden_or_saving, annotating_id, has_text_prompt, annot_bar) = {
            let state = board.read(cx);
            (
                state.selected_id(),
                state.menu.is_some(),
                state.hidden || state.save_dialog_open,
                state.annotating,
                state.text_prompt.is_some(),
                state.annot_bar_bounds_on(output),
            )
        };
        let menu_on_this_output = board
            .read(cx)
            .menu
            .filter(|(_, position)| output.contains(position))
            .is_some();

        let sink = canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                let regions: Vec<Bounds<Pixels>> = if hidden_or_saving {
                    Vec::new()
                } else if any_menu {
                    vec![Bounds::new(point(px(0.), px(0.)), output.size)]
                } else {
                    let state = board.read(cx);
                    let mut rects: Vec<Bounds<Pixels>> = state
                        .pins
                        .iter()
                        .filter_map(|pin| {
                            // Click-through (ghost) pins omit their body from
                            // Wayland's wl_surface input region so pointer and
                            // wheel events pass straight to apps underneath;
                            // only the tiny top-right lock chip stays clickable.
                            let target = if pin.ghost {
                                ghost_chip_bounds(pin.rect)
                            } else {
                                pin.rect
                            };
                            let slice = target.intersect(&output);
                            (slice.size.width > px(0.) && slice.size.height > px(0.))
                                .then_some(Bounds::new(slice.origin - output.origin, slice.size))
                        })
                        .collect();
                    if let Some(bar) = state.annot_bar_bounds_on(output) {
                        let slice = bar.intersect(&output);
                        if slice.size.width > px(0.) && slice.size.height > px(0.) {
                            rects.push(Bounds::new(slice.origin - output.origin, slice.size));
                        }
                    }
                    if let Some(tp) = state.text_prompt.as_ref() {
                        let slice = tp.global_bounds.intersect(&output);
                        if slice.size.width > px(0.) && slice.size.height > px(0.) {
                            rects.push(Bounds::new(slice.origin - output.origin, slice.size));
                        }
                    }
                    rects
                };
                window.set_input_region(Some(&regions));

                let moving = board.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let global = event.position + output.origin;
                    if moving.read(cx).annot_gesture.is_some() {
                        moving.update(cx, |b, cx| {
                            if b.annot_pointer_move(global, event.modifiers.shift) {
                                cx.notify();
                            }
                        });
                    } else if moving.read(cx).drag.is_some() {
                        moving.update(cx, |b, cx| {
                            b.move_drag(global, event.modifiers.alt);
                            cx.notify();
                        });
                    }
                });

                let releasing = board.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let global = event.position + output.origin;
                    if releasing.read(cx).annot_gesture.is_some() {
                        releasing.update(cx, |b, cx| {
                            if b.annot_pointer_up(global, event.modifiers.shift) {
                                cx.notify();
                            }
                        });
                    } else if releasing.read(cx).drag.is_some() {
                        releasing.update(cx, |b, cx| {
                            b.finish_drag(global, event.modifiers.alt);
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let capsule = crate::ui::e2e::debug_ui_style()
            .unwrap_or_else(|| crate::model::settings::current().ui_style())
            == crate::model::settings::UiStyle::Capsule;
        let pin_cfg = crate::model::settings::current().pin();
        let show_border = pin_cfg.show_border;
        let show_shadow = pin_cfg.shadow;
        let double_click_action = pin_cfg.double_click;
        let c = theme::c();

        let pins: Vec<AnyElement> = if hidden_or_saving {
            Vec::new()
        } else {
            self.board
                .read(cx)
                .pins
                .iter()
                .filter_map(|pin| {
                    let slice = pin.rect.intersect(&output);
                    if slice.size.width <= px(0.) || slice.size.height <= px(0.) {
                        return None;
                    }
                    let id = pin.id;
                    let is_focused = active && focused == Some(id);
                    let local = pin.rect.origin - output.origin;
                    let pin_opacity = (f32::from(pin.opacity) / 100.).clamp(0.2, 1.0);
                    let is_annotating_this = annotating_id == Some(id);
                    let hud_text = pin.hud.as_ref().map(|(_, t)| t.clone());

                    // ── Ghost (click-through) pin ─────────────────────────────
                    if pin.ghost {
                        let frame = if show_border { px(1.) } else { px(0.) };
                        let chip_local = ghost_chip_bounds(pin.rect).origin - output.origin;
                        let board_unlock = self.board.clone();
                        let mut ghost_box = div()
                            .id(("pin-image", id))
                            .debug_selector(move || format!("pin-image-{id}"))
                            .absolute()
                            .left(local.x - frame)
                            .top(local.y - frame)
                            .w(pin.rect.size.width + frame * 2.)
                            .h(pin.rect.size.height + frame * 2.)
                            .opacity(pin_opacity)
                            .overflow_hidden();
                        if show_border {
                            let ghost_border = if capsule {
                                c.capsule_border()
                            } else {
                                (c.pin_border & 0xFFFF_FF00) | 0x66
                            };
                            ghost_box = ghost_box.border_1().border_color(rgba(ghost_border));
                        }
                        let unlock_btn = Button::new(("pin-ghost-unlock", id))
                            .debug_selector(move || format!("pin-ghost-unlock-{id}"))
                            .accessibility_label(crate::i18n::t!("pin.menu.unlock_all").as_ref())
                            .absolute()
                            .left(chip_local.x)
                            .top(chip_local.y)
                            .w(px(GHOST_CHIP_SIDE))
                            .h(px(GHOST_CHIP_SIDE))
                            .rounded(if capsule { px(6.) } else { px(4.) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(if capsule {
                                c.capsule_surface()
                            } else {
                                (c.toolbar_bg & 0xFFFF_FF00) | 0xBF
                            }))
                            .border_1()
                            .border_color(rgba(if capsule {
                                c.capsule_border()
                            } else {
                                c.toolbar_border
                            }))
                            .opacity(0.65)
                            .hover(|s| s.opacity(1.0).border_color(rgba(c.accent)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                board_unlock.update(cx, |b, cx| b.toggle_ghost(id, cx));
                            })
                            .child(
                                Icon::new(IconName::Lock)
                                    .size_3()
                                    .text_color(rgba(c.accent)),
                            );

                        return Some(
                            div()
                                .absolute()
                                .left(px(0.))
                                .top(px(0.))
                                .size_full()
                                .child(
                                    ghost_box
                                        .child(
                                            div()
                                                .size_full()
                                                .debug_selector(move || format!("pin-img-{id}"))
                                                .child(
                                                    img(pin.image.clone())
                                                        .size_full()
                                                        .object_fit(ObjectFit::Fill),
                                                ),
                                        )
                                        .children(
                                            hud_text
                                                .as_deref()
                                                .map(|t| render_hud_pill(t, capsule)),
                                        ),
                                )
                                .child(unlock_btn)
                                .into_any_element(),
                        );
                    }

                    // ── Collapsed thumbnail pin ───────────────────────────────
                    if pin.collapsed.is_some() {
                        let thumb_card = div()
                            .id(("pin-image", id))
                            .debug_selector(move || format!("pin-image-{id}"))
                            .absolute()
                            .left(local.x)
                            .top(local.y)
                            .w(pin.rect.size.width)
                            .h(pin.rect.size.height)
                            .rounded(if capsule { px(10.) } else { px(6.) })
                            .overflow_hidden()
                            .opacity(pin_opacity)
                            .border(if capsule && !is_focused {
                                px(1.)
                            } else {
                                px(2.)
                            })
                            .border_color(rgba(if is_focused {
                                c.accent
                            } else if capsule {
                                c.capsule_border()
                            } else {
                                c.pin_border
                            }))
                            .bg(rgba(if capsule {
                                c.capsule_surface()
                            } else {
                                c.toolbar_bg
                            }))
                            .shadow(if show_shadow && capsule {
                                theme::capsule_floating_shadow()
                            } else {
                                Vec::new()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    let global = event.position + output.origin;
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.menu = None;
                                        if event.click_count == 2 {
                                            b.toggle_collapse(id, cx);
                                        } else if let Some(p) = b.pins.last() {
                                            b.drag = Some(PinDrag {
                                                id,
                                                grab: global - p.rect.origin,
                                                press_global: global,
                                                moved: false,
                                            });
                                            cx.notify();
                                        }
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    this.board.update(cx, |b, cx| b.toggle_collapse(id, cx));
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.drag = None;
                                        b.menu = Some((id, event.position + output.origin));
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .size_full()
                                    .debug_selector(move || format!("pin-img-{id}"))
                                    .child(
                                        img(pin.thumb_image.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Cover),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(3.))
                                    .bottom(px(3.))
                                    .w(px(16.))
                                    .h(px(16.))
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(rgba(if capsule {
                                        c.capsule_surface()
                                    } else {
                                        (c.toolbar_bg & 0xFFFF_FF00) | 0xD8
                                    }))
                                    .child(
                                        Icon::new(IconName::Maximize2)
                                            .size_2p5()
                                            .text_color(rgba(c.toolbar_text)),
                                    ),
                            );
                        return Some(thumb_card.into_any_element());
                    }

                    // ── Full floating pin (standard or annotating) ────────────
                    // The frame is drawn AROUND the image, not inside it: the
                    // div is grown by the 1px border width on every side, so
                    // the border box's CONTENT area is exactly the pin rect
                    // and the bitmap lands pixel-perfect over the desktop it
                    // froze.
                    let frame = if show_border || is_annotating_this {
                        px(1.)
                    } else {
                        px(0.)
                    };
                    let zoom = (f32::from(pin.rect.size.width) / f32::from(pin.base.width).max(1.))
                        .max(0.01);
                    let selected_mark_box = is_annotating_this
                        .then(|| pin.annotations.selected())
                        .flatten()
                        .map(|shape| {
                            let sb = shape.selection_box();
                            Bounds::new(
                                point(
                                    px(f32::from(sb.origin.x) * zoom),
                                    px(f32::from(sb.origin.y) * zoom),
                                ),
                                size(
                                    px((f32::from(sb.size.width) * zoom).max(2.)),
                                    px((f32::from(sb.size.height) * zoom).max(2.)),
                                ),
                            )
                        });

                    let mut container = div()
                        .id(("pin-image", id))
                        .debug_selector(move || format!("pin-image-{id}"))
                        .absolute()
                        .left(local.x - frame)
                        .top(local.y - frame)
                        .w(pin.rect.size.width + frame * 2.)
                        .h(pin.rect.size.height + frame * 2.)
                        .opacity(pin_opacity)
                        .overflow_hidden();
                    if show_border || is_annotating_this {
                        let border_col = if is_annotating_this {
                            c.accent
                        } else if capsule && !is_focused {
                            c.capsule_border()
                        } else {
                            c.pin_border
                        };
                        container = container.border_1().border_color(rgba(border_col));
                    }
                    let shadows = if !show_shadow {
                        Vec::new()
                    } else if capsule {
                        if is_focused {
                            vec![
                                BoxShadow::new(px(0.), px(8.), rgba(0x0000_0059).into())
                                    .blur_radius(px(28.))
                                    .spread_radius(px(-2.)),
                                BoxShadow::new(
                                    px(0.),
                                    px(0.),
                                    rgba((c.accent & 0xFFFF_FF00) | 0x55).into(),
                                )
                                .blur_radius(px(10.)),
                            ]
                        } else {
                            theme::capsule_floating_shadow()
                        }
                    } else if is_focused {
                        vec![
                            BoxShadow::new(
                                px(0.),
                                px(0.),
                                rgba((c.accent & 0xFFFF_FF00) | 0x8C).into(),
                            )
                            .blur_radius(px(6.)),
                            BoxShadow::new(
                                px(0.),
                                px(0.),
                                rgba((c.accent & 0xFFFF_FF00) | 0x47).into(),
                            )
                            .blur_radius(px(24.)),
                        ]
                    } else {
                        Vec::new()
                    };

                    Some(
                        container
                            .shadow(shadows)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    let is_annot = this.board.read(cx).annotating == Some(id);
                                    if is_annot {
                                        this.board.update(cx, |b, cx| {
                                            b.raise(id);
                                            b.menu = None;
                                            cx.notify();
                                        });
                                        this.handle_annot_mouse_down(id, event, window, cx);
                                        cx.stop_propagation();
                                        return;
                                    }
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.menu = None;
                                        cx.notify();
                                    });
                                    if event.click_count == 2 {
                                        match double_click_action {
                                            PinDoubleClick::None => {}
                                            PinDoubleClick::Close => {
                                                this.close_selected(window, cx);
                                                cx.stop_propagation();
                                                return;
                                            }
                                            PinDoubleClick::CopyAndClose => {
                                                this.copy_selected(cx);
                                                this.close_selected(window, cx);
                                                cx.stop_propagation();
                                                return;
                                            }
                                            PinDoubleClick::ResetZoom => {
                                                this.board.update(cx, |b, cx| {
                                                    b.reset_zoom(id, cx);
                                                    cx.notify();
                                                });
                                                cx.stop_propagation();
                                                return;
                                            }
                                            PinDoubleClick::Collapse => {
                                                this.board.update(cx, |b, cx| {
                                                    b.toggle_collapse(id, cx);
                                                });
                                                cx.stop_propagation();
                                                return;
                                            }
                                        }
                                    }
                                    let global = event.position + output.origin;
                                    this.board.update(cx, |b, cx| {
                                        let Some(p) = b.pins.last() else {
                                            return;
                                        };
                                        b.drag = Some(PinDrag {
                                            id,
                                            grab: global - p.rect.origin,
                                            press_global: global,
                                            moved: false,
                                        });
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.toggle_collapse(id, cx);
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.drag = None;
                                        b.annot_gesture = None;
                                        b.menu = Some((id, event.position + output.origin));
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                                let cfg = crate::model::settings::current().pin();
                                let Some(wheel_action) = resolve_pin_wheel_action(
                                    &event.modifiers,
                                    cfg.zoom_modifier,
                                    cfg.opacity_modifier,
                                ) else {
                                    return;
                                };
                                let lines = match event.delta {
                                    ScrollDelta::Lines(lines) => lines.y,
                                    ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 40.,
                                };
                                if lines.abs() < 1e-4 {
                                    return;
                                }
                                let cursor = event.position + output.origin;
                                match wheel_action {
                                    PinWheelAction::Zoom => {
                                        this.board.update(cx, |b, cx| {
                                            b.zoom_at_with_hud(id, lines, cursor, cx);
                                            cx.notify();
                                        });
                                    }
                                    PinWheelAction::Opacity => {
                                        let delta = (lines * 5.0).round() as i16;
                                        let step = if delta == 0 {
                                            if lines > 0. { 5 } else { -5 }
                                        } else {
                                            delta
                                        };
                                        this.board.update(cx, |b, cx| {
                                            b.step_opacity(id, step, cx);
                                        });
                                    }
                                }
                                cx.stop_propagation();
                            }))
                            .child(
                                div()
                                    .relative()
                                    .size_full()
                                    .debug_selector(move || format!("pin-img-{id}"))
                                    .child(
                                        img(pin.image.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Fill),
                                    )
                                    .children(selected_mark_box.map(|mb| {
                                        div()
                                            .absolute()
                                            .left(mb.origin.x)
                                            .top(mb.origin.y)
                                            .w(mb.size.width)
                                            .h(mb.size.height)
                                            .border_1()
                                            .border_color(rgba(c.accent))
                                    }))
                                    .children(
                                        hud_text.as_deref().map(|t| render_hud_pill(t, capsule)),
                                    ),
                            )
                            .into_any_element(),
                    )
                })
                .collect()
        };

        let menu_el = self.board.read(cx).render_menu_on(output, capsule);
        let annot_bar_el = annot_bar.and_then(|b| self.render_annot_bar(b, cx));
        let text_prompt_el = self.render_text_prompt(cx);

        // All outputs consume the dismissing click when a context menu is open.
        let backdrop = any_menu.then(|| {
            div()
                .id("pin-menu-backdrop")
                .absolute()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.board.update(cx, |b, cx| {
                            b.menu = None;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.board.update(cx, |b, cx| {
                            b.menu = None;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }),
                )
        });

        let key_ctx = if menu_on_this_output {
            "ShotoriPinMenu"
        } else if has_text_prompt {
            "ShotoriPinText"
        } else if annotating_id.is_some() {
            "ShotoriPinAnnotate"
        } else {
            "ShotoriPin"
        };

        div()
            .id("shotori-pin")
            .overflow_hidden()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context(key_ctx)
            .on_action(cx.listener(|this, _: &DismissPinMenu, _, cx| {
                this.board.update(cx, |b, cx| {
                    b.menu = None;
                    cx.notify();
                });
            }))
            .on_action(cx.listener(move |this, _: &OpenPinMenu, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(pin) = b.pins.last() {
                        let visible = pin.rect.intersect(&output);
                        if visible.size.width > px(0.) && visible.size.height > px(0.) {
                            b.menu = Some((pin.id, visible.origin));
                            cx.notify();
                        }
                    }
                });
            }))
            .on_action(
                cx.listener(|this, _: &ClosePin, window, cx| this.close_selected(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseAllPins, window, cx| this.close_all_pins(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyPin, _, cx| this.copy_selected(cx)))
            .on_action(cx.listener(|this, _: &SavePin, _, cx| this.save_selected(cx)))
            .on_action(cx.listener(|this, _: &OcrPin, _, cx| this.ocr_selected(cx)))
            .on_action(cx.listener(|this, _: &QrPin, _, cx| this.qr_selected(cx)))
            .on_action(cx.listener(|this, _: &PastePin, _, cx| this.paste_clipboard_pin(cx)))
            .on_action(cx.listener(|this, _: &RestoreClosedPin, _, cx| {
                this.board.update(cx, |b, cx| {
                    b.restore_last_closed(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &OpenPinHistory, _, cx| {
                this.board.update(cx, |b, cx| {
                    b.menu = None;
                    cx.notify();
                });
                crate::launch_detached_args(&["pin", "--history"]);
            }))
            .on_action(cx.listener(|this, _: &ResetPinZoom, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.menu = None;
                        b.reset_zoom(id, cx);
                        cx.notify();
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &RotatePinCw, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.rotate_pin(id, true, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &RotatePinCcw, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.rotate_pin(id, false, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &FlipPinHorizontal, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.flip_pin(id, true, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &FlipPinVertical, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.flip_pin(id, false, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &TogglePinGrayscale, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.toggle_grayscale(id, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &TogglePinInvert, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.toggle_invert(id, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &StepPinOpacityUp, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.step_opacity(id, 10, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &StepPinOpacityDown, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.step_opacity(id, -10, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &TogglePinCollapse, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.toggle_collapse(id, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &TogglePinGhost, _, cx| {
                this.board.update(cx, |b, cx| {
                    if let Some(id) = b.selected_id() {
                        b.toggle_ghost(id, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &UnlockAllPins, _, cx| {
                this.board.update(cx, |b, cx| b.unlock_all_ghosts(cx));
            }))
            .on_action(cx.listener(|this, _: &TogglePinVisibility, _, cx| {
                this.board.update(cx, |b, cx| b.toggle_visibility(cx));
            }))
            .on_action(cx.listener(|this, nudge: &PinNudge, _, cx| {
                this.board
                    .update(cx, |b, cx| b.nudge_selected(nudge.dx, nudge.dy, cx));
            }))
            .on_action(cx.listener(|this, _: &NextPin, _, cx| {
                this.board.update(cx, |b, cx| b.cycle_focus(true, cx));
            }))
            .on_action(cx.listener(|this, _: &PrevPin, _, cx| {
                this.board.update(cx, |b, cx| b.cycle_focus(false, cx));
            }))
            .on_action(cx.listener(|this, _: &TogglePinAnnotate, window, cx| {
                this.commit_pin_text_prompt(window, cx);
                this.board.update(cx, |b, cx| b.toggle_annotate(cx));
            }))
            .on_action(cx.listener(|this, _: &ExitPinAnnotate, window, cx| {
                if this.board.read(cx).text_prompt.is_some() {
                    this.board.update(cx, |b, cx| {
                        b.text_prompt = None;
                        cx.notify();
                    });
                    window.focus(&this.focus_handle, cx);
                } else {
                    this.board.update(cx, |b, cx| b.exit_annotate_step(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PinUndo, _, cx| {
                this.board.update(cx, |b, cx| b.annot_undo(cx));
            }))
            .on_action(cx.listener(|this, _: &PinRedo, _, cx| {
                this.board.update(cx, |b, cx| b.annot_redo(cx));
            }))
            .on_action(cx.listener(|this, _: &PinDeleteMark, _, cx| {
                this.board.update(cx, |b, cx| b.annot_delete(cx));
            }))
            .on_action(cx.listener(|this, _: &PinClearMarks, _, cx| {
                this.board.update(cx, |b, cx| b.annot_clear(cx));
            }))
            .on_action(cx.listener(|this, _: &PinSizeDown, _, cx| {
                this.board.update(cx, |b, cx| b.step_annot_size(false, cx));
            }))
            .on_action(cx.listener(|this, _: &PinSizeUp, _, cx| {
                this.board.update(cx, |b, cx| b.step_annot_size(true, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolSelect, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Select, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolRect, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Rectangle, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolEllipse, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Ellipse, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolArrow, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Arrow, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolLine, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Line, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolPencil, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Pencil, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolHighlighter, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Highlighter, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolMosaic, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Mosaic, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolNumber, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Number, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolText, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Text, cx));
            }))
            .on_action(cx.listener(|this, _: &PinToolEraser, _, cx| {
                this.board
                    .update(cx, |b, cx| b.select_annot_tool(ShapeKind::Eraser, cx));
            }))
            .child(sink)
            .children(pins)
            .children(annot_bar_el)
            .children(text_prompt_el)
            .children(backdrop)
            .children(menu_el)
    }
}
