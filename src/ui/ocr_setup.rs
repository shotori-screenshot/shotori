//! # First-run OCR setup UI: confirm dialog → progress bar → cancel/retry
//!
//! Shown inside the overlay when Ctrl+O is pressed and no models are cached.
//! The download itself lives in [`crate::ocr::spawn_download`]; this module
//! only owns UI state and rendering. The overlay polls the shared progress
//! at ~80ms and transitions on completion.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::base::Button;
use gpui_kit::*;

use crate::model::settings::UiStyle;
use crate::ocr::DownloadProgress;
use crate::ui::theme;

gpui_kit::actions!([
    OcrSetupConfirm,
    OcrSetupCancel,
    OcrSetupNext,
    OcrSetupPrevious
]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", OcrSetupNext, Some("ShotoriOcrSetup")),
        KeyBinding::new("l", OcrSetupNext, Some("ShotoriOcrSetup")),
        KeyBinding::new("left", OcrSetupPrevious, Some("ShotoriOcrSetup")),
        KeyBinding::new("h", OcrSetupPrevious, Some("ShotoriOcrSetup")),
        KeyBinding::new("escape", OcrSetupCancel, Some("ShotoriOcrSetup")),
    ]);
}

/// Retained button identities survive progress renders and stage changes.
pub(crate) struct SetupFocus {
    confirm: FocusHandle,
    cancel: FocusHandle,
}

impl SetupFocus {
    pub fn new(cx: &mut App) -> Self {
        Self {
            confirm: cx.focus_handle(),
            cancel: cx.focus_handle(),
        }
    }

    pub fn focus_confirm(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.confirm, cx);
    }

    pub fn focus_cancel(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.cancel, cx);
    }
}

/// Cropped pixels frozen at Ctrl+O time. The selection may change while the
/// dialog is open; the frozen screen cannot — so we keep the exact crop the
/// user OCR'd and run it once the download finishes.
pub(crate) struct Snapshot {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

pub(crate) enum Stage {
    /// Awaiting the user's decision
    Confirm,
    /// Downloading; the overlay polls `progress`
    Downloading,
    /// Download or post-download OCR failed; message shown
    Failed(String),
}

pub(crate) struct OcrSetup {
    pub stage: Stage,
    pub snapshot: Snapshot,
    pub progress: Arc<DownloadProgress>,
}

impl OcrSetup {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            stage: Stage::Confirm,
            snapshot,
            progress: Arc::new(crate::ocr::DownloadProgress::for_selected_set()),
        }
    }

    /// Only the active download may update this dialog or consume its snapshot.
    pub fn owns_download(&self, progress: &Arc<DownloadProgress>) -> bool {
        Arc::ptr_eq(&self.progress, progress)
            && !progress.cancel.load(Ordering::Relaxed)
            && matches!(self.stage, Stage::Downloading)
    }
}

/// The setup card: dim backdrop + centered card, rendered above everything
/// else in the overlay. Clicks inside are swallowed (no new selections).
pub(crate) fn setup_card(style: UiStyle, setup: &OcrSetup, focus: &SetupFocus) -> impl IntoElement {
    div()
        .id("shotori-ocr-setup")
        .key_context("ShotoriOcrSetup")
        .tab_group()
        // Native Button also activates on Space; this dialog uses Enter only.
        .capture_key_down(|event, window, cx| {
            if event.keystroke.key == "space" {
                window.prevent_default();
                cx.stop_propagation();
            }
        })
        .capture_key_up(|event, window, cx| {
            if event.keystroke.key == "space" {
                window.prevent_default();
                cx.stop_propagation();
            }
        })
        // The overlay removes its toolbar while this modal is open, so these
        // are the window's only tab stops. GPUI cycles in both directions.
        .on_action(|_: &OcrSetupNext, window, cx| {
            window.focus_next(cx);
            cx.stop_propagation();
        })
        .on_action(|_: &OcrSetupPrevious, window, cx| {
            window.focus_prev(cx);
            cx.stop_propagation();
        })
        .absolute()
        .size_full()
        .left_0()
        .top_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(theme::c().dim()))
        // Any button on the backdrop (or on the card) must not reach the
        // overlay underneath: middle-click/right-click would start a new
        // selection through the modal.
        .on_mouse_down(MouseButton::Left, swallow)
        .on_mouse_down(MouseButton::Middle, swallow)
        .on_mouse_down(MouseButton::Right, swallow)
        .child(card_body(style, setup, focus))
}

