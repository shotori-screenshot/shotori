//! # Settings window (`shotori settings`)
//!
//! A regular toplevel — the one shotori surface that is NOT a
//! layer-shell overlay: plain `WindowOptions` (system titlebar, the
//! compositor places it; the layer-shell rules — display_id pinning,
//! bare `open_window` — do not apply, and `base::Root` stays banned
//! regardless).
//!
//! Launch: `shotori settings` or the tray menu. The screenshot overlay
//! is a run-and-exit process, so edits cannot reach a live session:
//! every toggle writes straight through to `settings.toml`
//! (see [`crate::model::settings`]) and the NEXT capture picks it up —
//! crash-safe by construction, no OK button, no dirty state.
//!
//! v1 section: the selection toolbar's buttons (per-button visibility).
//! The section list below is data; theme picking and language join as
//! further sections when they become user-configurable.

use std::path::PathBuf;

use gpui_kit::base::{Button, Switch, SwitchThumb, SwitchTrack};
use gpui_kit::*;

use crate::model::settings::{Settings, ToolbarButton, ToolbarCluster};
use crate::ui::theme;
use crate::ui::toolbar::{BUTTONS, ButtonMeta};

gpui_kit::actions!([CloseSettings]);

/// The window's sections, in display order. A section lists the row-one
/// clusters it edits; the confirm pair (Cancel/Copy) shares "Actions"
/// here — the cluster split only matters to the toolbar's separator.
const SECTIONS: [(&str, &[ToolbarCluster]); 2] = [
    ("Annotation tools", &[ToolbarCluster::Tools]),
    (
        "Actions",
        &[ToolbarCluster::Actions, ToolbarCluster::Confirm],
    ),
];

const WINDOW_W: f32 = 620.;
const WINDOW_H: f32 = 660.;
const PAGE_PAD: f32 = 24.;
const ROW_H: f32 = 38.;
const ROW_GAP: f32 = 6.;
/// Two rows side by side; flex-wrap reflows if the window resizes.
const ROW_W: f32 = (WINDOW_W - 2. * PAGE_PAD - ROW_GAP) / 2.;
const SWITCH_W: f32 = 36.;
const SWITCH_H: f32 = 20.;
const SWITCH_THUMB: f32 = 16.;

/// Run the settings window; returns when the window closes.
pub fn run() {
    application()
        .with_assets(crate::ui::toolbar::ToolbarSource)
        .run(|cx| {
            cx.set_app_identity(crate::APP_ID, "Shotori");
            gpui_kit::base::init(cx);
            cx.bind_keys([KeyBinding::new(
                "escape",
                CloseSettings,
                Some("ShotoriSettings"),
            )]);
            // Open the window from a spawned task: displays() is empty
            // during synchronous startup (zed#46378) — the same trap
            // the overlay path guards against. The compositor places
            // this toplevel; the first event-loop pass is still the
            // safe place to create it.
            cx.spawn(async move |cx| {
                cx.update(|cx| {
                    cx.open_window(window_options(), |window, cx| {
                        cx.new(|cx| SettingsWindow::new(window, cx))
                    })
                    .expect("failed to open the settings window");
                });
            })
            .detach();
        });
}

fn window_options() -> WindowOptions {
    WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: Some(TitlebarOptions {
            title: Some("Shotori Settings".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(WINDOW_W), px(WINDOW_H)),
        })),
        focus: true,
        ..Default::default()
    }
}

