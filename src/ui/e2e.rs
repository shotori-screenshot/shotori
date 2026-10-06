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

use gpui_kit::*;

use crate::actions::{ClearAnnotations, CopySelection, OcrSelection, QuitOverlay, SaveSelection};
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
    }) else {
        return;
    };
    let win = window.window_handle();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(1500))
            .await;
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