/// Shared stop_propagation handler for the modal's button swallowing.
fn swallow(_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

fn card_body(style: UiStyle, setup: &OcrSetup, focus: &SetupFocus) -> impl IntoElement {
    const CARD_W: f32 = 460.;

    let (title, body, buttons): (String, Vec<AnyElement>, Vec<AnyElement>) = match &setup.stage {
        Stage::Confirm => (
            crate::i18n::t!("ocr.setup.title").to_string(),
            // Two lines on purpose: the question and the cache path are
            // separate statements, and text_line flattens \n — one run-on
            // paragraph here read badly with long cache paths.
            vec![
                text_line(
                    style,
                    &crate::i18n::t!(
                        "ocr.setup.confirm",
                        model = crate::model::settings::current().ocr_model().label()
                    ),
                ),
                text_line(
                    style,
                    &crate::i18n::t!("ocr.setup.stored_at", dir = crate::ocr::model_dir_display()),
                ),
            ],
            if style == UiStyle::Capsule {
                // Alert sheet button order: Cancel on the left, Primary action on the right
                vec![
                    action_button(
                        style,
                        false,
                        "ocr-setup-no",
                        crate::i18n::t!("ocr.setup.cancel").as_ref(),
                        OcrSetupCancel,
                        &focus.cancel,
                    ),
                    action_button(
                        style,
                        true,
                        "ocr-setup-go",
                        crate::i18n::t!("ocr.setup.download").as_ref(),
                        OcrSetupConfirm,
                        &focus.confirm,
                    ),
                ]
            } else {
                vec![
                    action_button(
                        style,
                        true,
                        "ocr-setup-go",
                        crate::i18n::t!("ocr.setup.download").as_ref(),
                        OcrSetupConfirm,
                        &focus.confirm,
                    ),
                    action_button(
                        style,
                        false,
                        "ocr-setup-no",
                        crate::i18n::t!("ocr.setup.cancel").as_ref(),
                        OcrSetupCancel,
                        &focus.cancel,
                    ),
                ]
            },
        ),
        Stage::Downloading => {
            let p = &setup.progress;
            let bytes = p.bytes.load(Ordering::Relaxed);
            let total = p.total.load(Ordering::Relaxed);
            let idx = p.file_idx.load(Ordering::Relaxed);
            let file = crate::ocr::asset_name(idx);
            let pct = if total > 0 {
                (bytes as f32 / total as f32).clamp(0., 1.)
            } else {
                0.
            };
            let mb = |v: u64| format!("{:.1} MB", v as f64 / 1e6);
            // Set-wide line: with only the per-file bar, file 1 of a
            // 5-file set looks stalled for hundreds of MB.
            let done = p.done_bytes.load(Ordering::Relaxed);
            let overall = format!(
                "{} · {} + {}",
                crate::i18n::t!("ocr.setup.overall", idx = idx, count = p.file_count),
                mb(done),
                mb(bytes)
            );
            (
                crate::i18n::t!("ocr.setup.downloading").to_string(),
                vec![
                    text_line(style, &overall),
                    text_line(style, &format!("{file} ({idx}/{})", p.file_count)),
                    progress_bar(style, pct, CARD_W - 64.),
                    text_line(
                        style,
                        &if total > 0 {
                            format!("{} / {}", mb(bytes), mb(total))
                        } else {
                            mb(bytes)
                        },
                    ),
                ],
                vec![action_button(
                    style,
                    false,
                    "ocr-setup-abort",
                    crate::i18n::t!("ocr.setup.cancel").as_ref(),
                    OcrSetupCancel,
                    &focus.cancel,
                )],
            )
        }
        Stage::Failed(msg) => (
            crate::i18n::t!("ocr.setup.failed").to_string(),
            vec![text_line(style, &truncate(msg, 320))],
            if style == UiStyle::Capsule {
                vec![
                    action_button(
                        style,
                        false,
                        "ocr-setup-close",
                        crate::i18n::t!("ocr.setup.close").as_ref(),
                        OcrSetupCancel,
                        &focus.cancel,
                    ),
                    action_button(
                        style,
                        true,
                        "ocr-setup-retry",
                        crate::i18n::t!("ocr.setup.retry").as_ref(),
                        OcrSetupConfirm,
                        &focus.confirm,
                    ),
                ]
            } else {
                vec![
                    action_button(
                        style,
                        true,
                        "ocr-setup-retry",
                        crate::i18n::t!("ocr.setup.retry").as_ref(),
                        OcrSetupConfirm,
                        &focus.confirm,
                    ),
                    action_button(
                        style,
                        false,
                        "ocr-setup-close",
                        crate::i18n::t!("ocr.setup.close").as_ref(),
                        OcrSetupCancel,
                        &focus.cancel,
                    ),
                ]
            },
        ),
    };

    let t = theme::c();
    let mut card = div()
        .w(px(CARD_W))
        .flex()
        .flex_col()
        .gap_2()
        .on_mouse_down(MouseButton::Left, swallow)
        .on_mouse_down(MouseButton::Middle, swallow)
        .on_mouse_down(MouseButton::Right, swallow);
    card = match style {
        UiStyle::Classic => card
            .rounded(px(14.))
            .bg(rgba(t.chip_bg))
            .border_1()
            .border_color(rgba(t.accent))
            .p_4(),
        UiStyle::Capsule => card
            .rounded(px(18.))
            .bg(rgba(t.capsule_sheet_surface()))
            .border_1()
            .border_color(rgba(t.capsule_border()))
            .shadow(theme::capsule_floating_shadow())
            .p_5(),
    };
    let mut title_el = div().text_size(px(15.)).child(title);
    title_el = match style {
        UiStyle::Classic => title_el.text_color(rgba(t.btn_text)),
        UiStyle::Capsule => title_el
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgba(t.toolbar_text)),
    };
    card.child(title_el)
        .children(body)
        .child(div().flex().justify_end().gap_2().pt_2().children(buttons))
}

