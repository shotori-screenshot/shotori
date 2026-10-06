# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori">
</p>

[![Crates.io](https://img.shields.io/crates/v/shotori.svg)](https://crates.io/crates/shotori)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Linux-8892bf)
![Status](https://img.shields.io/badge/status-early%20development-orange)

Wayland 原生的截图工具，内置本地 OCR——整套 UI 用
[gpui-kit](https://crates.io/crates/gpui-kit) 手绘。

冻结屏幕、拖框选区，然后复制、保存、标注，或者直接把图里的文字读出来——
全程不用碰鼠标。

**[English](README.md)**

## 状态

Shotori 处于**早期开发阶段**（pre-1.0）。截图、标注、复制、保存、OCR、
贴图这些核心流程已可日常使用，但仍会有毛边：功能可能在版本之间无预警地
调整或移除，命令行参数、键位与主题格式尚未稳定。已在 niri、sway、
Hyprland 上测试，其他 Wayland 合成器表现可能不同。欢迎到
[issue 区](https://github.com/mengh04/shotori/issues)报告问题与反馈。

## 功能

- 区域选择，画完可再调整；多屏感知，混合缩放与旋转输出、跨屏选区都能正确处理
- 标注：矩形、椭圆、直线、折线、箭头、序号、画笔、荧光笔、马赛克/模糊、橡皮、文字
- 选择工具（`V`）：绘制永远不会顺带选中——拾取、移动、缩放、微调已放置的标注，
  以及双击编辑文字/序号徽标的值，都在这个模式下进行
- 对象橡皮擦：笔刷（圆圈实时显示实际作用范围）或矩形框选，整条删除触碰到的
  标注——不会出现擦了一半的像素；一次撤销即可恢复整次擦除
- 一键清除全部标注（`Ctrl+Shift+Del` 或工具栏垃圾桶按钮）；选区保持不变，
  一次撤销即可全部恢复
- 拖动角点（或形状手柄）时出现放大镜：3 倍冻结画面悬浮在落点旁，
  十字线标记精确像素，不遮挡落点本身
- 双击已有文字可原位编辑，实时换行，可调整字号和颜色，整次编辑可一步撤销；
  文本框与选区四边保留 2px，超出底部的输入、粘贴或字号调整不会生效，避免保存被截断的内容
- 贴图：把选区裁剪成置顶浮动小窗，截图界面关闭后依然保留——可跨屏拖动、滚轮缩放
- 长截图：框选后 `Ctrl+L`，自己滚动内容（终端、网页均可），或按住框上工具栏
  的 ⇕ 按钮进入拖动模式、自由拖动框的垂直位置——引擎实时拼接；工具栏还有
  复制 / 保存 / 取消；侧边面板同步显示长图增长并高亮当前视口位置。
  （自动滚轮注入在 niri 上暂不可用，自动模式经 `SHOTORI_SCROLL_AUTO=1` 供测试）
- 复制到剪贴板、系统"另存为"对话框保存，或 OCR 成文字（首次下载 ~31MB 模型后完全离线）
- CLI 全屏静默截图
- 可选托盘图标；主题跟随系统深浅色

## 环境要求

- wlroots 系 Wayland 合成器（niri、sway、Hyprland……）
- 保存对话框依赖 `xdg-desktop-portal`（绝大多数桌面发行版默认就有）
- 通知 daemon（dunst、mako、swaync……）可选

## 安装

```bash
cargo install shotori        # crates.io
paru -S shotori              # AUR（预编译二进制）
```

也可以从 [GitHub Releases](https://github.com/mengh04/shotori/releases)
直接下载二进制。

需要应用启动器入口和桌面图标时，先确保桌面环境的 PATH 中能找到 `shotori`，
再在源码目录或解压后的发行包目录执行：

```bash
sh tools/install-desktop.sh   # 安装到 ~/.local/share，无需 root
```

打包者可用 `DESTDIR="$pkgdir" sh tools/install-desktop.sh /usr`。
托盘和通知图标已内嵌，不依赖这一步安装；AUR 包会自动安装桌面资源。

绑到键位上，比如 niri：

```kdl
Mod+Shift+S { spawn "shotori"; }
```

`shotori tray` 常驻系统托盘（StatusNotifierItem；waybar、KDE Plasma、
GNOME appindicator 扩展均可用）。

## 使用

运行 `shotori`（或 `shotori gui`）：所有屏幕冻结并出现选区浮层，松开后
选区下方出现等价按钮的工具栏。

| 按键               | 功能                                     |
| ------------------ | ---------------------------------------- |
| 拖动               | 选择区域                                 |
| `Ctrl+A`           | 全选当前屏幕；再按一次 → 所有屏幕        |
| `Enter` / `Ctrl+C` | 复制选区到剪贴板                         |
| `Ctrl+S`           | 保存选区——系统"另存为"对话框            |
| `Ctrl+O`           | OCR 选区 → 文字进剪贴板                  |
| `Ctrl+P`           | 把选区钉成贴图                           |
| `Ctrl+L`           | 长截图——滚动内容或拖动边框，实时拼接；     |
|                    | Enter/Ctrl+C 复制、Ctrl+S 保存、Esc 取消   |
| `Esc`              | 放弃当前拖动 / 退出                      |

非交互截图（不出现浮层）：

```sh
shotori full                 # 全部屏幕 → 剪贴板
shotori full -p ~/Pictures   # → 目录下带时间戳的 PNG
shotori full -d 2            # 先等 2 秒
```

主题：`shotori --theme light`（也支持 `dark`、`high_contrast`；`auto`
跟随系统）。自定义配色：把
[`docs/theme.example.toml`](docs/theme.example.toml) 复制到
`~/.config/shotori/theme.toml`。

完整命令行参数见 `shotori --help`。

## 开发

```bash
git clone https://github.com/mengh04/shotori
cd shotori
cargo build --release
cargo test    # 单元测试，无需合成器
```

CI 强制 `cargo fmt --all --check` 和
`cargo clippy --all-targets -- -D warnings`，推送前请先本地跑一遍。

- 应用图标：[SVG 和多尺寸 PNG/ICO 资源](assets/app/README.md)；修改后运行
  `python3 tools/generate-icons.py` 重新生成（需要 `rsvg-convert`）
- 模块结构：[`src/lib.rs`](src/lib.rs) 文件头
- 决策与踩坑记录：[ROADMAP.md](ROADMAP.md)

## License

[MIT](LICENSE)
