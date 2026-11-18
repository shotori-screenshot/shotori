//! Visual Pin History manager (`shotori pin --history`).
//!
//! A regular XDG toplevel window (like `SettingsWindow`, wrapped in
//! `gpui_kit::component::Root` with a client `TitleBar` and synced palette)
//! displaying persisted pin thumbnails with one-click Re-pin, Copy, Save,
//! Delete, Clear All, Unlock Click-Through, and Open Folder actions.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::{ActiveTheme, Icon, Root, TITLE_BAR_HEIGHT, TitleBar};
use gpui_kit::*;

use crate::actions::{ClosePinHistory, RefreshPinHistory};
use crate::model::pin_history::{PinHistoryEntry, PinHistoryStore, ensure_history_dir};
use crate::ui::image_util;

const HIST_WIN_W: f32 = 940.;
const HIST_WIN_H: f32 = 640.;
const THUMB_MAX_SIDE: u32 = 320;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

struct LoadedCard {
    entry: PinHistoryEntry,
    preview: Arc<RenderImage>,
}

pub(crate) struct PinHistoryWindow {
    store: PinHistoryStore,
    cards: Vec<LoadedCard>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _activation: Subscription,
}

/// Run the standalone Pin History window; exits when the window closes.
pub fn run() -> i32 {
    let store = match PinHistoryStore::default_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[shotori] pin history unavailable: {e:#}");
            return 1;
        }
    };
    application()
        .with_assets(crate::ui::toolbar::ToolbarSource)
        .run(move |cx| {
            cx.set_app_identity(crate::APP_ID, "Shotori");
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::actions::bind_keys(cx, crate::model::settings::current().keys());
            crate::ui::settings::sync_shell_theme(cx);

            let store = store.clone();
            cx.spawn(async move |cx| {
                let opened = cx.update(|cx| {
                    cx.open_window(window_options(), |window, cx| {
                        let view = cx.new(|cx| PinHistoryWindow::new(store, window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                });
                if let Err(err) = opened {
                    eprintln!("[shotori] failed to open pin history window: {err:#}");
                }
            })
            .detach();
        });
    0
}

fn window_options() -> WindowOptions {
    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: Some(TitlebarOptions {
            title: Some(crate::i18n::t!("pin.history.title").to_string().into()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(HIST_WIN_W), px(HIST_WIN_H)),
        })),
        is_resizable: true,
        focus: true,
        ..TitleBar::window_options()
    }
}

fn load_cards(store: &PinHistoryStore) -> Vec<LoadedCard> {
    store
        .entries()
        .into_iter()
        .filter_map(|entry| {
            let path = store.png_path(&entry);
            let bytes = std::fs::read(&path).ok()?;
            let dyn_img =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
            let thumb = if dyn_img.width() > THUMB_MAX_SIDE || dyn_img.height() > THUMB_MAX_SIDE {
                dyn_img.thumbnail(THUMB_MAX_SIDE, THUMB_MAX_SIDE)
            } else {
                dyn_img
            };
            let rgba = thumb.into_rgba8();
            let (tw, th) = rgba.dimensions();
            let preview = image_util::rgba_to_render_image(rgba.into_raw(), tw, th);
            Some(LoadedCard { entry, preview })
        })
        .collect()
}

impl PinHistoryWindow {
    pub(crate) fn new(store: PinHistoryStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::ui::settings::sync_shell_theme(cx);
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let cards = load_cards(&store);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload(cx);
            }
        });
        Self {
            store,
            cards,
            focus_handle,
            scroll_handle: ScrollHandle::new(),
            _activation: activation,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.cards = load_cards(&self.store);
        cx.notify();
    }

    fn repin_entry(&self, id: &str) {
        crate::launch_detached_args(&["pin", "--restore-id", id]);
    }

    fn copy_entry(&self, id: String, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let (entry, rgba) = store.load_entry(&id)?;
                    let png = std::fs::read(store.png_path(&entry))?;
                    crate::clipboard::copy_image(entry.width, entry.height, &rgba, &png)?;
                    Ok::<Vec<u8>, anyhow::Error>(png)
                })
                .await;
            match res {
                Ok(png) => crate::notify::copied(&png),
                Err(e) => {
                    eprintln!("[shotori] history copy failed: {e:#}");
                    crate::notify::send(
                        crate::i18n::t!("pin.notify.copy_failed.title").as_ref(),
                        crate::i18n::t!("pin.notify.copy_failed.body").as_ref(),
                    );
                }
            }
        })
        .detach();
    }

    fn save_entry(&self, id: String) {
        let store = self.store.clone();
        std::thread::Builder::new()
            .name("shotori-hist-save".into())
            .spawn(move || {
                let Ok((entry, rgba)) = store.load_entry(&id) else {
                    return;
                };
                if let Ok(fixed) = std::env::var("SHOTORI_DEBUG_SAVE_PATH") {
                    drop(crate::model::export::save_image(
                        std::path::Path::new(&fixed),
                        entry.width,
                        entry.height,
                        &rgba,
                    ));
                    return;
                }
                let export_cfg = crate::model::settings::current().export();
                if export_cfg.auto_save {
                    crate::save_dialog::save_direct_and_notify(entry.width, entry.height, &rgba);
                    return;
                }
                let dir = export_cfg
                    .resolved_save_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                drop(std::fs::create_dir_all(&dir));
                let dialog = rfd::FileDialog::new()
                    .set_title(crate::i18n::t!("save.dialog_title").as_ref())
                    .set_directory(dir)
                    .set_file_name(crate::save_dialog::suggested_name_for(
                        entry.width,
                        entry.height,
                    ))
                    .set_can_create_directories(true);
                if let Some(mut path) = dialog.save_file() {
                    if path.extension().is_none() {
                        path.set_extension(export_cfg.format.extension());
                    }
                    if crate::model::export::save_image(&path, entry.width, entry.height, &rgba)
                        .is_ok()
                    {
                        crate::notify::saved(&path);
                    }
                }
            })
            .ok();
    }

    fn delete_entry(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.store.remove(id) {
            eprintln!("[shotori] remove history entry failed: {e:#}");
        }
        self.cards.retain(|c| c.entry.id != id);
        cx.notify();
    }

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.store.clear() {
            eprintln!("[shotori] clear history failed: {e:#}");
        }
        self.cards.clear();
        cx.notify();
    }

    fn header(&self, window: &Window, cx: &App) -> TitleBar {
        let supported = window.window_controls();
        let theme = cx.theme();
        let win_ctrl = |id: &'static str,
                        icon: IconName,
                        danger: bool,
                        on_click: fn(&ClickEvent, &mut Window, &mut App)| {
            let hover_bg = if danger {
                theme.danger
            } else {
                theme.secondary_hover
            };
            let hover_fg = if danger {
                theme.danger_foreground
            } else {
                theme.secondary_foreground
            };
            div()
                .id(id)
                .flex()
                .w(TITLE_BAR_HEIGHT)
                .h_full()
                .flex_shrink_0()
                .justify_center()
                .items_center()
                .text_color(theme.foreground)
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |ev, win, cx| {
                    cx.stop_propagation();
                    on_click(ev, win, cx);
                })
                .child(Icon::new(icon).size_4())
        };

        let mut right = div().flex().items_center().h_full().flex_shrink_0();
        if supported.minimize {
            right = right.child(win_ctrl(
                "pin-hist-min",
                IconName::WindowMinimize,
                false,
                |_, w, _| w.minimize_window(),
            ));
        }
        if supported.maximize {
            right = right.child(win_ctrl(
                "pin-hist-max",
                if window.is_maximized() {
                    IconName::WindowRestore
                } else {
                    IconName::WindowMaximize
                },
                false,
                |_, w, _| w.zoom_window(),
            ));
        }
        right = right.child(win_ctrl(
            "pin-hist-close",
            IconName::WindowClose,
            true,
            |_, w, _| w.remove_window(),
        ));

        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                .h_full()
                .pl_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child(Icon::new(IconName::Pin).size_4().text_color(theme.primary))
                        .child(crate::i18n::t!("pin.history.title").to_string()),
                )
                .child(right),
        )
    }
}

