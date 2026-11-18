//! Per-pin and board-wide operations: Copy, Save (direct & portal-safe async),
//! OCR, QR scan, Clipboard-to-Pin paste, closed-pin restoration, image
//! transforms, opacity stepping, ghost click-through, thumbnail collapse,
//! keyboard nudge, and pin cycling.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

use crate::model::pin::{
    clamp_origin, collapse_bounds, expand_bounds, flip_rgba_horizontal, flip_rgba_vertical,
    rotate_rgba_ccw, rotate_rgba_cw,
};
use crate::ui::theme;

use super::{ClosedPin, PinBoard, PinSpec, PinSurface};

const MAX_CLOSED_STACK: usize = 24;

impl PinBoard {
    /// Display a transient HUD badge on `id`, clearing automatically after 1.2s
    /// unless superseded by a newer badge on the same pin.
    pub(super) fn show_hud(&mut self, id: u64, text: String, cx: &mut Context<Self>) {
        self.hud_seq = self.hud_seq.wrapping_add(1);
        let seq = self.hud_seq;
        let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) else {
            return;
        };
        pin.hud = Some((seq, text));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |board, cx| {
                if let Some(pin) = board.pins.iter_mut().find(|p| p.id == id)
                    && pin.hud.as_ref().is_some_and(|(s, _)| *s == seq)
                {
                    pin.hud = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn sync_pin_to_history(&self, id: u64) {
        let Some(store) = &self.history_store else {
            return;
        };
        let Some(pin) = self.pins.iter().find(|p| p.id == id) else {
            return;
        };
        let Some(hist_id) = pin.history_id.as_deref() else {
            return;
        };
        let effective_rect = match pin.collapsed {
            Some(collapsed) => expand_bounds(pin.rect, collapsed, &self.outputs),
            None => pin.rect,
        };
        let updated_raster = pin.dirty_raster.then(|| {
            let (w, h, ref rgba) = *pin.composited_crop;
            (w, h, rgba.as_slice())
        });
        if let Err(e) = store.update_entry(hist_id, effective_rect, pin.opacity, updated_raster) {
            eprintln!("[shotori] pin history update failed: {e:#}");
        }
    }

    /// Remove one pin, push onto `closed_stack` for `Ctrl+Shift+T`, and flush
    /// its final bounds/opacity/annotations to disk history.
    pub(super) fn remove_pin_by_id(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        }
        self.sync_pin_to_history(id);
        if let Some(ix) = self.pins.iter().position(|p| p.id == id) {
            let removed = self.pins.remove(ix);
            let restored_rect = match removed.collapsed {
                Some(collapsed) => expand_bounds(removed.rect, collapsed, &self.outputs),
                None => removed.rect,
            };
            let (cw, ch, ref crgba) = *removed.composited_crop;
            if self.closed_stack.len() >= MAX_CLOSED_STACK {
                self.closed_stack.remove(0);
            }
            self.closed_stack.push(ClosedPin {
                w: cw,
                h: ch,
                rgba: crgba.clone(),
                base: removed.base,
                rect: restored_rect,
                opacity: removed.opacity,
                history_id: removed.history_id,
                label: removed.label,
            });
        }
        if self.drag.as_ref().is_some_and(|d| d.id == id) {
            self.drag = None;
        }
        self.menu = None;
        cx.notify();
        self.pins.is_empty()
    }

    pub(super) fn restore_last_closed(&mut self, cx: &mut Context<Self>) -> bool {
        self.menu = None;
        if let Some(closed) = self.closed_stack.pop() {
            let zoom = f32::from(closed.rect.size.width) / f32::from(closed.base.width).max(1.);
            let mut spec = PinSpec::new(closed.w, closed.h, closed.rgba, closed.rect);
            spec.opacity = Some(closed.opacity);
            spec.history_id = closed.history_id;
            spec.label = closed.label;
            let new_id = self.add_internal(spec, Some(closed.base), false, cx);
            let _ = zoom;
            self.show_hud(new_id, crate::i18n::t!("pin.hud.restored").into_owned(), cx);
            return true;
        }

        // Fall back to persistent disk history for entries not already open.
        let Some(store) = self.history_store.clone() else {
            return false;
        };
        let active_ids: std::collections::BTreeSet<String> = self
            .pins
            .iter()
            .filter_map(|p| p.history_id.clone())
            .collect();
        let candidate = store
            .entries()
            .into_iter()
            .find(|e| !active_ids.contains(&e.id));
        let Some(entry) = candidate else {
            return false;
        };
        match store.load_entry(&entry.id) {
            Ok((loaded, rgba)) => {
                let mut spec =
                    PinSpec::new(loaded.width, loaded.height, rgba, loaded.logical_bounds());
                spec.opacity = Some(loaded.opacity);
                spec.history_id = Some(loaded.id);
                spec.label = loaded.label;
                let new_id = self.add_internal(spec, None, false, cx);
                self.show_hud(new_id, crate::i18n::t!("pin.hud.restored").into_owned(), cx);
                true
            }
            Err(e) => {
                eprintln!("[shotori] restore pin failed: {e:#}");
                false
            }
        }
    }

    pub(super) fn step_opacity(&mut self, id: u64, delta: i16, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            let next = (i16::from(pin.opacity) + delta).clamp(20, 100) as u8;
            pin.opacity = next;
            let badge = format!("{next}%");
            self.show_hud(id, badge, cx);
            self.sync_pin_to_history(id);
        }
        cx.notify();
    }

