//! The pinned scene's element tree: dynamic Wayland input regions (with
//! native click-through exclusion for ghost pins), framed images, collapsed
//! thumbnail cards, in-pin annotation overlays, HUD status badges, and the
//! context menu.

use gpui_kit::assets::IconName;
use gpui_kit::base::{Button, GlobalState};
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::actions::*;
use crate::annotation::ShapeKind;
use crate::model::session::EraserChrome;
use crate::model::settings::{PinDoubleClick, PinWheelAction, resolve_pin_wheel_action};
use crate::ui::hud::{
    annotation_chrome, annotation_hover_chrome, eraser_chrome, lens_draft_chrome, marquee_chrome,
};
use crate::ui::theme;
use crate::ui::toolbar::pin_annotation_toolbar;

use super::menu::render_hud_pill;
use super::{GHOST_CHIP_SIDE, PinDrag, PinSurface, ghost_chip_bounds};

impl Render for PinSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_annot_session_observer(window, cx);
        let annot_session_info = self.active_annot_session(cx);
        let size_slider = self.annot_size_slider(window, cx);
        let swatch_picker = self.annot_swatch_editor_seed(window, cx);

        let board = self.board.clone();
        let output = self.output.bounds;
        let active = window.is_window_active();
        let has_text_editing = self.text_editing.is_some();
        let deferred_open = GlobalState::is_in_deferred_context(cx)
            || self
                .swatch_picker
                .as_ref()
                .is_some_and(|p| p.read(cx).is_open());

        let (focused, any_menu, hidden_or_saving, annotating_id, annot_bar) = {
            let state = board.read(cx);
            (
                state.selected_id(),
                state.menu.is_some(),
                state.hidden || state.save_dialog_open,
                state.annotating_id(),
                state.annot_bar_bounds_on(output, cx),
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
                } else if any_menu || deferred_open || has_text_editing {
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
                    if let Some(bar) = state.annot_bar_bounds_on(output, cx) {
                        let slice = bar.intersect(&output);
                        if slice.size.width > px(0.) && slice.size.height > px(0.) {
                            rects.push(Bounds::new(slice.origin - output.origin, slice.size));
                        }
                    }
                    rects
                };
                window.set_input_region(Some(&regions));

                let moving = board.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || GlobalState::is_in_deferred_context(cx) {
                        return;
                    }
                    let global = event.position + output.origin;
                    if moving.read(cx).drag.is_some() {
                        moving.update(cx, |b, cx| {
                            b.move_drag(global, event.modifiers.alt);
                            cx.notify();
                        });
                    } else if moving.read(cx).annot_session.is_some() {
                        moving.update(cx, |b, cx| {
                            if b.annot_pointer_move(
                                global,
                                event.modifiers.shift,
                                event.modifiers.alt,
                                cx,
                            ) {
                                cx.notify();
                            }
                        });
                    }
                });

                let releasing = board.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || GlobalState::is_in_deferred_context(cx)
                    {
                        return;
                    }
                    let global = event.position + output.origin;
                    let had_annot_drag = releasing
                        .read(cx)
                        .annot_session
                        .as_ref()
                        .is_some_and(|a| a.pointer_down_active);
                    if had_annot_drag {
                        releasing.update(cx, |b, cx| {
                            if b.annot_pointer_up(
                                global,
                                event.modifiers.shift,
                                event.modifiers.alt,
                                cx,
                            ) {
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
                            let ghost_border = (c.pin_border & 0xFFFF_FF00) | 0x66;
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
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(c.capsule_surface()))
                            .border_1()
                            .border_color(rgba(c.capsule_border()))
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
                                        .children(hud_text.as_deref().map(render_hud_pill)),
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
                            .rounded(px(10.))
                            .overflow_hidden()
                            .opacity(pin_opacity)
                            .border(if show_border { px(1.) } else { px(0.) })
                            .border_color(rgba(c.pin_border))
                            .bg(rgba(c.capsule_surface()))
                            .shadow(if !show_shadow {
                                Vec::new()
                            } else if is_focused {
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
                                    if this.board.read(cx).annotating_id().is_some() {
                                        this.handle_annot_right_click(window, cx);
                                        cx.stop_propagation();
                                        return;
                                    }
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
                                    .bg(rgba(c.capsule_surface()))
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
                        } else {
                            c.pin_border
                        };
                        container = container.border_1().border_color(rgba(border_col));
                    }
                    let shadows = if !show_shadow {
                        Vec::new()
                    } else if is_focused || is_annotating_this {
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
                    };

                    Some(
                        container
                            .shadow(shadows)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    if GlobalState::is_in_deferred_context(cx) {
                                        return;
                                    }
                                    let is_annot = this.board.read(cx).annotating_id() == Some(id);
                                    if is_annot {
                                        this.board.update(cx, |b, cx| {
                                            b.raise(id);
                                            b.menu = None;
                                            cx.notify();
                                        });
                                        if this.handle_annot_mouse_down(id, event, window, cx) {
                                            cx.stop_propagation();
                                            return;
                                        }
                                    } else {
                                        this.finish_pin_text(true, window, cx);
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
                                    if this.board.read(cx).annotating_id().is_some() {
                                        this.handle_annot_right_click(window, cx);
                                        cx.stop_propagation();
                                        return;
                                    }
                                    this.board.update(cx, |b, cx| {
                                        b.raise(id);
                                        b.drag = None;
                                        b.menu = Some((id, event.position + output.origin));
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                }),
                            )
                            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                                if GlobalState::is_in_deferred_context(cx)
                                    || this.text_editing.is_some()
                                {
                                    cx.propagate();
                                    return;
                                }
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
                                    .children(hud_text.as_deref().map(render_hud_pill)),
                            )
                            .into_any_element(),
                    )
                })
                .collect()
        };

        let (
            annot_hover_el,
            annot_chrome_el,
            annot_marquee_el,
            annot_lens_el,
            annot_eraser_el,
            is_drawing_polyline,
        ) = if !hidden_or_saving
            && let Some((aid, ref session)) = annot_session_info
            && let Some(apin) = self
                .board
                .read(cx)
                .pins
                .iter()
                .find(|p| p.id == aid && !p.ghost && p.collapsed.is_none())
        {
            let slice = apin.rect.intersect(&output);
            let s = session.read(cx);
            let drawing_poly = s.annotations().is_drawing_polyline();
            if slice.size.width > px(0.) && slice.size.height > px(0.) {
                let pin_local = apin.rect.origin - output.origin;
                let zoom = apin.zoom_factor();
                let scale_pt = |p: Point<Pixels>| {
                    pin_local + point(px(f32::from(p.x) * zoom), px(f32::from(p.y) * zoom))
                };
                let hover = s
                    .annotation_hover_bounds("pin")
                    .map(|corners| annotation_hover_chrome(corners.map(scale_pt)));
                let selected: Vec<_> = s
                    .selected_shapes_local("pin")
                    .into_iter()
                    .map(|sh| sh.scaled_for_display(zoom, pin_local))
                    .collect();
                let pointer_local = s.pointer_in("pin").map(scale_pt);
                let active_anchor = s.handle_drag_anchor().map(|(_, a)| a);
                let chrome = annotation_chrome(selected, pointer_local, active_anchor);
                let marquee = s.marquee_bounds_local("pin").map(|b| {
                    let origin = scale_pt(b.origin);
                    let sz = size(
                        px(f32::from(b.size.width) * zoom),
                        px(f32::from(b.size.height) * zoom),
                    );
                    marquee_chrome(Bounds::new(origin, sz))
                });
                let lens_draft = s
                    .local_lens_draft_outline("pin")
                    .map(|sh| lens_draft_chrome(sh.scaled_for_display(zoom, pin_local)));
                let eraser = s.eraser_chrome("pin").map(|ec| {
                    eraser_chrome(EraserChrome {
                        center: scale_pt(ec.center),
                        radius: ec.radius * zoom,
                    })
                });
                (
                    hover,
                    Some(chrome),
                    marquee,
                    lens_draft,
                    eraser,
                    drawing_poly,
                )
            } else {
                (None, None, None, None, None, drawing_poly)
            }
        } else {
            (None, None, None, None, None, false)
        };

        let menu_el = self.board.read(cx).render_menu_on(output);
        let annot_bar_el = if hidden_or_saving {
            None
        } else if let (Some(bar), Some((_, session))) = (annot_bar, &annot_session_info) {
            let local_bar = Bounds::new(bar.origin - output.origin, bar.size);
            Some(pin_annotation_toolbar(
                local_bar,
                session.read(cx).annotations(),
                session.clone(),
                self.focus_handle.clone(),
                size_slider,
                swatch_picker,
            ))
        } else {
            None
        };
        let text_editor_el = (!hidden_or_saving && annot_bar.is_some())
            .then(|| self.text_editing.as_ref().map(|e| e.render()))
            .flatten();

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
        } else if has_text_editing {
            "ShotoriTextEditing"
        } else if GlobalState::is_in_deferred_context(cx) {
            "ShotoriControls"
        } else if annotating_id.is_some() && is_drawing_polyline {
            "ShotoriPinAnnotate PolylineDrawing"
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    if !GlobalState::is_in_deferred_context(cx)
                        && this.board.read(cx).menu.is_none()
                        && this.board.read(cx).annotating_id().is_some()
                    {
                        this.handle_annot_right_click(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !GlobalState::is_in_deferred_context(cx) && this.text_editing.is_some() {
                        this.finish_pin_text(true, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
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
            .on_action(cx.listener(|this, _: &ClosePin, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.close_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseAllPins, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.close_all_pins(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyPin, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.copy_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &SavePin, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.save_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &OcrPin, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.ocr_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &QrPin, window, cx| {
                this.finish_pin_text(true, window, cx);
                this.qr_selected(cx);
            }))
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
                this.finish_pin_text(true, window, cx);
                this.board.update(cx, |b, cx| b.toggle_annotate(cx));
            }))
            .on_action(cx.listener(|this, _: &EscapeOverlay, window, cx| {
                this.handle_annot_escape(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Enter, _, cx| {
                    if this.text_editing.is_some() && !GlobalState::is_in_deferred_context(cx) {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Escape, window, cx| {
                    if GlobalState::is_in_deferred_context(cx) {
                        cx.propagate();
                        return;
                    }
                    if let Some(editor) = &this.text_editing {
                        if editor.focus_handle(cx).is_focused(window) {
                            this.handle_annot_escape(window, cx);
                        } else {
                            editor.focus(window, cx);
                        }
                    } else {
                        window.focus(&this.focus_handle, cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSelect, window, cx| {
                this.toggle_pin_tool(ShapeKind::Select, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRectangle, window, cx| {
                this.toggle_pin_tool(ShapeKind::Rectangle, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEllipse, window, cx| {
                this.toggle_pin_tool(ShapeKind::Ellipse, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleLine, window, cx| {
                this.toggle_pin_tool(ShapeKind::Line, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleArrow, window, cx| {
                this.toggle_pin_tool(ShapeKind::Arrow, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleText, window, cx| {
                this.toggle_pin_tool(ShapeKind::Text, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleNumber, window, cx| {
                this.toggle_pin_tool(ShapeKind::Number, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePencil, window, cx| {
                this.toggle_pin_tool(ShapeKind::Pencil, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHighlighter, window, cx| {
                this.toggle_pin_tool(ShapeKind::Highlighter, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEraser, window, cx| {
                this.toggle_pin_tool(ShapeKind::Eraser, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSpotlight, window, cx| {
                this.toggle_pin_tool(ShapeKind::Spotlight, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMagnifier, window, cx| {
                this.toggle_pin_tool(ShapeKind::Magnifier, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMosaic, window, cx| {
                let Some((_, session)) = this.active_annot_session(cx) else {
                    return;
                };
                let blur = session.read(cx).annotations().tool() == Some(ShapeKind::Blur);
                let kind = if blur {
                    ShapeKind::Blur
                } else {
                    ShapeKind::Mosaic
                };
                this.toggle_pin_tool(kind, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePolyline, window, cx| {
                this.toggle_pin_tool(ShapeKind::Polyline, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FinishPolyline, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        s.edit_annotations(|a| a.finish_polyline());
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &PopPolylineVertex, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.pop_polyline_vertex() {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &DuplicateAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.duplicate_selected_annotations() {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &BringForward, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.reorder_selected_annotations(
                            crate::annotation::LayerOrder::BringForward,
                        ) {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &SendBackward, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.reorder_selected_annotations(
                            crate::annotation::LayerOrder::SendBackward,
                        ) {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &BringToFront, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.reorder_selected_annotations(
                            crate::annotation::LayerOrder::BringToFront,
                        ) {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &SendToBack, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.reorder_selected_annotations(crate::annotation::LayerOrder::SendToBack)
                        {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &StepSizeDown, _, cx| {
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        let mut changed = false;
                        s.edit_annotations(|a| changed = a.step_size(false));
                        if changed {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &StepSizeUp, _, cx| {
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        let mut changed = false;
                        s.edit_annotations(|a| changed = a.step_size(true));
                        if changed {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &Nudge, _, cx| {
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        if s.nudge(point(px(action.dx as f32), px(action.dy as f32))) {
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &UndoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        s.undo();
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RedoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        s.redo();
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        s.delete_selected_annotations();
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &ClearAnnotations, window, cx| {
                this.finish_pin_text(false, window, cx);
                window.focus(&this.focus_handle, cx);
                if let Some((_, session)) = this.active_annot_session(cx) {
                    session.update(cx, |s, cx| {
                        s.clear_annotations();
                        cx.notify();
                    });
                }
            }))
            .child(sink)
            .children(pins)
            .children(annot_hover_el)
            .children(annot_chrome_el)
            .children(annot_marquee_el)
            .children(annot_lens_el)
            .children(annot_eraser_el)
            .children(annot_bar_el)
            .children(text_editor_el)
            .children(backdrop)
            .children(menu_el)
    }
}
