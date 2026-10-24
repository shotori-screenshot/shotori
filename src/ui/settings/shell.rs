//! Settings navigation, title bar, save feedback and window resizing.
use super::*;
use gpui_kit::base::InteractiveElementExt;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::tooltip::Tooltip;

/// One title-bar control (34px wide, full height), styled like the
/// component's cluster: the quiet secondary hover for minimize /
/// maximize, the theme's danger tint for close.
fn window_control(
    id: &'static str,
    icon: IconName,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
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
        // Keep the bar's drag handler from treating a control press as
        // the start of a window move.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(Icon::new(icon).size_4())
}

/// Restores the editable section using its existing mutation entry point.
pub(super) fn reset_button(
    label: &'static str,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new("section-reset")
        .accessibility_label(crate::i18n::t!(label).to_string())
        .px_3()
        .h_8()
        .flex_shrink_0()
        .rounded(cx.theme().radius)
        .text_sm()
        .text_color(cx.theme().foreground)
        .hover(|s| s.bg(cx.theme().secondary_hover))
        .focus_visible(|s| s.border_1().border_color(cx.theme().ring))
        .on_click(on_click)
        .child(crate::i18n::t!(label).to_string())
}

/// The title bar's control cluster for a server-decorated window that
/// has no system bar (see [`SettingsWindow::header`]). Only the
/// controls the compositor advertises through `wm_capabilities` are
/// offered; close is always ours.
fn window_controls(window: &Window, cx: &App) -> impl IntoElement {
    let supported = window.window_controls();
    let mut controls = div()
        .id("title-bar-controls")
        .flex()
        .items_center()
        .h_full()
        .flex_shrink_0();
    if supported.minimize {
        controls = controls.child(window_control(
            "title-bar-minimize",
            IconName::WindowMinimize,
            false,
            |_, window, _| window.minimize_window(),
            cx,
        ));
    }
    if supported.maximize {
        let icon = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };
        controls = controls.child(window_control(
            "title-bar-maximize",
            icon,
            false,
            |_, window, _| window.zoom_window(),
            cx,
        ));
    }
    controls.child(window_control(
        "title-bar-close",
        IconName::WindowClose,
        true,
        |_, window, _| window.remove_window(),
        cx,
    ))
}

