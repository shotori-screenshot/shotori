//! # OCR OSD: the tiny "recognizing…" chip
//!
//! The full-screen overlays unmap the moment OCR starts (the desktop must
//! stay usable — see overlay's `exit_into_background_ocr`); this chip is
//! the only in-flight feedback. It is a NON-interactive layer-shell
//! surface: `KeyboardInteractivity::None` never steals typing from the app
//! the user went back to, and the top-center anchor (the volume-OSD slot)
//! keeps it away from where the user is working. It spins until the result
//! notification lands, then the process quits.

use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::ui::theme;

/// The chip's logical surface size; the visible pill is content-sized and
/// centered inside — the slack keeps longer locale strings from clipping.
const OSD_W: f32 = 180.;
const OSD_H: f32 = 48.;

pub(crate) struct OcrOsd;

impl Render for OcrOsd {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .rounded_lg()
                    .bg(rgba(theme::c().chip_bg))
                    .border_1()
                    .border_color(rgba(theme::c().accent))
                    .child(spinner())
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgba(theme::c().hint_text))
                            .child(crate::i18n::t!("osd.ocr_running").to_string()),
                    ),
            )
    }
}

/// Spinner: a faint ring with one accent dot orbiting inside. Pure element
/// properties animated via `with_animation` (respects reduce_motion;
/// max_fps caps the redraw rate).
fn spinner() -> impl IntoElement {
    const R: f32 = 7.; // orbit radius
    const BOX: f32 = 2. * R + 5.; // container edge
    const CENTER: f32 = BOX / 2.;
    const DOT: f32 = 4.;

    div()
        .id("shotori-spinner")
        .relative()
        .size(px(BOX))
        // faint ring for context
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(CENTER))
                .border_1()
                .border_color(rgba(theme::c().pin_border)),
        )
        // the orbiting dot
        .child(
            div()
                .absolute()
                .size(px(DOT))
                .rounded(px(DOT / 2.))
                .bg(rgba(theme::c().accent))
                .with_animation(
                    "shotori-spin",
                    Animation::new(std::time::Duration::from_millis(900))
                        .repeat()
                        .with_max_fps(15.),
                    move |dot, delta| {
                        let a = delta * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                        let (dx, dy) = (a.cos() * R, a.sin() * R);
                        dot.left(px(CENTER + dx - DOT / 2.))
                            .top(px(CENTER + dy - DOT / 2.))
                    },
                ),
        )
}

fn window_options(display_id: Option<DisplayId>) -> WindowOptions {
    let kind = WindowKind::LayerShell(LayerShellOptions {
        namespace: "shotori-ocr-osd".into(),
        layer: Layer::Overlay,
        // Top edge only: the one-edge anchor centers the chip horizontally
        // (the volume-OSD slot) and pins it to the top of the output
        anchor: Anchor::TOP,
        // Zone 0 = claim no exclusive space but RESPECT other surfaces':
        // a top panel tucks the chip below itself instead of under it
        exclusive_zone: Some(px(0.)),
        margin: Some((px(14.), px(0.), px(0.), px(0.))),
        // The chip must never interact — the user went back to their app
        // the moment the overlays unmapped
        keyboard_interactivity: KeyboardInteractivity::None,
        ..Default::default()
    });

    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(OSD_W), px(OSD_H)),
        })),
        kind,
        ..Default::default()
    }
}

/// Open the chip on `display_id` — the triggering output's display, NOT
/// the compositor's choice (the mixed-DPI placement lottery, the same rule
/// the overlay's window options follow). Best-effort by design: `None`
/// only degrades feedback to the completion notification; it never fails
/// the OCR itself.
pub(crate) fn open(display_id: Option<DisplayId>, cx: &mut App) -> Option<AnyWindowHandle> {
    match cx.open_window(window_options(display_id), |_, cx| cx.new(|_| OcrOsd)) {
        Ok(handle) => Some(handle.into()),
        Err(e) => {
            eprintln!("[shotori] could not open the OCR indicator: {e:#}");
            None
        }
    }
}
