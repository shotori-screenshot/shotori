# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori">
</p>

[![Crates.io](https://img.shields.io/crates/v/shotori.svg)](https://crates.io/crates/shotori)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Linux-8892bf)
![Status](https://img.shields.io/badge/status-early%20development-orange)

Wayland 原生截图工具——选区、标注、贴图、长截图，需要文字时本地 OCR——
整套 UI 用 [gpui-kit](https://crates.io/crates/gpui-kit) 手绘。

冻结屏幕、拖框选区，然后复制、保存、标注、贴图，或者把滚动内容拼成一张
长图——一切都在本机完成。

**[English](README.md)**

## 状态

Shotori 处于**早期开发阶段**（pre-1.0）。截图、标注、复制、保存、OCR、
贴图、长截图这些核心流程已可日常使用，但仍会有毛边：功能可能在版本之间
无预警地调整或移除，命令行参数、键位与主题格式尚未稳定。目前只在
niri 和 Hyprland 上测试过，其他 Wayland 合成器欢迎大家试用并通过
[issue](https://github.com/shotori-screenshot/shotori/issues) 反馈结果——
bug、不顺手的地方、想要的功能都欢迎提。

## 功能

- 区域选择：多显示器感知，混合缩放、旋转输出、跨屏选区都能正确处理；画完可再调整
- 标注：矩形、椭圆、箭头、序号、画笔、荧光笔、马赛克、文字等，画完可继续编辑与撤销
- 贴图：把选区钉成置顶浮动小窗，截图界面关闭后依然保留——可跨屏拖动、滚轮缩放
- 长截图：滚动内容实时拼接，侧边面板同步预览
- OCR：本地离线识别选区中的文字（首次使用需下载模型）
- 复制到剪贴板，或经系统"另存为"对话框保存
- CLI 非交互全屏截图
- 可选托盘图标

## 环境要求

- wlroots 系 Wayland 合成器（niri、sway、Hyprland……）
- 保存对话框依赖 `xdg-desktop-portal`（绝大多数桌面发行版默认就有）
- 通知 daemon（dunst、mako、swaync……）可选

## 安装

```bash
cargo install shotori        # crates.io
paru -S shotori              # AUR（预编译二进制）
```

也可以从 [GitHub Releases](https://github.com/shotori-screenshot/shotori/releases)
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
|                    | 会话内 Enter 复制、Ctrl+S 保存、Esc 取消   |
| `Esc`              | 放弃当前拖动 / 退出                      |

## 贡献

**提 issue 和写代码同样有价值**——bug、用着不合理的地方、想要的功能，
都欢迎到 [issue 区](https://github.com/shotori-screenshot/shotori/issues)
提出来（中文即可）。想提交代码的话，构建步骤、代码规约和 PR 检查清单
都在 [CONTRIBUTING.md](CONTRIBUTING.md)（英文）。

## 开发

```bash
git clone https://github.com/shotori-screenshot/shotori
cd shotori
cargo build --release
cargo test    # 单元测试，无需合成器
```

构建依赖、CI 检查项、代码规约与 PR 清单见
[CONTRIBUTING.md](CONTRIBUTING.md)。

## License

[MIT](LICENSE)