struct SettingsWindow {
    /// Working copy of the user's settings; every change writes
    /// through to `path` immediately.
    settings: Settings,
    /// Where settings.toml lives (None when no config dir resolves).
    path: Option<PathBuf>,
    /// The last save's failure (or load warning), shown inline until
    /// the next successful save.
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // follow the system palette like the overlay does
        theme::follow_system(window.appearance());
        let (settings, warnings) = Settings::load(crate::model::settings::config_path().as_deref());
        for warning in &warnings {
            eprintln!("[shotori] settings: {warning}");
        }
        let subscriptions = vec![window.observe_window_appearance(|window, cx| {
            theme::follow_system(window.appearance());
            cx.refresh_windows();
        })];
        Self {
            settings,
            path: crate::model::settings::config_path(),
            error: warnings.first().cloned(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn set_button(&mut self, button: ToolbarButton, visible: bool, cx: &mut Context<Self>) {
        self.settings.set_toolbar_visible(button, visible);
        self.persist(cx);
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.settings = Settings::default();
        self.persist(cx);
    }

    /// Write-through save: the file is the single source of truth.
    fn persist(&mut self, cx: &mut Context<Self>) {
        self.error = match &self.path {
            Some(path) => self.settings.save(path).err(),
            None => Some("no config directory (is HOME set?)".into()),
        };
        cx.notify();
    }

    fn location(&self) -> String {
        match &self.path {
            Some(path) => format!("Saves to {}", path.display()),
            None => "no config directory (is HOME set?)".into(),
        }
    }

    fn section(&self, title: &'static str, clusters: &[ToolbarCluster], cx: &Context<Self>) -> Div {
        let mut rows = div().flex().flex_wrap().gap(px(ROW_GAP));
        for (ix, meta) in BUTTONS
            .iter()
            .enumerate()
            .filter(|(_, meta)| clusters.contains(&meta.button.cluster()))
        {
            rows = rows.child(self.button_row(ix, meta, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .child(
                div()
                    .pt(px(10.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(rgba(theme::c().hint_text))
                    .child(title),
            )
            .child(rows)
    }

    /// One button's editor row: the toolbar's own icon and name, the
    /// shortcut that always works (hidden buttons keep their keys),
    /// and the visibility switch.
    fn button_row(&self, ix: usize, meta: &ButtonMeta, cx: &Context<Self>) -> Stateful<Div> {
        let visible = self.settings.toolbar_visible(meta.button);
        let entity = cx.entity();
        let button = meta.button;
        let toggle = Switch::new((meta.debug_id, ix))
            .accessibility_label(meta.name)
            .checked(visible)
            .on_change(move |checked, _, _, cx| {
                entity.update(cx, |this, cx| {
                    this.set_button(button, checked, cx);
                });
            })
            .relative()
            .w(px(SWITCH_W))
            .h(px(SWITCH_H))
            .child(
                SwitchTrack::new((ElementId::from(meta.debug_id), "track"))
                    .checked(visible)
                    .absolute()
                    .size_full()
                    .rounded_full()
                    .bg(rgba(theme::c().toolbar_border))
                    .styles(|s| s.checked(|s| s.bg(rgba(theme::c().accent))))
                    .child(
                        SwitchThumb::new(visible)
                            .absolute()
                            .top(px(2.))
                            .left(px(if visible {
                                SWITCH_W - SWITCH_THUMB - 2.
                            } else {
                                2.
                            }))
                            .size(px(SWITCH_THUMB))
                            .rounded_full()
                            .bg(rgba(0xFFFFFFFF)),
                    ),
            );
        div()
            .id(("settings-row", ix))
            .w(px(ROW_W))
            .h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(px(6.))
            .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
            .child(meta.icon.svg())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.)).child(meta.name))
                    .children((!meta.shortcut.is_empty()).then(|| {
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(theme::c().hint_text))
                            .child(meta.shortcut)
                    })),
            )
            .child(div().flex_1())
            .child(toggle)
    }

    fn footer(&self, cx: &Context<Self>) -> Div {
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(12.))
            .child(
                Button::new("settings-reset")
                    .accessibility_label("Reset to defaults")
                    .px(px(12.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(rgba(theme::c().btn_text))
                    .border_1()
                    .border_color(rgba(theme::c().toolbar_border))
                    .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
                    .focus_visible(|s| s.border_color(rgba(theme::c().accent)))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| this.reset(cx));
                    })
                    .child("Reset to defaults"),
            )
            .child(
                div()
                    .flex_1()
                    .text_right()
                    .text_size(px(11.))
                    .text_color(if self.error.is_some() {
                        rgba(theme::c().accent)
                    } else {
                        rgba(theme::c().hint_text)
                    })
                    .child(self.error.clone().unwrap_or_else(|| self.location())),
            )
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-root")
            .size_full()
            .bg(rgba(theme::c().toolbar_bg))
            .text_color(rgba(theme::c().toolbar_text))
            .track_focus(&self.focus)
            .key_context("ShotoriSettings")
            // Esc mirrors the titlebar close (the binding is registered
            // in `run`; handled here — app-level backstops never see
            // window-dispatched actions)
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| {
                window.remove_window();
            }))
            .child(
                div()
                    .id("settings-body")
                    .size_full()
                    .flex()
                    .flex_col()
                    .p(px(PAGE_PAD))
                    .gap(px(ROW_GAP))
                    // the section list fits the default window; scroll
                    // keeps a resized-smaller window usable
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .pb(px(8.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(rgba(theme::c().btn_text))
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgba(theme::c().hint_text))
                                    .child("Changes apply the next time you take a screenshot."),
                            ),
                    )
                    .children(
                        SECTIONS
                            .iter()
                            .map(|(title, clusters)| self.section(title, clusters, cx)),
                    )
                    .child(div().flex_1())
                    .child(self.footer(cx)),
            )
    }
}
