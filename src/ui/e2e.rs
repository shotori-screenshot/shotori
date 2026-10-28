//! # E2E debug backdoors: entry points for automated headless tests
//!
//! Normal launches are unaffected — everything here is gated on
//! `SHOTORI_DEBUG_*` environment variables:
//! - `SHOTORI_DEBUG_TARGET=<output name>` — with one overlay per screen
//!   all running the same code, enabling all of them makes them fight
//!   each other; this scopes the other two knobs to one overlay
//! - `SHOTORI_DEBUG_SELECTION=x,y,w,h` — inject a ready-made selection
//! - `SHOTORI_DEBUG_ACTION=copy|save|pin|quit|ocr|ocrsetup|clear` —
//!   fire the action(s) automatically after 1.5s; the only entry point
//!   for headless e2e (the virtual pointer is dead on niri). quit/ocr
//!   go through the real dispatch_action pipeline;
//!   "ocrsetup" drives the full first-run flow: OcrSelection at 1.5s
//!   (opens the dialog since models are missing), then
//!   OcrSetupConfirm at 6s (starts the download); "clear" fires the
//!   clear-all annotations action (issue #15)
//!   "textnew", "textmiddle", "textedit" and "textempty" open synthetic
//!   text editing states through keyboard and pointer dispatch
//! - `SHOTORI_DEBUG_EDITING_SCENE=hover|selected|reselect|cancel` —
//!   seed annotation and crop interaction states on the target output
//! - `SHOTORI_DEBUG_SETTINGS_PAGE=general|toolbar|keys|ocr|appearance|about`
//!   — open the settings window on that page. A headless seat has no
//!   pointer AND no keyboard (docs/headless-e2e.md), so without this
//!   there is no way to reach a page while reviewing it

use gpui_kit::*;

use crate::actions::{
    ClearAnnotations, CopySelection, OcrSelection, QuitOverlay, SaveSelection, parse_keystroke,
};
use crate::model::selection::Selection;
use crate::ui::overlay::Overlay;

/// Does the backdoor target this overlay? Unset = enabled everywhere.
pub(crate) fn debug_targeted(output_name: &str) -> bool {
    std::env::var("SHOTORI_DEBUG_TARGET")
        .map(|t| t == output_name)
        .unwrap_or(true)
}

/// Inject a ready-made selection (no-op unless targeted).
pub(crate) fn debug_selection(targeted: bool) -> Selection {
    if !targeted {
        return Selection::Idle;
    }
    std::env::var("SHOTORI_DEBUG_SELECTION")
        .ok()
        .and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            (v.len() == 4).then(|| Selection::Selected {
                bounds: Bounds {
                    origin: point(px(v[0]), px(v[1])),
                    size: size(px(v[2]), px(v[3])),
                },
            })
        })
        .unwrap_or(Selection::Idle)
}

/// Seed crop editing and hover states on a private headless display.
pub(crate) fn debug_editing_scene(
    targeted: bool,
    session: &mut crate::model::session::ScreenshotSession,
    output: &str,
) {
    if !targeted {
        return;
    }
    let Ok(scene) = std::env::var("SHOTORI_DEBUG_EDITING_SCENE") else {
        return;
    };
    if !matches!(scene.as_str(), "hover" | "selected" | "reselect" | "cancel") {
        return;
    }
    let Some(bounds) = session.selection().bounds() else {
        return;
    };
    let p = |x, y| bounds.origin + point(px(x), px(y));
    session.edit_annotations(|a| {
        a.toggle(crate::annotation::ShapeKind::Rectangle);
        a.begin(p(80., 70.), bounds, false);
        a.drag_to(p(290., 220.), bounds, false);
        a.end();
        a.toggle(crate::annotation::ShapeKind::Arrow);
        a.begin(p(350., 150.), bounds, false);
        a.drag_to(p(530., 150.), bounds, false);
        a.end();
        a.toggle(crate::annotation::ShapeKind::Select);
    });
    let origin = session.to_global(output, point(px(0.), px(0.)));
    let local = |x, y| p(x, y) - origin;
    session.pointer_move(output, local(180., 66.), false);
    if scene == "selected" {
        session.pointer_down(output, local(180., 66.), false);
        session.pointer_up(output, local(180., 66.), false);
        session.nudge(point(px(10.), px(10.)));
        session.finish_nudge();
    } else if scene == "reselect" || scene == "cancel" {
        session.pointer_down(output, local(-60., -60.), false);
        session.pointer_move(output, local(100., 100.), false);
        if scene == "cancel" {
            session.cancel_annotation();
        }
    }
}

/// Which settings page to open on. `None` (unset or unknown name) keeps
/// the normal default — the settings window maps the name itself.
pub(crate) fn debug_settings_page() -> Option<String> {
    std::env::var("SHOTORI_DEBUG_SETTINGS_PAGE")
        .ok()
        .filter(|page| !page.is_empty())
}

/// Open a settings color editor for headless inspection (`accent` or
/// `accent:hsla`). Kept here with the rest of the runtime test backdoors.
pub(crate) fn debug_settings_color() -> Option<(String, bool)> {
    let value = std::env::var("SHOTORI_DEBUG_SETTINGS_COLOR")
        .ok()
        .filter(|value| !value.is_empty())?;
    let (key, tab) = value.split_once(':').unwrap_or((&value, "palette"));
    Some((key.to_owned(), tab == "hsla"))
}