impl SettingsWindow {
    /// The self-drawn title bar: brand mark plus window title on the
    /// left, the window controls on the right. The app owns it because
    /// the window asks for client-side decorations; the branding lives
    /// here (not in the sidebar header) so it never moves or shrinks
    /// when the navigation rail collapses. `TitleBar` supplies the drag
    /// / double-click-zoom behavior and the component's control cluster
    /// wherever the compositor honors client decorations.
    pub(super) fn header(&self, window: &Window, cx: &mut Context<Self>) -> TitleBar {
        let toggle_label = if self.sidebar_collapsed {
            crate::i18n::t!("settings.nav.expand")
        } else {
            crate::i18n::t!("settings.nav.collapse")
        }
        .to_string();
        // TitleBar owns double-click-to-zoom. Stop both presses and clicks at
        // the icon so a double activation cannot bubble into window chrome.
        let brand = div()
            .id("settings-brand-toggle")
            .debug_selector(|| "settings-brand-toggle".into())
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_double_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new("settings-brand-button")
                    .accessibility_label(toggle_label.clone())
                    .size_7()
                    .rounded(cx.theme().radius)
                    .hover(|s| s.bg(cx.theme().secondary_hover))
                    .focus_visible(|s| s.border_1().border_color(cx.theme().ring))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(toggle_label.clone())).into())
                    // Same optical nudge as `help_tooltip`: the CJK
                    // title text beside it rides ~2px low in its line
                    // box, box-centering reads visibly high.
                    .child(
                        img(ImageSource::Render(SIDEBAR_LOGO.clone()))
                            .size_5()
                            .relative()
                            .top(px(2.)),
                    ),
            );
        let leading = div()
            .flex()
            .items_center()
            .h_full()
            .gap_2()
            .child(brand)
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t!("settings.title").to_string()),
            );
        // `TitleBar` hides its own control cluster when the compositor
        // claims server-side decorations — it must not duplicate a
        // system title bar. But some compositors claim that while
        // drawing nothing (niri with `prefer-no-csd`), so OUR bar is the
        // only chrome the window ever gets and has to carry the
        // controls itself. In client-decorated mode the component's
        // cluster handles it (outside the drag region, same as here).
        let own_controls = matches!(window.window_decorations(), Decorations::Server)
            .then(|| window_controls(window, cx));
        TitleBar::new().child(leading).children(own_controls)
    }

    pub(super) fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::{
            Sizable,
            input::Input,
            sidebar::{Sidebar, SidebarItem, SidebarMenu, SidebarMenuItem},
        };
        let collapsed = self.sidebar_collapsed || self.compact;
        let query = self.search.read(cx).value().to_lowercase();
        let mut menu = SidebarMenu::new();
        let mut count = 0;
        for (target, key, icon, keywords) in [
            (
                Section::Appearance,
                "settings.preferences.appearance",
                IconName::Palette,
                "theme color language 主题 配色 语言 外观",
            ),
            (
                Section::Capture,
                "settings.preferences.capture",
                IconName::Scan,
                "capture toolbar annotation selection 截图 工具栏 标注 选区 遮罩",
            ),
            (
                Section::Keys,
                "settings.keys_page.title",
                IconName::Keyboard,
                "shortcuts keys 快捷键 按键",
            ),
            (
                Section::Ocr,
                "settings.ocr_page.title",
                IconName::ScanText,
                "ocr model text 文字 识别 模型",
            ),
            (
                Section::Advanced,
                "settings.preferences.advanced",
                IconName::Settings2,
                "advanced config files 高级 配置 文件",
            ),
        ] {
            let label = crate::i18n::t!(key).to_string();
            if !query.trim().is_empty()
                && !format!("{label} {keywords}")
                    .to_lowercase()
                    .contains(query.trim())
            {
                continue;
            }
            count += 1;
            menu = menu.child(
                SidebarMenuItem::new(label)
                    .icon(icon)
                    .active(self.section == target)
                    .border_l_2()
                    .border_color(if self.section == target {
                        cx.theme().primary
                    } else {
                        cx.theme().transparent
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_section(target, cx);
                    })),
            );
        }
        let about = SidebarMenuItem::new(crate::i18n::t!("settings.about.nav").to_string())
            .icon(IconName::Info)
            .active(self.section == Section::About)
            .border_l_2()
            .border_color(if self.section == Section::About {
                cx.theme().primary
            } else {
                cx.theme().transparent
            })
            .on_click(cx.listener(|this, _, _, cx| this.select_section(Section::About, cx)));
        if collapsed {
            menu = menu.child(about.clone());
        }
        let mut sidebar = Sidebar::new("settings-navigation")
            .w(rems(SIDEBAR_W))
            .collapsed(collapsed)
            .child(menu);
        if !collapsed {
            sidebar = sidebar
                .header(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size_4()),
                )
                .footer(SidebarMenu::new().w_full().child(about).render(
                    "settings-about",
                    window,
                    cx,
                ));
        }
        if count == 0 {
            sidebar = sidebar.child(
                SidebarMenu::new().child(
                    SidebarMenuItem::new(
                        crate::i18n::t!("settings.preferences.no_results").to_string(),
                    )
                    .disable(true),
                ),
            );
        }
        sidebar
    }

    pub(super) fn save_error(&self, cx: &mut Context<Self>) -> Option<Div> {
        use gpui_kit::component::collapsible::Collapsible;

        self.appearance
            .error
            .as_ref()
            .or(self.error.as_ref())
            .map(|error| {
                let trigger =
                    Button::new("config-error-toggle")
                        .w_full()
                        .h_auto()
                        .p_3()
                        .flex()
                        .items_center()
                        .justify_start()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .focus_visible(|s| s.border_1().border_color(cx.theme().ring))
                        .hover(|s| s.bg(cx.theme().secondary_hover))
                        .child(
                            Icon::new(IconName::Info)
                                .size_4()
                                .text_color(cx.theme().danger),
                        )
                        .child(div().flex_1().child(
                            crate::i18n::t!("settings.preferences.config_error").to_string(),
                        ))
                        .child(
                            Icon::new(if self.config_error_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_4(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.config_error_expanded = !this.config_error_expanded;
                            cx.notify();
                        }));
                div()
                    .mx_8()
                    .mt_4()
                    .flex_shrink_0()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .child(
                        Collapsible::new()
                            .open(self.config_error_expanded)
                            .child(trigger)
                            .content(
                                div()
                                    .id("config-error-detail")
                                    .max_h(rems(10.))
                                    .overflow_y_scroll()
                                    .px_3()
                                    .pb_3()
                                    .text_sm()
                                    .line_height(relative(1.5))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(error.clone()),
                            ),
                    )
            })
    }

    pub(super) fn page(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let page = match self.section {
            Section::Advanced => self.page_advanced(cx),
            Section::Capture => self.page_capture(window, cx),
            Section::Keys => self.page_keys(cx),
            Section::Ocr => self.page_ocr(window, cx),
            Section::Appearance => self.page_appearance(window, cx),
            Section::About => self.page_about(cx),
        };
        let scroll = &self.page_scroll[self.section as usize];
        e2e::debug_settings_scroll(scroll);
        page.flex_1()
            .min_w_0()
            .min_h_0()
            .track_scroll(scroll)
            .relative()
            .child(Scrollbar::vertical(scroll).mode(ScrollbarMode::Scrolling))
    }
}