    pub(super) fn rotate_pin(&mut self, id: u64, cw: bool, cx: &mut Context<Self>) {
        self.menu = None;
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        }
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            if let Some(collapsed) = pin.collapsed.take() {
                pin.rect = expand_bounds(pin.rect, collapsed, &self.outputs);
            }
            // Flatten any active annotations into the base crop before permuting
            // dimensions so rotated text/strokes stay 100% crisp and aligned.
            pin.bake_annotations_into_base();
            let (w, h, ref rgba) = *pin.base_crop;
            let (nw, nh, rotated) = if cw {
                rotate_rgba_cw(w, h, rgba)
            } else {
                rotate_rgba_ccw(w, h, rgba)
            };
            pin.base_crop = Arc::new((nw, nh, rotated));
            let center = pin.rect.center();
            pin.base = size(pin.base.height, pin.base.width);
            let swapped_size = size(pin.rect.size.height, pin.rect.size.width);
            let origin = point(
                center.x - swapped_size.width / 2.,
                center.y - swapped_size.height / 2.,
            );
            pin.rect = Bounds::new(
                clamp_origin(origin, swapped_size, &self.outputs),
                swapped_size,
            );
            pin.dirty_raster = true;
            pin.rebuild_visuals();
            let msg = if cw { "90° ↻" } else { "90° ↺" }.to_owned();
            self.show_hud(id, msg, cx);
            self.sync_pin_to_history(id);
        }
        cx.notify();
    }

    pub(super) fn flip_pin(&mut self, id: u64, horizontal: bool, cx: &mut Context<Self>) {
        self.menu = None;
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        }
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.bake_annotations_into_base();
            let (w, h, ref rgba) = *pin.base_crop;
            let mut flipped = rgba.clone();
            if horizontal {
                flip_rgba_horizontal(w, h, &mut flipped);
            } else {
                flip_rgba_vertical(w, h, &mut flipped);
            }
            pin.base_crop = Arc::new((w, h, flipped));
            pin.dirty_raster = true;
            pin.rebuild_visuals();
            let msg = if horizontal {
                crate::i18n::t!("pin.hud.flipped_h").into_owned()
            } else {
                crate::i18n::t!("pin.hud.flipped_v").into_owned()
            };
            self.show_hud(id, msg, cx);
            self.sync_pin_to_history(id);
        }
        cx.notify();
    }

    pub(super) fn toggle_grayscale(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toggle_pin_effect(
            id,
            |pin| {
                pin.grayscale = !pin.grayscale;
                if pin.grayscale {
                    crate::i18n::t!("pin.hud.grayscale_on").into_owned()
                } else {
                    crate::i18n::t!("pin.hud.grayscale_off").into_owned()
                }
            },
            cx,
        );
    }

    pub(super) fn toggle_invert(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toggle_pin_effect(
            id,
            |pin| {
                pin.invert = !pin.invert;
                if pin.invert {
                    crate::i18n::t!("pin.hud.invert_on").into_owned()
                } else {
                    crate::i18n::t!("pin.hud.invert_off").into_owned()
                }
            },
            cx,
        );
    }

    fn toggle_pin_effect(
        &mut self,
        id: u64,
        flip: impl FnOnce(&mut super::Pin) -> String,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let is_annot = self.annotating_id() == Some(id);
        let msg = if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.dirty_raster = true;
            let msg = flip(pin);
            if !is_annot {
                pin.rebuild_visuals();
            }
            Some(msg)
        } else {
            None
        };
        if let Some(msg) = msg {
            if is_annot {
                self.flush_annot_visuals(cx);
            }
            self.show_hud(id, msg, cx);
            self.sync_pin_to_history(id);
        }
        cx.notify();
    }

    pub(super) fn toggle_collapse(&mut self, id: u64, cx: &mut Context<Self>) {
        self.menu = None;
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        }
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            if let Some(saved) = pin.collapsed.take() {
                pin.rect = expand_bounds(pin.rect, saved, &self.outputs);
            } else {
                let (thumb_rect, saved) = collapse_bounds(pin.rect, &self.outputs);
                pin.rect = thumb_rect;
                pin.collapsed = Some(saved);
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_ghost(&mut self, id: u64, cx: &mut Context<Self>) {
        self.menu = None;
        if self.annotating_id() == Some(id) {
            self.exit_annotate(cx);
        }
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            pin.ghost = !pin.ghost;
            let on = pin.ghost;
            let msg = if on {
                crate::i18n::t!("pin.hud.ghost_on").into_owned()
            } else {
                crate::i18n::t!("pin.hud.ghost_off").into_owned()
            };
            self.show_hud(id, msg, cx);
        }
        cx.notify();
    }

    pub(super) fn unlock_all_ghosts(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let mut last_unlocked = None;
        for pin in &mut self.pins {
            if pin.ghost {
                pin.ghost = false;
                last_unlocked = Some(pin.id);
            }
        }
        if let Some(id) = last_unlocked {
            self.raise(id);
            self.show_hud(id, crate::i18n::t!("pin.hud.ghost_off").into_owned(), cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_visibility(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.drag = None;
        self.hidden = !self.hidden;
        cx.notify();
    }

    pub(super) fn nudge_selected(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if let Some(pin) = self.pins.iter_mut().find(|p| p.id == id) {
            let next = point(
                pin.rect.origin.x + px(dx as f32),
                pin.rect.origin.y + px(dy as f32),
            );
            pin.rect.origin = clamp_origin(next, pin.rect.size, &self.outputs);
            self.sync_pin_to_history(id);
        }
        cx.notify();
    }

    pub(super) fn cycle_focus(&mut self, forward: bool, cx: &mut Context<Self>) {
        let visible_count = self.pins.iter().filter(|p| !p.ghost).count();
        if visible_count <= 1 {
            return;
        }
        self.menu = None;
        if forward {
            // Rotate the bottommost non-ghost pin to the top.
            if let Some(ix) = self.pins.iter().position(|p| !p.ghost) {
                let p = self.pins.remove(ix);
                let id = p.id;
                self.pins.push(p);
                self.show_hud(id, format!("#{}", self.pins.len()), cx);
            }
        } else if let Some(top_ix) = self.pins.iter().rposition(|p| !p.ghost) {
            let top = self.pins.remove(top_ix);
            let insert_at = self.pins.iter().position(|p| !p.ghost).unwrap_or(0);
            self.pins.insert(insert_at, top);
            if let Some(new_top) = self.pins.iter().rev().find(|p| !p.ghost) {
                let id = new_top.id;
                self.show_hud(id, format!("#{id}"), cx);
            }
        }
        cx.notify();
    }
}

impl PinSurface {
    pub(super) fn close_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.board.read(cx).selected_id() else {
            return;
        };
        let empty = self
            .board
            .update(cx, |board, cx| board.remove_pin_by_id(id, cx));
        if empty {
            self.teardown_all_windows(window, cx);
        }
    }

    pub(super) fn close_all_pins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.board.read(cx).pins.iter().map(|p| p.id).collect();
        self.board.update(cx, |board, cx| {
            for id in ids {
                board.remove_pin_by_id(id, cx);
            }
        });
        self.teardown_all_windows(window, cx);
    }

    pub(super) fn teardown_all_windows(&self, window: &mut Window, cx: &mut App) {
        let handles = self.board.read(cx).windows.clone();
        let current = window.window_handle();
        window.remove_window();
        for handle in handles {
            if handle != current {
                drop(handle.update(cx, |_, w, _| w.remove_window()));
            }
        }
    }

    /// Copy the selected pin's composited raster (including transforms & annotations).
    pub(super) fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let target = self.board.update(cx, |board, cx| {
            let id = board.selected_id()?;
            if board.annotating_id() == Some(id) {
                board.flush_annot_visuals(cx);
            }
            board.menu = None;
            cx.notify();
            board.crop_of(id).map(|crop| (id, crop))
        });
        let Some((id, crop)) = target else {
            return;
        };
        let board = self.board.clone();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let (w, h, ref rgba) = *crop;
                    let png = crate::model::export::encode_png_fast(w, h, rgba)?;
                    crate::clipboard::copy_image(w, h, rgba, &png)?;
                    Ok::<(u32, u32, Vec<u8>), anyhow::Error>((w, h, png))
                })
                .await;
            match result {
                Ok((w, h, png)) => {
                    println!("[shotori] copied pinned {w}x{h} to clipboard");
                    crate::notify::copied(&png);
                    board.update(cx, |b, cx| {
                        b.show_hud(id, crate::i18n::t!("pin.hud.copied").into_owned(), cx);
                    });
                }
                Err(e) => {
                    eprintln!("[shotori] pin copy failed: {e:#}");
                    crate::notify::send(
                        crate::i18n::t!("pin.notify.copy_failed.title").as_ref(),
                        crate::i18n::t!("pin.notify.copy_failed.body").as_ref(),
                    );
                }
            }
        })
        .detach();
    }

    /// Save the selected pin to disk (direct save when `export.auto_save` or
    /// `SHOTORI_DEBUG_SAVE_PATH` is set; otherwise temporarily hides layer-shell
    /// pins while running the `rfd` portal Save dialog on a background OS thread).
    pub(super) fn save_selected(&mut self, cx: &mut Context<Self>) {
        let target = self.board.update(cx, |board, cx| {
            let id = board.selected_id()?;
            if board.annotating_id() == Some(id) {
                board.flush_annot_visuals(cx);
            }
            board.menu = None;
            cx.notify();
            board.crop_of(id).map(|crop| (id, crop))
        });
        let Some((id, crop)) = target else {
            return;
        };
        let debug_path = std::env::var("SHOTORI_DEBUG_SAVE_PATH").ok();
        let auto_save = crate::model::settings::current().export().auto_save;
        let board = self.board.clone();

        if debug_path.is_some() || auto_save {
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .spawn(async move {
                        let (w, h, ref rgba) = *crop;
                        if let Some(fixed) = debug_path {
                            if let Err(e) = crate::model::export::save_image(
                                std::path::Path::new(&fixed),
                                w,
                                h,
                                rgba,
                            ) {
                                eprintln!("[shotori] pin save failed: {e:#}");
                            }
                        } else {
                            crate::save_dialog::save_direct_and_notify(w, h, rgba);
                        }
                    })
                    .await;
                board.update(cx, |b, cx| {
                    b.show_hud(id, crate::i18n::t!("pin.hud.saved").into_owned(), cx);
                });
            })
            .detach();
            return;
        }

        // Portal dialog path: hide layer-shell pin surfaces while the native
        // portal dialog is open so a large Top-layer pin never occludes it.
        self.board.update(cx, |b, cx| {
            b.save_dialog_open = true;
            cx.notify();
        });
        let (tx, rx) = async_channel::bounded::<bool>(1);
        std::thread::Builder::new()
            .name("shotori-pin-save".into())
            .spawn(move || {
                let (w, h, ref rgba) = *crop;
                let export_cfg = crate::model::settings::current().export();
                let dir = if export_cfg.remember_last_dir
                    && let Some(last) = crate::model::settings::read_last_save_dir()
                {
                    last
                } else {
                    export_cfg
                        .resolved_save_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                };
                drop(std::fs::create_dir_all(&dir));
                let mut dialog = rfd::FileDialog::new()
                    .set_title(crate::i18n::t!("save.dialog_title").as_ref())
                    .set_directory(dir)
                    .set_file_name(crate::save_dialog::suggested_name_for(w, h))
                    .set_can_create_directories(true);
                for fmt in crate::model::settings::ExportFormat::ALL {
                    let label = match fmt {
                        crate::model::settings::ExportFormat::Png => {
                            crate::i18n::t!("save.png_filter")
                        }
                        crate::model::settings::ExportFormat::Jpeg => {
                            crate::i18n::t!("save.jpeg_filter")
                        }
                        crate::model::settings::ExportFormat::Webp => {
                            crate::i18n::t!("save.webp_filter")
                        }
                    };
                    dialog = dialog.add_filter(label.as_ref(), fmt.filter_extensions());
                }
                let mut saved = false;
                if let Some(mut path) = dialog.save_file() {
                    if path.extension().is_none() {
                        path.set_extension(export_cfg.format.extension());
                    }
                    if export_cfg.remember_last_dir
                        && let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        crate::model::settings::write_last_save_dir(parent);
                    }
                    match crate::model::export::save_image(&path, w, h, rgba) {
                        Ok(()) => {
                            crate::notify::saved(&path);
                            saved = true;
                        }
                        Err(e) => {
                            eprintln!("[shotori] pin save failed: {e:#}");
                            crate::notify::send(
                                crate::i18n::t!("save.failed.title").as_ref(),
                                crate::i18n::t!("save.failed.body").as_ref(),
                            );
                        }
                    }
                }
                tx.send_blocking(saved).ok();
            })
            .ok();

        cx.spawn(async move |_, cx| {
            let saved = rx.recv().await.unwrap_or(false);
            board.update(cx, |b, cx| {
                b.save_dialog_open = false;
                if saved {
                    b.show_hud(id, crate::i18n::t!("pin.hud.saved").into_owned(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run on-device OCR directly against the selected pin's composited pixels.
    pub(super) fn ocr_selected(&mut self, cx: &mut Context<Self>) {
        let target = self.board.update(cx, |board, cx| {
            let id = board.selected_id()?;
            if board.annotating_id() == Some(id) {
                board.flush_annot_visuals(cx);
            }
            board.menu = None;
            cx.notify();
            board.crop_of(id).map(|crop| (id, crop))
        });
        let Some((id, crop)) = target else {
            return;
        };
        if crate::ocr::models_missing() {
            crate::notify::send(
                crate::i18n::t!("notify.ocr.failed.title").as_ref(),
                crate::i18n::t!("pin.notify.ocr_missing_models").as_ref(),
            );
            crate::launch_detached_args(&["settings", "ocr"]);
            return;
        }
        self.board.update(cx, |b, cx| {
            b.show_hud(id, crate::i18n::t!("osd.ocr_running").into_owned(), cx);
        });
        let board = self.board.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .spawn(async move {
                    let (w, h, ref rgba) = *crop;
                    let res = crate::ocr::run_ocr(rgba, w, h);
                    crate::ocr::deliver_text(w, h, res);
                })
                .await;
            board.update(cx, |b, cx| {
                b.show_hud(id, crate::i18n::t!("pin.hud.ocr_done").into_owned(), cx);
            });
        })
        .detach();
    }

    /// Scan QR codes directly on the selected pin's composited pixels.
    pub(super) fn qr_selected(&mut self, cx: &mut Context<Self>) {
        let target = self.board.update(cx, |board, cx| {
            let id = board.selected_id()?;
            if board.annotating_id() == Some(id) {
                board.flush_annot_visuals(cx);
            }
            board.menu = None;
            cx.notify();
            board.crop_of(id).map(|crop| (id, crop))
        });
        let Some((id, crop)) = target else {
            return;
        };
        let board = self.board.clone();
        cx.spawn(async move |_, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let (w, h, ref rgba) = *crop;
                    let payloads = crate::qr::scan_qr(rgba, w, h)?;
                    if payloads.is_empty() {
                        crate::notify::send(
                            crate::i18n::t!("notify.qr.empty.title").as_ref(),
                            crate::i18n::t!("notify.qr.empty.body").as_ref(),
                        );
                        return Ok::<bool, anyhow::Error>(false);
                    }
                    let text = crate::qr::join_payloads(&payloads);
                    crate::clipboard::copy_text(text.clone())?;
                    let preview = crate::notify::preview_snippet(&text);
                    crate::notify::qr_copied(&text, &preview);
                    Ok(true)
                })
                .await;
            if let Ok(has_qr) = found {
                let badge = if has_qr {
                    crate::i18n::t!("pin.hud.qr_copied").into_owned()
                } else {
                    crate::i18n::t!("notify.qr.empty.title").into_owned()
                };
                board.update(cx, |b, cx| b.show_hud(id, badge, cx));
            }
        })
        .detach();
    }

    /// Read an image or UTF-8 text from the Wayland clipboard and float it as a
    /// new pin centered on this surface's output (`Ctrl+V` / menu).
    pub(super) fn paste_clipboard_pin(&mut self, cx: &mut Context<Self>) {
        self.board.update(cx, |board, cx| {
            board.menu = None;
            cx.notify();
        });
        let out_bounds = self.output.bounds;
        let out_scale = self.output.scale.max(1.0);
        let c = theme::c();
        let board = self.board.clone();

        cx.spawn(async move |_, cx| {
            let payload = cx
                .background_executor()
                .spawn(async move {
                    let clip = crate::clipboard::read_clipboard()?;
                    match clip {
                        crate::clipboard::ClipboardData::Image { w, h, rgba } => {
                            Ok::<(u32, u32, Vec<u8>, Option<String>), anyhow::Error>((
                                w, h, rgba, None,
                            ))
                        }
                        crate::clipboard::ClipboardData::Text(text) => {
                            let label = Some(text.lines().next().unwrap_or("").to_owned());
                            let (w, h, rgba) = crate::annotation::text::render_text_card(
                                &text,
                                out_scale,
                                c.toolbar_bg,
                                c.toolbar_text,
                            )?;
                            Ok((w, h, rgba, label))
                        }
                    }
                })
                .await;
            match payload {
                Ok((w, h, rgba, label)) => {
                    let out_w = f32::from(out_bounds.size.width).max(1.);
                    let out_h = f32::from(out_bounds.size.height).max(1.);
                    let base_w = (w as f32 / out_scale).max(1.);
                    let base_h = (h as f32 / out_scale).max(1.);
                    let fit = ((out_w * 0.85) / base_w)
                        .min((out_h * 0.85) / base_h)
                        .min(1.0);
                    let pw = (base_w * fit).round().max(1.);
                    let ph = (base_h * fit).round().max(1.);
                    let origin = point(
                        out_bounds.origin.x + px(((out_w - pw) * 0.5).round()),
                        out_bounds.origin.y + px(((out_h - ph) * 0.5).round()),
                    );
                    let mut spec =
                        PinSpec::new(w, h, rgba, Bounds::new(origin, size(px(pw), px(ph))));
                    spec.label = label;
                    board.update(cx, |b, cx| {
                        b.add(spec, cx);
                    });
                }
                Err(e) => {
                    eprintln!("[shotori] paste pin failed: {e:#}");
                    crate::notify::send(
                        crate::i18n::t!("pin.notify.paste_failed.title").as_ref(),
                        crate::i18n::t!("pin.notify.paste_failed.body").as_ref(),
                    );
                }
            }
        })
        .detach();
    }
}
