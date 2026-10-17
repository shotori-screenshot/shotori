//! The post-selection exits: clipboard copy, native save dialog,
//! scroll capture, pin — plus the OCR exit (background inference) and
//! its first-run model-setup dialog.

use super::Overlay;
use crate::platform::capture::Capture;
use gpui_kit::*;

impl Overlay {
    /// Enter / Ctrl+C / toolbar [Copy]: crop → PNG (fast, background) →
    /// clipboard (resident daemon) → exit. The primary exit of daily use.
    /// The encode + clipboard handoff run on the background executor so the
    /// overlay never blocks on compression (the balanced-tier 4K encode cost
    /// ~1.4 s; the fast tier is ~45 ms).
    pub(super) fn copy_selection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some((w, h, rgba)) = self.crop(cx) else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let name = self.capture.output_name.clone();
        let entity = cx.entity();
        cx.spawn(async move |_, cx| {
            copy_to_clipboard(w, h, rgba, name, entity, cx).await;
        })
        .detach();
    }

    /// Ctrl+S / toolbar [Save]: crop → stash pixels → quit the overlay.
    /// The overlay is a layer-shell surface that would cover the native file
    /// dialog, so it exits first; the dialog itself (xdg-desktop-portal
    /// SaveFile), the write and the notification run on the main thread
    /// afterwards — see [`crate::save_dialog`]
    pub(super) fn save_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some((w, h, rgba)) = self.crop(cx) else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        println!(
            "[shotori] save: handing {w}x{h} (from {}) to the file dialog",
            self.capture.output_name
        );
        crate::save_dialog::stash(w, h, rgba);
        // Unmap all overlays — they would cover the save dialog (layer-shell
        // Overlay layer + exclusive keyboard) — and quit on a delay (see
        // quit_after_close)
        crate::save_dialog::quit_after_close(window, cx);
    }

    /// Ctrl+L / toolbar [Scroll]: the fifth exit — a long screenshot.
    /// The overlays show frozen pixels, so they must unmap for the live
    /// content to scroll; the slim control bar (`ui::scroll_bar`) is
    /// opened FIRST (never zero windows — the loop must survive the
    /// overlay teardown) and then drives the scroll engine. The bar owns
    /// the exits from here (Enter copies, Ctrl+S saves, Esc cancels).
    pub(super) fn scroll_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let session = self.session.read(cx);
        let Some(rect) = session.scroll_region(&self.capture.output_name) else {
            println!("[shotori] scroll: the selection must sit fully inside one screen");
            crate::notify::send(
                crate::i18n::t!("notify.scroll.start_failed.title").as_ref(),
                crate::i18n::t!("notify.scroll.cross_screen").as_ref(),
            );
            return;
        };
        // The bar lives on the output that hosts the selection (mixed-DPI
        // placement is a lottery otherwise — AGENTS "the 240Hz case").
        let (lw, lh) = self.capture.logical_size_f32();
        let center = point(
            px(self.capture.logical_pos.0 as f32 + lw / 2.),
            px(self.capture.logical_pos.1 as f32 + lh / 2.),
        );
        let display_id = crate::ui::pin::display_containing(center);
        let shared_rect = std::sync::Arc::new(std::sync::Mutex::new(rect));
        // Filled in by the preview panel once it knows its own height
        // (0 = chrome-free: no panel, no preview pixels rendered).
        let preview_rows = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let spec = crate::platform::scroll_capture::ScrollSpec {
            output: self.capture.output_name.clone(),
            rect: shared_rect,
            preview_rows,
            // Auto-scroll is shelved behind an env flag until the
            // injection story works on niri (see inject_step); the
            // shipped UX is manual scrolling. The e2e backdoor needs
            // the self-contained auto mode (a headless run has no hands
            // to scroll with).
            axis: crate::model::scroll_sheet::Axis::Vertical,
            mode: if std::env::var_os("SHOTORI_SCROLL_AUTO").is_some()
                || std::env::var_os("SHOTORI_DEBUG_ACTION").is_some()
            {
                crate::platform::scroll_capture::ScrollMode::Auto
            } else {
                crate::platform::scroll_capture::ScrollMode::Manual
            },
        };
        let chrome = crate::ui::scroll_bar::ScrollChrome {
            display_id,
            output_width: lw,
            output_height: lh,
        };
        if let Err(e) = crate::ui::scroll_bar::launch(spec, chrome, cx) {
            eprintln!("[shotori] scroll: could not open the control bar: {e:#}");
            crate::notify::send(
                crate::i18n::t!("notify.scroll.start_failed.title").as_ref(),
                crate::i18n::t!("notify.scroll.no_bar").as_ref(),
            );
            return;
        }
        // Overlay teardown LAST: the bar is up, the loop has a window.
        // Same flush caveat as the save flow — close_overlays removes
        // this window through its own reference.
        crate::save_dialog::close_overlays(window, cx);
    }

    /// Ctrl+P / toolbar [Pin]: crop → floating pinned layer surfaces →
    /// overlays down. The pin is one shared state plus a transparent
    /// Top-layer surface per output (see `ui::pin`) — first frame at
    /// the selection's exact spot, draggable across outputs, no
    /// compositor IPC. The app stays alive on pins and exits when the
    /// last one closes — no `cx.quit()` here, unlike copy/save.
    pub(super) fn pin_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.edit_annotations(|a| a.finish_polyline());
            cx.notify();
        });
        let Some(crop) = self
            .session
            .read(cx)
            .crop_for_pin(&self.capture.output_name)
        else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let (w, h) = (crop.width, crop.height);
        let spec = crate::ui::pin::PinSpec {
            w,
            h,
            rgba: crop.rgba,
            rect: crop.bounds,
        };
        self.session.update(cx, |session, cx| {
            session.set_blocked(true);
            cx.notify();
        });
        let prepared = cx.background_spawn(async move { crate::ui::pin::prepare(spec) });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepared.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.session.update(cx, |session, cx| {
                    session.set_blocked(false);
                    cx.notify();
                });
                match prepared.and_then(|prepared| crate::ui::pin::open(prepared, cx)) {
                    Ok(()) => {
                        println!("[shotori] pinned {w}x{h}");
                        crate::save_dialog::close_overlays(window, cx);
                    }
                    Err(error) => {
                        eprintln!("[shotori] pin failed: {error:#}");
                        crate::notify::send(
                            crate::i18n::t!("notify.pin.failed.title").as_ref(),
                            crate::i18n::t!("notify.pin.failed.body").as_ref(),
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// Ctrl+O / toolbar [OCR]. With cached models this runs immediately; on
    /// the very first use it opens the setup dialog (confirm → progress →
    /// cancel) instead — [`crate::ui::ocr_setup`].
    pub(super) fn ocr_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).blocked() {
            return; // dialog open or an OCR already running
        }
        let Some((w, h, rgba)) = self
            .session
            .read(cx)
            .crop_original(&self.capture.output_name)
        else {
            println!("[shotori] empty selection, ignoring");
            return;
        };
        let snapshot = crate::ui::ocr_setup::Snapshot { w, h, rgba };

        self.session.update(cx, |s, cx| {
            s.set_blocked(true);
            cx.notify();
        });
        if crate::ocr::models_missing() {
            self.ocr_setup = Some(Box::new(crate::ui::ocr_setup::OcrSetup::new(snapshot)));
            self.setup_focus.focus_confirm(window, cx);
            cx.notify();
        } else {
            exit_into_background_ocr(snapshot, &self.capture, window, cx);
        }
    }

    /// Setup dialog [Download]/[Retry]: switch to Downloading, spawn the
    /// download thread and start the poll loop that drives the progress bar
    /// and the completion transition. The loop holds an Arc clone of the
    /// progress and the entity handle — no shared state mutation races with
    /// the UI thread.
    pub(super) fn ocr_setup_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Retry after a failure needs a fresh progress arc — through
        // for_selected_set, NOT Default: Default carries file_count = 0
        // (its doc says it serves lock holders that never render), and
        // this arc DOES render — a Default here showed "(2/0)" in the
        // file counter.
        let fresh = std::sync::Arc::new(crate::ocr::DownloadProgress::for_selected_set());
        if let Some(setup) = self.ocr_setup.as_mut() {
            if !matches!(setup.stage, crate::ui::ocr_setup::Stage::Confirm)
                && !matches!(setup.stage, crate::ui::ocr_setup::Stage::Failed(_))
            {
                return; // already downloading
            }
            setup.progress = fresh;
            setup.stage = crate::ui::ocr_setup::Stage::Downloading;
        } else {
            return; // nothing to confirm (no dialog open)
        }
        let Some(setup) = self.ocr_setup.as_ref() else {
            return;
        };
        let progress = setup.progress.clone();
        self.setup_focus.focus_cancel(window, cx);
        crate::ocr::spawn_download(progress.clone());
        cx.notify();

        let entity = cx.entity();
        let window_handle = window.window_handle();

        cx.spawn(async move |_, cx| {
            let mut last = (0u64, 0u64, 0u8);
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                if !entity.update(cx, |this, _| {
                    this.ocr_setup
                        .as_ref()
                        .is_some_and(|s| s.owns_download(&progress))
                }) {
                    return; // cancelled, closed, or replaced by a newer attempt
                }
                if !progress.is_running() {
                    break;
                }
                let readout = (
                    progress.bytes.load(std::sync::atomic::Ordering::Relaxed),
                    progress.total.load(std::sync::atomic::Ordering::Relaxed),
                    progress.file_idx.load(std::sync::atomic::Ordering::Relaxed),
                );
                if readout != last {
                    last = readout;
                    // re-render: the card reads the atomics at draw time
                    entity.update(cx, |_, cx| cx.notify());
                }
            }
            if progress.finished_ok() {
                println!("[shotori] model download complete");
                // hand the frozen snapshot over to OCR
                let snap = entity.update(cx, |this, _| {
                    if this
                        .ocr_setup
                        .as_ref()
                        .is_some_and(|s| s.owns_download(&progress))
                    {
                        this.ocr_setup
                            .take()
                            .map(|s| (s.snapshot, this.capture.clone()))
                    } else {
                        None
                    }
                });
                if let Some((snapshot, capture)) = snap {
                    // Not inside this window's action handler, so the
                    // handle update works here (the no-op trap only hits
                    // action handlers)
                    let _ = window_handle.update(cx, |_, window, cx| {
                        exit_into_background_ocr(snapshot, &capture, window, cx);
                    });
                }
            } else {
                let err = progress.error();
                let _ = window_handle.update(cx, |_, window, cx| {
                    entity.update(cx, |this, cx| {
                        if let Some(setup) = this.ocr_setup.as_mut()
                            && setup.owns_download(&progress)
                        {
                            setup.stage = crate::ui::ocr_setup::Stage::Failed(err);
                            this.setup_focus.focus_confirm(window, cx);
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }

    /// Setup dialog [Cancel]/[Close] and Esc: abort the download, clean up,
    /// back to plain selection mode.
    pub(super) fn ocr_setup_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = self.ocr_setup.take() else {
            return;
        };
        setup
            .progress
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.session.update(cx, |s, cx| {
            s.set_blocked(false);
            cx.notify();
        });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
}

/// Enter/Ctrl+C exit path: PNG-encode (fast tier) + clipboard handoff run
/// on the background executor so the frozen overlay never blocks on
/// compression (the balanced-tier 4K encode cost ~1.4 s; the fast tier is
/// ~45 ms). The daemon spawn (Linux) is blocking I/O
/// and rides the same background task.
async fn copy_to_clipboard(
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    name: String,
    entity: gpui_kit::Entity<Overlay>,
    cx: &mut gpui_kit::AsyncApp,
) {
    let result = cx
        .background_executor()
        .spawn(async move {
            let png = crate::model::export::encode_png_fast(w, h, &rgba)?;
            crate::clipboard::copy_image(w, h, &rgba, &png)?;
            Ok::<Vec<u8>, anyhow::Error>(png)
        })
        .await;
    match result {
        Ok(png) => {
            println!("[shotori] copied {w}x{h} (from {name}) to clipboard");
            // The thumbnail renders inside the detached notify child —
            // the parent does no pixel work (see notify::copied)
            crate::notify::copied(&png);
            cx.update(|cx| cx.quit());
        }
        Err(e) => {
            // Stay in the overlay on failure: the user can still Ctrl+S
            eprintln!("[shotori] copy failed: {e:#}");
            entity.update(cx, |_, cx| cx.notify());
            crate::notify::send(
                crate::i18n::t!("notify.copy.failed.title").as_ref(),
                crate::i18n::t!("notify.copy.failed.body").as_ref(),
            );
        }
    }
}

/// The OCR exit: the desktop comes back the moment the overlays unmap and
/// the inference runs on the background executor while a tiny
/// non-interactive OSD chip (top-center of the triggering output, see
/// [`crate::ui::ocr_osd`]) is the only in-flight feedback — no frozen
/// screen, no stolen keyboard. On completion the text is delivered
/// (clipboard + notification) and the process quits. `window` is the
/// triggering overlay's window: the current window must remove itself
/// through its own reference (see save_dialog's teardown notes). Shared by
/// the Ctrl+O fast path and the post-download handover.
fn exit_into_background_ocr(
    snapshot: crate::ui::ocr_setup::Snapshot,
    capture: &Capture,
    window: &mut Window,
    cx: &mut App,
) {
    let crate::ui::ocr_setup::Snapshot { w, h, rgba } = snapshot;
    // Explicit quit BEFORE anything closes: between the overlays unmapping
    // and the completion callback the app is down to zero windows, and the
    // default auto-quit would kill the in-flight inference with them
    cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
    // Pin the chip to the triggering output (position-matched — see
    // display_for_capture for why Window::display is not usable here)
    let osd = crate::platform::display::display_for_capture(capture, cx)
        .and_then(|did| crate::ui::ocr_osd::open(Some(did), cx));
    crate::save_dialog::close_overlays(window, cx);

    let work = cx.background_executor().spawn(async move {
        let result = crate::ocr::run_ocr(&rgba, w, h);
        crate::ocr::deliver_text(w, h, result);
    });
    cx.spawn(async move |cx| {
        work.await;
        cx.update(|cx| {
            if let Some(osd) = osd {
                let _ = osd.update(cx, |_, window, _| window.remove_window());
            }
            // The usual flush lag (quit_after_close's measured details):
            // the compositor learns of the OSD's destroy only when the
            // loop pumps, so quit waits a beat
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    })
    .detach();
}