/// Inspect expanded colors and the shortcut recorder without a headless input seat.
pub(crate) fn debug_settings_custom_colors() -> bool {
    std::env::var("SHOTORI_DEBUG_SETTINGS_CUSTOM_COLORS").as_deref() == Ok("1")
}

pub(crate) fn debug_settings_recording() -> Option<String> {
    std::env::var("SHOTORI_DEBUG_SETTINGS_RECORDING")
        .ok()
        .filter(|id| {
            crate::actions::CONFIGURABLE_KEYS
                .iter()
                .any(|(key, _)| key == id)
        })
}

/// Seed search and scale in an isolated settings review session.
pub(crate) fn debug_settings_query() -> String {
    std::env::var("SHOTORI_DEBUG_SETTINGS_QUERY").unwrap_or_default()
}

pub(crate) fn debug_settings_font() -> Option<f32> {
    std::env::var("SHOTORI_DEBUG_SETTINGS_FONT")
        .ok()?
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (12.0..=24.0).contains(value))
}

/// Inspect the lower part of a settings page on a seat without input.
pub(crate) fn debug_settings_scroll(scroll: &ScrollHandle) {
    if let Ok(value) = std::env::var("SHOTORI_DEBUG_SETTINGS_SCROLL")
        && let Ok(offset) = value.parse::<f32>()
        && offset.is_finite()
    {
        scroll.set_offset(point(px(0.), px(-offset.max(0.))));
    }
}

/// Fire the configured action(s) after 1.5s.
pub(crate) fn spawn_debug_action(window: &mut Window, cx: &mut Context<Overlay>) {
    let Some(action) = std::env::var("SHOTORI_DEBUG_ACTION").ok().filter(|a| {
        a == "copy"
            || a == "quit"
            || a == "save"
            || a == "pin"
            || a == "ocr"
            || a == "ocrsetup"
            || a == "clear"
            || a == "scroll"
            || matches!(
                a.as_str(),
                "textnew" | "textmiddle" | "textedit" | "textempty"
            )
    }) else {
        return;
    };
    let win = window.window_handle();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(1500))
            .await;
        if matches!(
            action.as_str(),
            "textnew" | "textmiddle" | "textedit" | "textempty"
        ) {
            let Some(bounds) = debug_selection(true).bounds() else {
                eprintln!("[shotori] text review requires a debug selection");
                return;
            };
            let position = bounds.origin + point(px(70.), px(60.));
            if let Err(error) = win.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(crate::actions::ToggleText), cx);
            }) {
                eprintln!("[shotori] text review could not select the tool: {error}");
                return;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            if let Err(error) = win.update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        position,
                        button: MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position,
                        button: MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            }) {
                eprintln!("[shotori] text review could not start: {error}");
                return;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            if action == "textempty" {
                return;
            }
            if let Err(error) = win.update(cx, |_, window, cx| {
                // Every chord sent here is runtime text (characters, or
                // key names picked at runtime), so all of them go through
                // the same parser and the same failure report.
                fn send(chord: &str, window: &mut Window, cx: &mut App) {
                    match parse_keystroke(chord) {
                        Ok(key) => {
                            window.dispatch_keystroke(key, cx);
                        }
                        Err(error) => eprintln!("[shotori] text review key failed: {error}"),
                    }
                }
                for character in "canvas note".chars() {
                    if character == ' ' {
                        send("space", window, cx);
                    } else {
                        send(&character.to_string(), window, cx);
                    }
                }
                if action == "textedit" {
                    send("enter", window, cx);
                } else if action == "textmiddle" {
                    for key in ["home", "right", "right", "right", "right", "right", "right"] {
                        send(key, window, cx);
                    }
                }
            }) {
                eprintln!("[shotori] text review failed: {error}");
                return;
            }
            if action == "textedit" {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                if let Err(error) = win.update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::ToggleSelect), cx);
                }) {
                    eprintln!("[shotori] text review could not select the tool: {error}");
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                if let Err(error) = win.update(cx, |_, window, cx| {
                    let position = position + point(px(30.), px(15.));
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            position,
                            button: MouseButton::Left,
                            click_count: 2,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            position,
                            button: MouseButton::Left,
                            click_count: 2,
                            ..Default::default()
                        }),
                        cx,
                    );
                }) {
                    eprintln!("[shotori] text review could not reopen: {error}");
                }
            }
            return;
        }
        if action == "ocrsetup" {
            // phase 1: open the setup dialog (models must be missing)
            let _ = win.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OcrSelection), cx);
            });
            cx.background_executor()
                .timer(std::time::Duration::from_millis(4500))
                .await;
            let _ = win.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(crate::ui::ocr_setup::OcrSetupConfirm), cx);
            });
            return;
        }
        let _ = win.update(cx, |_, window, cx| {
            let action: Box<dyn gpui_kit::Action> = match action.as_str() {
                "copy" => Box::new(CopySelection),
                "save" => Box::new(SaveSelection),
                "pin" => Box::new(crate::actions::PinSelection),
                "ocr" => Box::new(OcrSelection),
                "clear" => Box::new(ClearAnnotations),
                "scroll" => Box::new(crate::actions::ScrollSelection),
                _ => Box::new(QuitOverlay),
            };
            window.dispatch_action(action, cx);
        });
    })
    .detach();
}
