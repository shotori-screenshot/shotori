//! Settings navigation, title bar, save feedback and window resizing.
use super::*;
use gpui_kit::base::InteractiveElementExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::tooltip::Tooltip;

const SIDEBAR_COLLAPSED_W: f32 = 60.;
const SIDEBAR_HEADER_H: f32 = 48.;
const CONTENT_HEADER_H: f32 = 40.;

/// Interactive window-move header strip matching client-decorated drag/zoom/menu.
fn drag_bar(id: &'static str, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let state = window.use_keyed_state((ElementId::from(id), "drag"), cx, |_, _| false);
    let is_client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
    let mut bar = div()
        .id(id)
        .window_control_area(WindowControlArea::Drag)
        .on_double_click(|_, window, _| window.zoom_window())
        .on_mouse_down_out(window.listener_for(&state, |should_move, _, _, _| {
            *should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |should_move, _, _, _| {
                *should_move = true;
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |should_move, _, _, _| {
                *should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(&state, |should_move, _, window, _| {
            if *should_move {
                *should_move = false;
                window.start_window_move();
            }
        }));
    if is_client_decorated {
        bar = bar.on_mouse_down(MouseButton::Right, |ev, window, _| {
            window.show_window_menu(ev.position);
        });
    }
    bar
}

pub(super) const SECTION_SEARCH_ITEMS: [(Section, &str, IconName, &str); 8] = [
    (
        Section::Appearance,
        "settings.preferences.appearance",
        IconName::Palette,
        "theme color capsule frosted language reduce motion animation palette catppuccin dracula gruvbox nord rose pine tokyo night solarized github 主题 配色 胶囊 毛玻璃 语言 外观 动画 过渡",
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

/// Circular capsule window-control pill (`28×28`).
fn window_control(
    id: &'static str,
    icon: IconName,
    danger: bool,
    pill_hover: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let hover_bg = if danger { theme.danger } else { pill_hover };
    let hover_fg = if danger {
        theme.danger_foreground
    } else {
        theme.foreground
    };
    div()
        .id(id)
        .flex()
        .size_7()
        .flex_shrink_0()
        .rounded_full()
        .justify_center()
        .items_center()
        .text_color(theme.muted_foreground)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .on_double_click(|_, _, cx| cx.stop_propagation())
        .child(Icon::new(icon).size_3p5())
}

/// Restores the editable section using its existing mutation entry point.
pub(super) fn reset_button(
    label: &'static str,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let s = super::palette::surfaces(cx);
    let text = crate::i18n::t!(label).to_string();
    Button::new("section-reset")
        .accessibility_label(text.clone())
        .styles(|st| st.disabled(|d| d.opacity(0.45)))
        .flex()
        .items_center()
        .gap_1p5()
        .px_3p5()
        .h_8()
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(s.hairline)
        .bg(s.pill_bg)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().foreground)
        .hover(move |st| st.bg(s.pill_hover))
        .focus_visible(|st| st.border_color(cx.theme().ring))
        .on_click(on_click)
        .child(
            Icon::new(IconName::Undo2)
                .size_3p5()
                .text_color(cx.theme().muted_foreground),
        )
        .child(text)
}

fn window_controls(window: &Window, pill_hover: Hsla, cx: &App) -> Option<Stateful<Div>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let supported = window.window_controls();
    let mut controls = div()
        .id("title-bar-controls")
        .flex()
        .items_center()
        .gap_1p5()
        .h_full()
        .flex_shrink_0();
    if supported.minimize {
        controls = controls.child(window_control(
            "title-bar-minimize",
            IconName::WindowMinimize,
            false,
            pill_hover,
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
            pill_hover,
            |_, window, _| window.zoom_window(),
            cx,
        ));
    }
    Some(controls.child(window_control(
        "title-bar-close",
        IconName::WindowClose,
        true,
        pill_hover,
        |_, window, _| window.remove_window(),
        cx,
    )))
}

fn section_id(section: Section) -> &'static str {
    match section {
        Section::Appearance => "appearance",
        Section::Capture => "capture",
        Section::Annotation => "annotation",
        Section::Export => "export",
        Section::Desktop => "desktop",
        Section::Keys => "keys",
        Section::Ocr => "ocr",
        Section::Advanced => "advanced",
        Section::About => "about",
    }
}

impl SettingsWindow {
    pub(super) fn content_header(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let s = super::palette::surfaces(cx);
        drag_bar("content-drag-header", window, cx)
            .h(px(CONTENT_HEADER_H))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end()
            .pt_2()
            .pr_3()
            .children(window_controls(window, s.pill_hover, cx))
    }

    fn nav_item(
        &self,
        target: Section,
        label: String,
        icon: IconName,
        collapsed: bool,
        s: &super::palette::SettingsSurfaces,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.section == target;
        let dark = cx.theme().is_dark();
        let accent = cx.theme().primary;
        let pill_hover = s.pill_hover;
        let active_bg = s.selected_card_bg;
        let active_border = accent.opacity(if dark { 0.28 } else { 0.22 });
        let inactive_icon = cx.theme().muted_foreground;
        let id = SharedString::from(format!("nav-item-{}", section_id(target)));

        if collapsed {
            let tip_label = label.clone();
            let btn = Button::new(id)
                .accessibility_label(label)
                .size(px(38.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .border_1()
                .border_color(if active {
                    active_border
                } else {
                    cx.theme().transparent
                })
                .bg(if active {
                    active_bg
                } else {
                    cx.theme().transparent
                })
                .hover(move |st| if active { st } else { st.bg(pill_hover) })
                .focus_visible(|st| st.border_color(cx.theme().ring))
                .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tip_label.clone())).into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_section(target, cx);
                }))
                .child(Icon::new(icon).size_4().text_color(if active {
                    accent
                } else {
                    inactive_icon
                }));
            return div()
                .flex()
                .justify_center()
                .w_full()
                .child(btn)
                .into_any_element();
        }

        Button::new(id)
            .accessibility_label(label.clone())
            .w_full()
            .h(px(36.))
            .px_3()
            .flex()
            .items_center()
            .justify_start()
            .gap_2p5()
            .rounded(px(10.))
            .border_1()
            .border_color(if active {
                active_border
            } else {
                cx.theme().transparent
            })
            .bg(if active {
                active_bg
            } else {
                cx.theme().transparent
            })
            .text_sm()
            .font_weight(if active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .text_color(if active {
                cx.theme().foreground
            } else {
                s.sidebar_text
            })
            .hover(move |st| if active { st } else { st.bg(pill_hover) })
            .focus_visible(|st| st.border_color(cx.theme().ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_section(target, cx);
            }))
            .child(
                Icon::new(icon)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(if active { accent } else { inactive_icon }),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .into_any_element()
    }

    pub(super) fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = super::palette::surfaces(cx);
        let collapsed = self.sidebar_collapsed || self.compact;
        let query = self.search.read(cx).value().to_lowercase();
        let pill_hover = s.pill_hover;
        let fg = cx.theme().foreground;

        let toggle_label = if collapsed {
            crate::i18n::t!("settings.nav.expand")
        } else {
            crate::i18n::t!("settings.nav.collapse")
        }
        .to_string();

        let brand_toggle = div()
            .id("settings-brand-toggle")
            .debug_selector(|| "settings-brand-toggle".into())
            .flex()
            .items_center()
            .flex_shrink_0()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_double_click(|_, _, cx| cx.stop_propagation())
            .child(if collapsed {
                Button::new("settings-brand-button")
                    .accessibility_label(toggle_label.clone())
                    .size_9()
                    .rounded(px(11.))
                    .hover(move |st| st.bg(pill_hover))
                    .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(toggle_label.clone())).into())
                    .child(img(ImageSource::Render(SIDEBAR_LOGO.clone())).size_5())
            } else {
                Button::new("settings-brand-button")
                    .accessibility_label(toggle_label.clone())
                    .size_7()
                    .rounded_full()
                    .text_color(cx.theme().muted_foreground)
                    .hover(move |st| st.bg(pill_hover).text_color(fg))
                    .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(toggle_label.clone())).into())
                    .child(Icon::new(IconName::PanelLeftClose).size_4())
            });

        let top_bar = if collapsed {
            drag_bar("sidebar-drag-header", window, cx)
                .h(px(SIDEBAR_HEADER_H))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(brand_toggle)
        } else {
            drag_bar("sidebar-drag-header", window, cx)
                .h(px(SIDEBAR_HEADER_H))
                .flex_shrink_0()
                .px_3p5()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            img(ImageSource::Render(SIDEBAR_LOGO.clone()))
                                .size(px(22.))
                                .flex_shrink_0()
                                .rounded(px(6.)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .truncate()
                                .child("Shotori"),
                        ),
                )
                .child(brand_toggle)
        };

        let search_box = (!collapsed).then(|| {
            div().px_2p5().pb_2().flex_shrink_0().child(
                Input::new(&self.search)
                    .small()
                    .cleanable(true)
                    .rounded(px(10.))
                    .bg(s.well_bg)
                    .border_color(s.divider)
                    .prefix(
                        Icon::new(IconName::Search)
                            .size_3p5()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
        });

        let mut nav_list = div()
            .id("settings-nav-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2p5()
            .py_1()
            .flex()
            .flex_col()
            .gap_1();
        let mut count = 0;
        for (target, key, icon, keywords) in SECTION_SEARCH_ITEMS {
            if !collapsed && !section_matches_query(target, key, keywords, &query) {
                continue;
            }
            let label = crate::i18n::t!(key).to_string();
            count += 1;
            nav_list = nav_list.child(self.nav_item(target, label, icon, collapsed, &s, cx));
        }
        if count == 0 && !collapsed {
            nav_list = nav_list.child(
                div()
                    .px_3()
                    .py_2p5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t!("settings.preferences.no_results").to_string()),
            );
        }

        let about = self.nav_item(
            Section::About,
            crate::i18n::t!("settings.about.nav").to_string(),
            IconName::Info,
            collapsed,
            &s,
            cx,
        );

        let footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(div().mx_3().h(px(1.)).bg(s.divider))
            .child(div().px_2p5().py_2().child(about));

        let mut island = div()
            .id("settings-navigation")
            .flex()
            .flex_col()
            .h_full()
            .rounded(px(18.))
            .bg(s.sidebar_bg)
            .border_1()
            .border_color(s.hairline)
            .shadow(s.card_shadow.clone())
            .overflow_hidden()
            .child(top_bar)
            .children(search_box)
            .child(nav_list)
            .child(footer);

        island = if collapsed {
            island.w(px(SIDEBAR_COLLAPSED_W))
        } else {
            island.w(rems(SIDEBAR_W))
        };
        div().h_full().py_3().pl_3().flex_shrink_0().child(island)
    }

    pub(super) fn save_error(&self, cx: &mut Context<Self>) -> Option<Div> {
        use gpui_kit::component::collapsible::Collapsible;

        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let banner_bg = s.card_bg.blend(cx.theme().danger.opacity(0.10));

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
                        .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                        .hover(move |st| st.bg(pill_hover))
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
                    .w_full()
                    .max_w(rems(CONTENT_MAX))
                    .mx_auto()
                    .px_8()
                    .pb_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .border_1()
                            .border_color(cx.theme().danger.opacity(0.45))
                            .rounded(px(12.))
                            .bg(banner_bg)
                            .overflow_hidden()
                            .child(
                                Collapsible::new()
                                    .open(self.config_error_expanded)
                                    .child(trigger)
                                    .content(
                                        div()
                                            .id("config-error-detail")
                                            .max_h(rems(10.))
                                            .overflow_y_scroll()
                                            .px_3p5()
                                            .pb_3()
                                            .text_sm()
                                            .line_height(relative(1.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(error.clone()),
                                    ),
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