impl Render for PinHistoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.cards.len();
        let is_empty = self.cards.is_empty();
        let entity = cx.entity();

        let top_btn = |id: &'static str,
                       icon: IconName,
                       label: String,
                       disabled: bool,
                       on_click: ClickHandler| {
            Button::new(id)
                .debug_selector(move || id.into())
                .accessibility_label(label.clone())
                .disabled(disabled)
                .styles(|s| s.disabled(|d| d.opacity(0.45)))
                .px_2p5()
                .h_8()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().group_box)
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(cx.theme().foreground)
                .hover(|s| s.bg(cx.theme().secondary_hover))
                .on_click(on_click)
                .child(Icon::new(icon).size_3p5())
                .child(label)
        };

        let toolbar = div()
            .w_full()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div().flex().items_center().gap_2().child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t!("pin.history.subtitle", count = count).to_string()),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(top_btn(
                        "pin-hist-clipboard",
                        IconName::ClipboardPaste,
                        crate::i18n::t!("pin.history.pin_clipboard").to_string(),
                        false,
                        Box::new(|_, _, _| {
                            crate::launch_detached_args(&["pin", "--clipboard"]);
                        }),
                    ))
                    .child(top_btn(
                        "pin-hist-unlock",
                        IconName::LockOpen,
                        crate::i18n::t!("pin.menu.unlock_all").to_string(),
                        false,
                        Box::new(|_, _, _| {
                            drop(crate::ui::pin::send_control(
                                crate::ui::pin::ControlCommand::UnlockAll,
                            ));
                        }),
                    ))
                    .child(top_btn(
                        "pin-hist-folder",
                        IconName::FolderOpen,
                        crate::i18n::t!("pin.history.open_folder").to_string(),
                        false,
                        Box::new(|_, _, _| {
                            if let Ok(dir) = ensure_history_dir() {
                                drop(std::process::Command::new("xdg-open").arg(dir).spawn());
                            }
                        }),
                    ))
                    .child(top_btn(
                        "pin-hist-refresh",
                        IconName::RefreshCw,
                        crate::i18n::t!("pin.history.refresh").to_string(),
                        false,
                        {
                            let ent = entity.clone();
                            Box::new(move |_, _, cx| {
                                ent.update(cx, |this, cx| this.reload(cx));
                            })
                        },
                    ))
                    .child(top_btn(
                        "pin-hist-clear",
                        IconName::Trash,
                        crate::i18n::t!("pin.history.clear_all").to_string(),
                        is_empty,
                        {
                            let ent = entity.clone();
                            Box::new(move |_, _, cx| {
                                ent.update(cx, |this, cx| this.clear_all(cx));
                            })
                        },
                    )),
            );

        let content: AnyElement = if is_empty {
            div()
                .id("pin-hist-empty")
                .debug_selector(|| "pin-hist-empty".into())
                .flex_1()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(
                    Icon::new(IconName::Images)
                        .size_8()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(crate::i18n::t!("pin.history.empty_title").to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t!("pin.history.empty_body").to_string()),
                )
                .into_any_element()
        } else {
            let cards_els: Vec<AnyElement> = self
                .cards
                .iter()
                .enumerate()
                .map(|(ix, card)| {
                    let entry_id = card.entry.id.clone();
                    let dims = format!("{} × {}", card.entry.width, card.entry.height);
                    let time_str = card.entry.formatted_timestamp();
                    let snippet = card.entry.label.clone();
                    let opacity = card.entry.opacity;

                    let card_icon_btn =
                        |btn_id: SharedString,
                         sel: String,
                         icon: IconName,
                         primary: bool,
                         on_click: ClickHandler| {
                            Button::new(btn_id)
                                .debug_selector(move || sel.clone())
                                .h_7()
                                .px_2()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(if primary {
                                    cx.theme().primary
                                } else {
                                    cx.theme().border
                                })
                                .bg(if primary {
                                    cx.theme().primary
                                } else {
                                    cx.theme().background
                                })
                                .text_color(if primary {
                                    cx.theme().primary_foreground
                                } else {
                                    cx.theme().foreground
                                })
                                .hover(|s| {
                                    if primary {
                                        s.opacity(0.9)
                                    } else {
                                        s.bg(cx.theme().secondary_hover)
                                    }
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .text_xs()
                                .on_click(on_click)
                                .child(Icon::new(icon).size_3p5())
                        };

                    let id_repin = entry_id.clone();
                    let ent_repin = entity.clone();
                    let repin_btn = card_icon_btn(
                        format!("pin-card-repin-{ix}").into(),
                        format!("pin-card-repin-{ix}"),
                        IconName::Pin,
                        true,
                        Box::new(move |_, _, cx| {
                            ent_repin.read(cx).repin_entry(&id_repin);
                        }),
                    )
                    .child(crate::i18n::t!("common.pin").to_string());

                    let id_copy = entry_id.clone();
                    let ent_copy = entity.clone();
                    let copy_btn = card_icon_btn(
                        format!("pin-card-copy-{ix}").into(),
                        format!("pin-card-copy-{ix}"),
                        IconName::Copy,
                        false,
                        Box::new(move |_, _, cx| {
                            ent_copy.update(cx, |this, cx| this.copy_entry(id_copy.clone(), cx));
                        }),
                    );

                    let id_save = entry_id.clone();
                    let ent_save = entity.clone();
                    let save_btn = card_icon_btn(
                        format!("pin-card-save-{ix}").into(),
                        format!("pin-card-save-{ix}"),
                        IconName::Save,
                        false,
                        Box::new(move |_, _, cx| {
                            ent_save.read(cx).save_entry(id_save.clone());
                        }),
                    );

                    let id_del = entry_id.clone();
                    let ent_del = entity.clone();
                    let del_btn = card_icon_btn(
                        format!("pin-card-del-{ix}").into(),
                        format!("pin-card-del-{ix}"),
                        IconName::Trash,
                        false,
                        Box::new(move |_, _, cx| {
                            ent_del.update(cx, |this, cx| this.delete_entry(&id_del, cx));
                        }),
                    );

                    div()
                        .id(("pin-hist-card", ix))
                        .debug_selector(move || format!("pin-hist-card-{ix}"))
                        .w(px(272.))
                        .p_2p5()
                        .rounded(cx.theme().radius_lg)
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().group_box)
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .w_full()
                                .h(px(148.))
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border.opacity(0.6))
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    img(card.preview.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_xs()
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(cx.theme().foreground)
                                        .child(if opacity < 100 {
                                            format!("{dims} · {opacity}%")
                                        } else {
                                            dims
                                        }),
                                )
                                .child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(time_str),
                                ),
                        )
                        .children(snippet.map(|s| {
                            div()
                                .w_full()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(s)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_1p5()
                                .child(repin_btn)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(copy_btn)
                                        .child(save_btn)
                                        .child(del_btn),
                                ),
                        )
                        .into_any_element()
                })
                .collect();

            div()
                .relative()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .child(
                    div()
                        .id("pin-hist-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll_handle)
                        .p_5()
                        .child(div().flex().flex_wrap().gap_4().children(cards_els)),
                )
                .child(Scrollbar::vertical(&self.scroll_handle).mode(ScrollbarMode::Scrolling))
                .into_any_element()
        };

        div()
            .id("pin-history-window")
            .key_context("ShotoriPinHistory")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(|_: &ClosePinHistory, window, _| window.remove_window())
            .on_action(cx.listener(|this, _: &RefreshPinHistory, _, cx| this.reload(cx)))
            .child(self.header(window, cx))
            .child(toolbar)
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::PinHistoryWindow;
    use crate::model::pin_history::{NewPinRecord, PinHistoryStore};
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext, Bounds, TestAppContext, point, px, size};

    #[gpui_kit::test]
    fn history_window_renders_cards_deletes_single_and_clears_all(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinHistoryStore::at(tmp.path().join("hist"));
        store
            .record(
                NewPinRecord {
                    width: 8,
                    height: 6,
                    rgba: &[200u8; 8 * 6 * 4],
                    rect: Bounds::new(point(px(10.), px(20.)), size(px(80.), px(60.))),
                    opacity: 95,
                    label: Some("Snippet A".into()),
                },
                20,
            )
            .unwrap();
        store
            .record(
                NewPinRecord {
                    width: 10,
                    height: 10,
                    rgba: &[120u8; 10 * 10 * 4],
                    rect: Bounds::new(point(px(40.), px(50.)), size(px(100.), px(100.))),
                    opacity: 100,
                    label: None,
                },
                20,
            )
            .unwrap();

        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::actions::bind_keys(cx, &crate::actions::default_keys());
        });

        let store_for_view = store.clone();
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| PinHistoryWindow::new(store_for_view, window, cx));
            Root::new(view, window, cx)
        });
        vcx.simulate_resize(size(px(940.), px(640.)));
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(vcx.debug_bounds("pin-hist-card-0").is_some());
        assert!(vcx.debug_bounds("pin-hist-card-1").is_some());

        // Delete card 0 -> 1 card remains on screen and on disk.
        let del0 = vcx.debug_bounds("pin-card-del-0").unwrap();
        vcx.simulate_click(del0.center(), Default::default());
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(store.entries().len(), 1);
        assert!(vcx.debug_bounds("pin-hist-card-0").is_some());
        assert!(vcx.debug_bounds("pin-hist-card-1").is_none());

        // Clear all -> transitions to empty state and removes disk files.
        let clear = vcx.debug_bounds("pin-hist-clear").unwrap();
        vcx.simulate_click(clear.center(), Default::default());
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(store.entries().is_empty());
        assert!(vcx.debug_bounds("pin-hist-empty").is_some());
    }
}
