//! Floating-pin context menu and transient HUD status badge.
//!
//! Layout groups pin controls by frequency: a top icon
//! strip for transforms, annotation, collapse, and click-through lock, a
//! compact zoom/opacity bar, and keyboard-hinted action rows — automatically
//! tightening row metrics on constrained outputs so every control stays
//! reachable without clipping.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::actions::{
    ClosePin, CopyPin, FlipPinHorizontal, FlipPinVertical, OcrPin, OpenPinHistory, PastePin, QrPin,
    ResetPinZoom, RestoreClosedPin, RotatePinCcw, RotatePinCw, SavePin, StepPinOpacityDown,
    StepPinOpacityUp, TogglePinAnnotate, TogglePinCollapse, TogglePinGhost, TogglePinGrayscale,
    TogglePinInvert, UnlockAllPins,
};
use crate::ui::theme;

use super::PinBoard;

pub(super) const MENU_W: f32 = 244.;

impl PinBoard {
    pub(super) fn render_menu_on(&self, output: Bounds<Pixels>) -> Option<Stateful<Div>> {
        let (pin_id, position) = self.menu.filter(|(_, pos)| output.contains(pos))?;
        let pin = self.pins.iter().find(|p| p.id == pin_id)?;
        let c = theme::c();
        let row_radius = px(6.);

        let zoom_pct = ((f32::from(pin.rect.size.width) / f32::from(pin.base.width).max(1.)) * 100.)
            .round() as u32;
        let opacity_pct = pin.opacity;
        let is_collapsed = pin.collapsed.is_some();
        let is_ghost = pin.ghost;
        let is_gray = pin.grayscale;
        let is_inv = pin.invert;
        let is_annot = self.annotating_id() == Some(pin_id);
        let has_any_ghost = self.pins.iter().any(|p| p.ghost);
        let can_restore = !self.closed_stack.is_empty()
            || self
                .history_store
                .as_ref()
                .and_then(|s| s.latest())
                .is_some();

        let row_count = 7 + usize::from(has_any_ghost);
        let out_h = f32::from(output.size.height);
        let compact = out_h < 290.;
        let pad = if compact { 3. } else { 5. };
        let gap = if compact { 2. } else { 3. };
        let strip_h = if compact { 22. } else { 28. };
        let bar_h = if compact { 20. } else { 26. };
        let item_h = if compact { 18. } else { 26. };
        let sep_h = 1.;

        // Total blocks = icon strip + sep + zoom/opacity bar + sep + `row_count` items.
        let total_blocks = 4 + row_count;
        let natural_h = pad * 2.
            + strip_h
            + bar_h
            + sep_h * 2.
            + (row_count as f32) * item_h
            + ((total_blocks - 1) as f32) * gap;

        let menu_w = MENU_W.min(f32::from(output.size.width));
        let menu_h = natural_h.min(out_h);
        let local = position - output.origin;
        let origin = point(
            local.x.max(px(0.)).min(output.size.width - px(menu_w)),
            local.y.max(px(0.)).min(output.size.height - px(menu_h)),
        );

        let icon_btn = |id: &'static str,
                        label: &str,
                        icon: IconName,
                        active: bool,
                        action: fn() -> Box<dyn Action>| {
            let sel_bg = c.accent;
            let on_accent = c.capsule_on_accent();
            let icon_col = if active { on_accent } else { c.toolbar_text };
            Button::new(id)
                .debug_selector(move || id.into())
                .accessibility_label(label)
                .cursor_pointer()
                .w(px(if compact { 22. } else { 25. }))
                .h(px(strip_h))
                .rounded(row_radius)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(if active { sel_bg } else { 0x0000_0000 }))
                .text_color(rgba(icon_col))
                .hover(move |s| s.bg(rgba(sel_bg)).text_color(rgba(on_accent)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                .child(Icon::new(icon).size_3p5())
        };

        let strip = div()
            .h(px(strip_h))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(icon_btn(
                "pin-menu-annotate",
                crate::i18n::t!("pin.menu.annotate").as_ref(),
                IconName::Pencil,
                is_annot,
                || Box::new(TogglePinAnnotate),
            ))
            .child(icon_btn(
                "pin-menu-rot-ccw",
                crate::i18n::t!("pin.menu.rotate_ccw").as_ref(),
                IconName::RotateCcw,
                false,
                || Box::new(RotatePinCcw),
            ))
            .child(icon_btn(
                "pin-menu-rot-cw",
                crate::i18n::t!("pin.menu.rotate_cw").as_ref(),
                IconName::RotateCw,
                false,
                || Box::new(RotatePinCw),
            ))
            .child(icon_btn(
                "pin-menu-flip-h",
                crate::i18n::t!("pin.menu.flip_h").as_ref(),
                IconName::FlipHorizontal2,
                false,
                || Box::new(FlipPinHorizontal),
            ))
            .child(icon_btn(
                "pin-menu-flip-v",
                crate::i18n::t!("pin.menu.flip_v").as_ref(),
                IconName::FlipVertical2,
                false,
                || Box::new(FlipPinVertical),
            ))
            .child(icon_btn(
                "pin-menu-gray",
                crate::i18n::t!("pin.menu.grayscale").as_ref(),
                IconName::Droplet,
                is_gray,
                || Box::new(TogglePinGrayscale),
            ))
            .child(icon_btn(
                "pin-menu-invert",
                crate::i18n::t!("pin.menu.invert").as_ref(),
                IconName::Contrast,
                is_inv,
                || Box::new(TogglePinInvert),
            ))
            .child(icon_btn(
                "pin-menu-collapse",
                crate::i18n::t!("pin.menu.collapse").as_ref(),
                if is_collapsed {
                    IconName::Maximize2
                } else {
                    IconName::Minimize2
                },
                is_collapsed,
                || Box::new(TogglePinCollapse),
            ))
            .child(icon_btn(
                "pin-menu-ghost",
                crate::i18n::t!("pin.menu.click_through").as_ref(),
                if is_ghost {
                    IconName::Lock
                } else {
                    IconName::LockOpen
                },
                is_ghost,
                || Box::new(TogglePinGhost),
            ));

        let status_bar = div()
            .h(px(bar_h))
            .w_full()
            .px_1()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(rgba(c.toolbar_text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("pin-opacity-down")
                            .debug_selector(|| "pin-opacity-down".into())
                            .cursor_pointer()
                            .w(px(20.))
                            .h(px(bar_h - 2.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgba(c.toolbar_text))
                            .hover(move |s| {
                                s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent()))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(StepPinOpacityDown), cx);
                            })
                            .child(Icon::new(IconName::Minus).size_3()),
                    )
                    .child(
                        div()
                            .min_w(px(42.))
                            .text_center()
                            .child(format!("{opacity_pct}%")),
                    )
                    .child(
                        Button::new("pin-opacity-up")
                            .debug_selector(|| "pin-opacity-up".into())
                            .cursor_pointer()
                            .w(px(20.))
                            .h(px(bar_h - 2.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgba(c.toolbar_text))
                            .hover(move |s| {
                                s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent()))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(StepPinOpacityUp), cx);
                            })
                            .child(Icon::new(IconName::Plus).size_3()),
                    ),
            )
            .child(
                Button::new("pin-zoom")
                    .debug_selector(|| "pin-zoom".into())
                    .accessibility_label(crate::i18n::t!("pin.menu.reset_zoom").as_ref())
                    .cursor_pointer()
                    .px_1p5()
                    .h(px(bar_h - 2.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(rgba(c.toolbar_text))
                    .hover(move |s| s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent())))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ResetPinZoom), cx))
                    .child(format!(
                        "{} ({zoom_pct}%)",
                        crate::i18n::t!("pin.menu.reset_zoom")
                    )),
            );

        let sep_col = c.capsule_border();
        let sep = || div().w_full().h(px(sep_h)).bg(rgba(sep_col));

        let item = |id: &'static str,
                    label: &str,
                    shortcut: &'static str,
                    action: fn() -> Box<dyn Action>| {
            Button::new(id)
                .debug_selector(move || id.into())
                .accessibility_label(label)
                .cursor_pointer()
                .w_full()
                .h(px(item_h))
                .px_2()
                .rounded(row_radius)
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(rgba(c.toolbar_text))
                .hover(move |s| s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent())))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                .child(label.to_string())
                .child(
                    div()
                        .text_color(rgba(c.hint_text))
                        .child(shortcut.to_string()),
                )
        };

        let history_row = div()
            .w_full()
            .h(px(item_h))
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("pin-history")
                    .debug_selector(|| "pin-history".into())
                    .accessibility_label(crate::i18n::t!("pin.menu.history").as_ref())
                    .cursor_pointer()
                    .flex_1()
                    .h_full()
                    .px_2()
                    .rounded(row_radius)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgba(c.toolbar_text))
                    .hover(move |s| s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent())))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenPinHistory), cx))
                    .child(crate::i18n::t!("pin.menu.history").to_string())
                    .child(div().text_color(rgba(c.hint_text)).child("Ctrl+H")),
            )
            .children(can_restore.then(|| {
                Button::new("pin-restore-last")
                    .debug_selector(|| "pin-restore-last".into())
                    .accessibility_label(crate::i18n::t!("pin.menu.restore_closed").as_ref())
                    .cursor_pointer()
                    .px_1p5()
                    .h_full()
                    .rounded(row_radius)
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(rgba(c.accent))
                    .hover(move |s| s.bg(rgba(c.accent)).text_color(rgba(c.capsule_on_accent())))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(RestoreClosedPin), cx);
                    })
                    .child(Icon::new(IconName::Undo2).size_3())
            }));

        let mut menu_div = div()
            .id("pin-menu")
            .debug_selector(|| "pin-menu".into())
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(px(menu_w))
            .h(px(menu_h))
            .p(px(pad))
            .flex()
            .flex_col()
            .gap(px(gap))
            .overflow_hidden()
            .border_1()
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        menu_div = menu_div
            .rounded(px(12.))
            .border_color(rgba(c.capsule_border()))
            .bg(rgba(c.capsule_sheet_surface()))
            .shadow(theme::capsule_floating_shadow());
        let mut menu_div = menu_div
            .child(strip)
            .child(sep())
            .child(status_bar)
            .child(sep())
            .child(item(
                "pin-copy",
                crate::i18n::t!("common.copy").as_ref(),
                "Ctrl+C",
                || Box::new(CopyPin),
            ))
            .child(item(
                "pin-save",
                crate::i18n::t!("common.save").as_ref(),
                "Ctrl+S",
                || Box::new(SavePin),
            ))
            .child(item(
                "pin-ocr",
                crate::i18n::t!("pin.menu.ocr").as_ref(),
                "Ctrl+O",
                || Box::new(OcrPin),
            ))
            .child(item(
                "pin-qr",
                crate::i18n::t!("pin.menu.qr").as_ref(),
                "Ctrl+Q",
                || Box::new(QrPin),
            ))
            .child(item(
                "pin-paste",
                crate::i18n::t!("pin.menu.paste").as_ref(),
                "Ctrl+V",
                || Box::new(PastePin),
            ))
            .child(history_row);

        if has_any_ghost {
            menu_div = menu_div.child(item(
                "pin-unlock-all",
                crate::i18n::t!("pin.menu.unlock_all").as_ref(),
                "",
                || Box::new(UnlockAllPins),
            ));
        }

        menu_div = menu_div.child(item(
            "pin-close",
            crate::i18n::t!("common.close").as_ref(),
            "Esc",
            || Box::new(ClosePin),
        ));

        Some(menu_div)
    }
}

/// Compact top-left status badge inside a floating pin (zoom %, opacity %, OCR…).
pub(super) fn render_hud_pill(text: &str) -> Div {
    let c = theme::c();
    div()
        .absolute()
        .left(px(6.))
        .top(px(6.))
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(rgba(c.capsule_surface()))
        .border_1()
        .border_color(rgba(c.capsule_border()))
        .text_xs()
        .text_color(rgba(c.toolbar_text))
        .child(text.to_owned())
}