/// Some compositors advertise server decorations while drawing no frame.
/// Root's WindowBorder deliberately does nothing in that mode, so provide
/// narrow application-owned edge targets without adding another CSD inset.
/// Native client decorations continue to use the kit's own resize handling.
pub(super) fn resize_handles(window: &Window) -> Vec<AnyElement> {
    if !matches!(window.window_decorations(), Decorations::Server)
        || !window.is_resizable()
        || window.is_maximized()
        || window.is_fullscreen()
    {
        return Vec::new();
    }
    // Physical hit slop is a window-system boundary, independent of UI zoom.
    let band = px(5.);
    let corner = px(12.);
    [
        (ResizeEdge::Top, CursorStyle::ResizeUpDown),
        (ResizeEdge::Bottom, CursorStyle::ResizeUpDown),
        (ResizeEdge::Left, CursorStyle::ResizeLeftRight),
        (ResizeEdge::Right, CursorStyle::ResizeLeftRight),
        (ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight),
        (ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft),
        (ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft),
        (ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (edge, cursor))| {
        let handle = div()
            .id(("settings-resize", index))
            .absolute()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
                window.start_window_resize(edge);
            });
        match edge {
            ResizeEdge::Top => handle.top_0().left(corner).right(corner).h(band),
            ResizeEdge::Bottom => handle.bottom_0().left(corner).right(corner).h(band),
            ResizeEdge::Left => handle.left_0().top(corner).bottom(corner).w(band),
            ResizeEdge::Right => handle.right_0().top(corner).bottom(corner).w(band),
            ResizeEdge::TopLeft => handle.top_0().left_0().size(corner),
            ResizeEdge::TopRight => handle.top_0().right_0().size(corner),
            ResizeEdge::BottomLeft => handle.bottom_0().left_0().size(corner),
            ResizeEdge::BottomRight => handle.bottom_0().right_0().size(corner),
        }
        .into_any_element()
    })
    .collect()
}