fn text_line(style: UiStyle, s: &str) -> AnyElement {
    let fg = match style {
        UiStyle::Classic => theme::c().hint_text,
        UiStyle::Capsule => theme::c().hint_text,
    };
    div()
        .text_size(px(13.))
        .text_color(rgba(fg))
        .line_height(px(19.))
        .child(s.replace('\n', " "))
        .into_any_element()
}

fn progress_bar(style: UiStyle, pct: f32, w: f32) -> AnyElement {
    let h = if style == UiStyle::Capsule { 6. } else { 8. };
    let r = h * 0.5;
    div()
        .w(px(w))
        .h(px(h))
        .rounded(px(r))
        .bg(rgba(if style == UiStyle::Capsule {
            theme::alpha(theme::c().toolbar_border, 0x88)
        } else {
            theme::c().btn_hover_bg
        }))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(px(w * pct))
                .rounded(px(r))
                .bg(rgba(theme::c().accent)),
        )
        .into_any_element()
}

fn action_button<A: Action + Clone + 'static>(
    style: UiStyle,
    primary: bool,
    id: &'static str,
    label: &str,
    action: A,
    focus: &FocusHandle,
) -> AnyElement {
    let t = theme::c();
    let base = Button::new(id)
        .track_focus(focus)
        .accessibility_label(label)
        .text_size(px(13.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            window.dispatch_action(Box::new(action.clone()), cx);
        });
    let styled = match style {
        UiStyle::Classic => base
            .px_3()
            .py_1()
            .rounded(px(6.))
            .text_color(rgba(t.btn_text))
            .border_1()
            .border_color(rgba(t.pin_border))
            .hover(move |s| s.bg(rgba(t.btn_hover_bg)))
            .focus_visible(move |s| s.border_color(rgba(t.accent))),
        UiStyle::Capsule => {
            if primary {
                base.px_4()
                    .py(px(5.))
                    .rounded(px(8.))
                    .font_weight(FontWeight::MEDIUM)
                    .bg(rgba(t.accent))
                    .text_color(rgba(t.capsule_on_accent()))
                    .border_1()
                    .border_color(rgba(t.accent))
                    .hover(move |s| s.opacity(0.92))
                    .focus_visible(move |s| s.border_color(rgba(t.toolbar_text)))
            } else {
                base.px_4()
                    .py(px(5.))
                    .rounded(px(8.))
                    .bg(rgba(t.capsule_hover()))
                    .text_color(rgba(t.toolbar_text))
                    .border_1()
                    .border_color(rgba(t.capsule_border()))
                    .hover(move |s| s.bg(rgba(t.toolbar_hover)))
                    .focus_visible(move |s| s.border_color(rgba(t.accent)))
            }
        }
    };
    styled.child(label.to_string()).into_any_element()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::{OcrSetup, Snapshot, Stage};
    use crate::ocr::DownloadProgress;
    use std::sync::{Arc, atomic::Ordering};

    fn setup() -> OcrSetup {
        OcrSetup::new(Snapshot {
            w: 1,
            h: 1,
            rgba: vec![0; 4],
        })
    }

    #[test]
    fn cancelled_download_cannot_update_reopened_dialog() {
        let mut old_setup = setup();
        old_setup.stage = Stage::Downloading;
        let old = old_setup.progress.clone();
        assert!(old_setup.owns_download(&old));
        old.cancel.store(true, Ordering::Relaxed);
        assert!(!old_setup.owns_download(&old));

        let mut reopened = setup();
        assert!(!reopened.owns_download(&old));
        reopened.stage = Stage::Downloading;
        assert!(!reopened.owns_download(&old));
        assert!(reopened.owns_download(&reopened.progress));
    }

    #[test]
    fn retry_rejects_previous_attempt_even_without_cancellation() {
        let mut setup = setup();
        setup.stage = Stage::Downloading;
        let old = setup.progress.clone();
        setup.stage = Stage::Failed("network error".into());
        assert!(!setup.owns_download(&old));
        setup.progress = Arc::new(DownloadProgress::default());
        setup.stage = Stage::Downloading;
        assert!(!setup.owns_download(&old));
        assert!(setup.owns_download(&setup.progress));
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::{
        OcrSetup, OcrSetupCancel, OcrSetupConfirm, SetupFocus, Snapshot, Stage, init, setup_card,
    };
    use gpui_kit::{
        App, Context, Entity, FocusHandle, IntoElement, KeyBinding, Render, TestAppContext,
        VisualTestContext, Window,
    };
    use gpui_kit::{InteractiveElement, ParentElement, Styled, div};

    gpui_kit::actions!([CopyBehindDialog]);

    struct Harness {
        setup: OcrSetup,
        focus: SetupFocus,
        root_focus: FocusHandle,
        closed: bool,
        confirmations: usize,
        cancellations: usize,
        copies: usize,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("keyboard-harness")
                .size_full()
                .key_context(if self.closed {
                    "ShotoriOverlay"
                } else {
                    "ShotoriOcrSetup"
                })
                .track_focus(&self.root_focus)
                .on_action(cx.listener(|this, _: &OcrSetupConfirm, window, cx| {
                    this.confirmations += 1;
                    this.setup.stage = Stage::Downloading;
                    this.focus.focus_cancel(window, cx);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &OcrSetupCancel, window, cx| {
                    this.cancellations += 1;
                    this.closed = true;
                    window.focus(&this.root_focus, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &CopyBehindDialog, _, _| {
                    this.copies += 1;
                }))
                .children(
                    (!self.closed)
                        .then(|| setup_card(super::UiStyle::Classic, &self.setup, &self.focus)),
                )
        }
    }

    fn harness(cx: &mut TestAppContext, stage: Stage) -> (Entity<Harness>, &mut VisualTestContext) {
        cx.update(|cx: &mut App| {
            gpui_kit::base::init(cx);
            init(cx);
            cx.bind_keys([
                KeyBinding::new("enter", CopyBehindDialog, Some("ShotoriOverlay")),
                KeyBinding::new("ctrl-c", CopyBehindDialog, Some("ShotoriOverlay")),
                KeyBinding::new("ctrl-s", CopyBehindDialog, Some("ShotoriOverlay")),
            ]);
        });
        let (entity, cx) = cx.add_window_view(move |window, cx| {
            let focus = SetupFocus::new(cx);
            if matches!(stage, Stage::Downloading) {
                focus.focus_cancel(window, cx);
            } else {
                focus.focus_confirm(window, cx);
            }
            let mut setup = OcrSetup::new(Snapshot {
                w: 1,
                h: 1,
                rgba: vec![0; 4],
            });
            setup.stage = stage;
            Harness {
                setup,
                focus,
                root_focus: cx.focus_handle(),
                closed: false,
                confirmations: 0,
                cancellations: 0,
                copies: 0,
            }
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        (entity, cx)
    }

    // GPUI's simulate_keystrokes emits key-down only. Native Button clicks
    // require a complete press/release pair.
    fn press(cx: &mut VisualTestContext, keys: &str) {
        for key in keys.split_whitespace() {
            cx.simulate_keystrokes(key);
            cx.simulate_event(gpui_kit::KeyUpEvent {
                keystroke: crate::actions::parse_keystroke(key).unwrap(),
            });
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
        }
    }

    #[gpui_kit::test]
    fn dialog_keys_cycle_confirm_ignore_space_and_shield_overlay(cx: &mut TestAppContext) {
        let (view, cx) = harness(cx, Stage::Confirm);
        press(cx, "space right space left h l");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.confirmations, 0);
            assert_eq!(view.cancellations, 0);
            assert!(!view.closed);
            assert!(view.focus.confirm.is_focused(window));
        });
        press(cx, "ctrl-c ctrl-s enter space");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.confirmations, 1);
            assert_eq!(view.copies, 0);
            assert!(view.focus.cancel.is_focused(window));
        });
        press(cx, "enter");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.cancellations, 1);
            assert!(view.closed);
            assert!(view.root_focus.is_focused(window));
            assert_eq!(view.copies, 0);
        });
        press(cx, "enter");
        cx.update(|_, cx| assert_eq!(view.read(cx).copies, 1));
    }

    #[gpui_kit::test]
    fn failed_retry_and_escape_cancel_restore_focus(cx: &mut TestAppContext) {
        let (view, cx) = harness(cx, Stage::Failed("test network error".into()));
        press(cx, "enter");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.confirmations, 1);
            assert!(view.focus.cancel.is_focused(window));
        });
        press(cx, "escape");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.closed);
            assert_eq!(view.cancellations, 1);
            assert!(view.root_focus.is_focused(window));
        });
    }
}
