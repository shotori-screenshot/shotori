//! Settings navigation, title bar, save feedback and window resizing.
use super::*;
use gpui_kit::base::InteractiveElementExt;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::tooltip::Tooltip;

pub(super) const SECTION_SEARCH_ITEMS: [(Section, &str, IconName, &str); 8] = [
    (
        Section::Appearance,
        "settings.preferences.appearance",
        IconName::Palette,
        "theme color language reduce motion animation palette catppuccin dracula gruvbox nord rose pine tokyo night solarized github 主题 配色 语言 外观 动画 过渡",
    ),
    (
        Section::Capture,
        "settings.preferences.capture",
        IconName::Scan,
        "capture toolbar selection window snap crosshair loupe zoom size label dim opacity scroll long qr double click right cancel 截图 工具栏 选区 窗口 吸附 十字 放大镜 尺寸 遮罩 长截图 保留 二维码 双击 右键",
    ),
    (
        Section::Annotation,
        "settings.preferences.annotation",
        IconName::Pencil,
        "annotation palette swatch stroke arrow highlighter mosaic blur eraser text wrap bold outline number badge spotlight magnifier pencil radius fill 标注 调色板 色块 线条 箭头 荧光笔 马赛克 模糊 橡皮擦 文字 换行 粗体 描边 序号 徽章 空心 聚光灯 铅笔 圆角 填充",
    ),
    (
        Section::Export,
        "settings.preferences.export",
        IconName::Save,
        "save export folder directory path auto direct dialog filename template format png jpeg jpg webp quality compression clipboard backup 保存 导出 目录 文件夹 路径 直存 弹窗 文件名 模板 格式 质量 压缩 剪贴板 备份",
    ),
    (
        Section::Desktop,
        "settings.preferences.desktop",
        IconName::Bell,
        "notification notify toast test thumbnail timeout pin zoom opacity double click border shadow edge snap qr 通知 气泡 测试 缩略图 超时 贴图 悬浮 缩放 透明度 双击 边框 阴影 边缘 吸附 二维码",
    ),
    (
        Section::Keys,
        "settings.keys_page.title",
        IconName::Keyboard,
        "shortcuts keys keybindings undo redo clear delete tools qr ctrl shift alt enter esc wheel 快捷键 按键 撤销 重做 清空 删除 二维码 滚轮 方向键",
    ),
    (
        Section::Ocr,
        "settings.ocr_page.title",
        IconName::ScanText,
        "ocr model text upright join lines cache download ppocrv6 ppocrv5 文字 识别 模型 方向 合并 换行 缓存 下载",
    ),
    (
        Section::Advanced,
        "settings.preferences.advanced",
        IconName::Settings2,
        "advanced config files toml reload cache diagnostics wayland factory reset 高级 配置 文件 重载 缓存 诊断 恢复出厂",
    ),
];

pub(super) fn section_matches_query(
    target: Section,
    key: &'static str,
    keywords: &str,
    q: &str,
) -> bool {
    let q = q.trim();
    if q.is_empty() {
        return true;
    }
    let label = crate::i18n::t!(key).to_lowercase();
    if format!("{label} {keywords}").to_lowercase().contains(q) {
        return true;
    }
    // Let typing a concrete keystroke or shortcut action name match Keys too.
    if target == Section::Keys {
        for &(id, _) in crate::actions::CONFIGURABLE_KEYS {
            if id.contains(q) {
                return true;
            }
        }
    }
    false
}

/// One title-bar control (34px wide, full height).
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
        .styles(|s| s.disabled(|d| d.opacity(0.45)))
        .px_3()
        .h_8()
        .flex_shrink_0()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().group_box)
        .text_sm()
        .text_color(cx.theme().foreground)
        .hover(|s| s.bg(cx.theme().secondary_hover))
        .focus_visible(|s| s.border_color(cx.theme().ring))
        .on_click(on_click)
        .child(crate::i18n::t!(label).to_string())
}

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
    pub(super) fn header(&self, window: &Window, cx: &mut Context<Self>) -> TitleBar {
        let toggle_label = if self.sidebar_collapsed {
            crate::i18n::t!("settings.nav.expand")
        } else {
            crate::i18n::t!("settings.nav.collapse")
        }
        .to_string();
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
        for (target, key, icon, keywords) in SECTION_SEARCH_ITEMS {
            if !section_matches_query(target, key, keywords, &query) {
                continue;
            }
            let label = crate::i18n::t!(key).to_string();
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
            Section::Appearance => self.page_appearance(window, cx),
            Section::Capture => self.page_capture(window, cx),
            Section::Annotation => self.page_annotation(window, cx),
            Section::Export => self.page_export(window, cx),
            Section::Desktop => self.page_desktop(window, cx),
            Section::Keys => self.page_keys(cx),
            Section::Ocr => self.page_ocr(window, cx),
            Section::Advanced => self.page_advanced(cx),
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

pub(super) fn resize_handles(window: &Window) -> Vec<AnyElement> {
    if !matches!(window.window_decorations(), Decorations::Server)
        || !window.is_resizable()
        || window.is_maximized()
        || window.is_fullscreen()
    {
        return Vec::new();
    }
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
