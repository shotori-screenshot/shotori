# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori" width="96" height="96">
</p>

<p align="center">
  <strong>定格桌面一瞬，让像素在本机自由流转。</strong><br>
  基于 Rust 与 <a href="https://crates.io/crates/gpui-kit">gpui-kit</a> 手绘打造的 Wayland 原生截图利器。
</p>

<p align="center">
  <a href="https://shotori.mengh04.top/zh-cn/"><img src="https://img.shields.io/badge/%E5%AE%98%E7%BD%91%E4%B8%8E%E5%AE%9E%E6%9C%BA%E6%BC%94%E7%A4%BA-shotori.mengh04.top-ff6a00?style=flat-square" alt="官方网站"></a>
  <a href="https://shotori.mengh04.top/zh-cn/getting-started/installation/"><img src="https://img.shields.io/badge/%E4%BD%BF%E7%94%A8%E6%96%87%E6%A1%A3-%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87%20%7C%20English-202028?style=flat-square" alt="使用文档"></a>
  <a href="https://github.com/shotori-screenshot/shotori/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/shotori-screenshot/shotori/ci.yml?style=flat-square&label=CI" alt="CI"></a>
  <a href="https://crates.io/crates/shotori"><img src="https://img.shields.io/crates/v/shotori.svg?style=flat-square" alt="Crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
</p>

<p align="center">
  <a href="https://shotori.mengh04.top/zh-cn/"><strong>官方网站与实机演示</strong></a> ·
  <a href="https://shotori.mengh04.top/zh-cn/getting-started/quick-start/"><strong>五分钟上手</strong></a> ·
  <a href="https://shotori.mengh04.top/zh-cn/reference/shortcuts/"><strong>快捷键速查</strong></a> ·
  <a href="https://shotori.mengh04.top/zh-cn/reference/cli/"><strong>命令行参考</strong></a> ·
  <a href="README.md"><strong>English</strong></a>
</p>

---

Shotori 冻结全部 Wayland 显示器，用单一原生二进制替代零散的 `grim + slurp + swappy + tesseract` 拼装脚本：跨屏混合缩放像素级对齐、14 种落笔后仍可随时修改的矢量与光学标注、跨屏置顶贴图、带 254px 实时预览的滚动长截图、完全离线的本地 OCR 与二维码解码——一切都在本机完成，零遥测、零云端上传。

<p align="center">
  <a href="https://shotori.mengh04.top/zh-cn/#stage">
    <img src="https://shotori.mengh04.top/media/hero-workflow-poster.webp" alt="Shotori 交互式截图覆盖层、矢量标注与双排 GPUI 工具栏" width="100%">
  </a>
</p>

> 访问 **[shotori.mengh04.top/zh-cn/](https://shotori.mengh04.top/zh-cn/)** 查看 60fps 实机演示视频、浏览器交互试玩台、全部 19 套主题 2× 实机画廊与一键合成器快捷键生成器。

## 核心亮点

- **Wayland 原生 · 混合 HiDPI 精确对齐**：基于 `zwlr-screencopy-v1` 与 `wlr-layer-shell-v1` 协议，在 **niri** 与 **Hyprland** 上主力实测，兼容 **sway**、**river**、**labwc**、**Wayfire**、**KWin**、**COSMIC** 等 wlroots 系合成器；完美处理 `1× / 1.25× / 2×` 混合缩放、旋转屏、跨屏选区与合成器 IPC 窗口吸附。
- **14 种可二次编辑矢量与光学标注**：矩形、椭圆、直线、折线、7 种箭头轮廓、Rubik 序号气泡（支持 `1/A/I`、引出箭头与删除自动重排）、中英 IME 文字、平滑画笔、正片叠底荧光笔、马赛克/模糊、1–8× 放大镜、聚光灯遮罩、选择移动与整对象橡皮擦。导出前每一笔都可随时移动、改色、撤销。
- **实时滚动长截图 (`Ctrl+L`)**：框选区域后亲自自然滚动页面，右侧 254px 预览窗实时呈现拼接生长过程（支持纵向与横向），自动剥离固定顶栏与静止背景，遇异常帧宁可跳过也不污染成图。
- **跨屏置顶贴图 (`Ctrl+P`)**：将任意选区（或通过 `shotori pin` 将本地图片）钉成无边框置顶浮窗，退出截图后依然常驻桌面，支持跨缩放显示器拖动与滚轮平滑缩放。
- **本地离线 OCR (`Ctrl+O`) 与二维码扫描 (`Ctrl+Q`)**：内置 6 档 SHA-256 校验的 PP-OCRv6 模型（首次使用一次性下载约 31 MB，此后零字节联网），支持挂接自定义 ONNX 模型；支持从屏幕选区、本地图片或 `stdin` 管道一键解码二维码。
- **免弹窗直接保存与 9 页图形化设置中心**：开启直接保存后按 `Ctrl+S` 免弹窗写入，支持 `strftime` 时间戳与 `{w}×{h}` 模板（PNG/JPEG/WebP + 保存同步复制）；在 `shotori settings` 中可直接拖拽定制 23 键工具栏与修改全部 25 组快捷键。
- **19 套内置深浅主题**：涵盖 Shotori Dark/Light、Capsule Dark/Light、Catppuccin 全家桶、Tokyo Night、Rosé Pine、Gruvbox、Nord、Dracula、Solarized Light、GitHub Light 与高对比度模式，支持 `theme.toml` 单字段覆写并自动保证 WCAG 4.5:1 对比度。

<table>
  <tr>
    <td width="50%">
      <a href="https://shotori.mengh04.top/zh-cn/guides/long-screenshots/">
        <img src="https://shotori.mengh04.top/media/long-capture-poster.webp" alt="实时滚动长截图与 254px 侧边预览">
      </a>
      <br><sub><strong>实时滚动长截图 (Ctrl+L)</strong> — 自然滚动 + 254px 实时拼接预览</sub>
    </td>
    <td width="50%">
      <a href="https://shotori.mengh04.top/zh-cn/guides/annotating/">
        <img src="https://shotori.mengh04.top/media/annotations-suite-poster.webp" alt="14 种可编辑标注工具、放大镜与整对象橡皮擦">
      </a>
      <br><sub><strong>14 种可编辑标注</strong> — 放大镜、马赛克、荧光笔、序号气泡与整笔橡皮</sub>
    </td>
  </tr>
  <tr>
    <td width="50%">
      <a href="https://shotori.mengh04.top/zh-cn/guides/pin/">
        <img src="https://shotori.mengh04.top/media/floating-pin-poster.webp" alt="桌面置顶贴图">
      </a>
      <br><sub><strong>跨屏置顶贴图 (Ctrl+P)</strong> — 始终置顶对照参考、滚轮缩放</sub>
    </td>
    <td width="50%">
      <a href="https://shotori.mengh04.top/zh-cn/reference/settings/">
        <img src="https://shotori.mengh04.top/media/settings/capture.webp" alt="9 页原生图形化设置中心">
      </a>
      <br><sub><strong>9 页图形化设置中心</strong> — 工具栏拖拽定制、25 组快捷键与免弹窗保存</sub>
    </td>
  </tr>
</table>

## 环境要求

- 实现 `zwlr-screencopy-v1` 协议的 Wayland 合成器（niri、Hyprland、sway、river、labwc、Wayfire、KWin、COSMIC……）。*不支持 GNOME Mutter 与 X11。*
- 系统保存对话框依赖 `xdg-desktop-portal`（主流发行版默认自带；开启免弹窗直接保存后可跳过）。
- 通知守护进程（`mako`、`dunst`、`swaync`……）可选。

## 安装

```bash
paru -S shotori              # Arch Linux AUR（预编译二进制 + 自动装桌面入口）
cargo install shotori        # crates.io（Rust Edition 2024）
```

也可直接从 [GitHub Releases](https://github.com/shotori-screenshot/shotori/releases) 下载预编译二进制。

若通过 `cargo install` 或压缩包手动安装，且希望在应用启动器中显示图标，可在源码或发行包目录执行：

```bash
sh scripts/install-desktop.sh   # 安装到 ~/.local/share，无需 root
```

打包者可用 `DESTDIR="$pkgdir" sh scripts/install-desktop.sh /usr`。托盘与通知图标已内嵌在二进制中，AUR 包会自动安装桌面资源。

### 绑定合成器快捷键

需要按需勾选生成配置？可直接用官网的 **[合成器快捷键生成器](https://shotori.mengh04.top/zh-cn/#cli)**。

<details>
<summary><strong>niri</strong>（<code>~/.config/niri/config.kdl</code>）</summary>

```kdl
spawn-at-startup "shotori" "tray"

binds {
    Mod+Shift+S { spawn "shotori"; }
    Print       { spawn "shotori" "full" "--screen" "--clipboard"; }
    Mod+Print   { spawn "shotori" "gui" "--window" "--tool" "arrow"; }
}
```
</details>

<details>
<summary><strong>Hyprland</strong>（<code>~/.config/hypr/hyprland.conf</code>）</summary>

```ini
exec-once = shotori tray

bind = SUPER SHIFT, S, exec, shotori
bind = , Print, exec, shotori full --screen --clipboard
bind = SUPER, Print, exec, shotori gui --window --tool arrow
```
</details>

<details>
<summary><strong>sway</strong>（<code>~/.config/sway/config</code>）</summary>

```ini
exec shotori tray

bindsym $mod+Shift+s exec shotori
bindsym Print exec shotori full --screen --clipboard
bindsym $mod+Print exec shotori gui --window --tool arrow
```
</details>

## 常用命令行速览

```bash
shotori                                     # 交互式截图覆盖层（冻结 + 吸附/拖框 + 标注）
shotori gui --window --tool arrow           # 通过 IPC 预选焦点窗口并激活箭头工具
shotori full --screen --clipboard           # 无界面静默复制当前屏幕
shotori full --output DP-1 --stdout > a.png # 输出原始 PNG 字节流到标准输出
shotori ocr --download --window --clipboard # 对当前窗口执行本地离线 OCR 并复制文字
shotori qr invoice.png                      # 解码本地图片二维码（支持 `-` 标准输入）
shotori pin mockup.png --at 420,180         # 将本地图片钉成置顶贴图
shotori outputs --json                      # 以 JSON 查看全部显示器缩放与坐标
shotori settings [PAGE]                     # 打开 9 页原生图形化设置窗口
shotori tray                                # 常驻系统托盘（StatusNotifierItem）与贴图宿主
```

完整文档、快捷键表、TOML 配置、主题列表与自定义 OCR 模型指南请参阅 **[shotori.mengh04.top/zh-cn/](https://shotori.mengh04.top/zh-cn/getting-started/installation/)**。

## 当前状态

Shotori 处于活跃的 pre-1.0 开源迭代中。截图、标注、复制、直接保存、贴图、长截图、离线 OCR、二维码扫描、主题与图形化设置等核心流程已可日常主力使用，但 CLI 参数与配置项在版本间仍可能微调。目前在 **niri** 与 **Hyprland** 上主力实测，欢迎在其他 Wayland 合成器上体验并到 [issue 区](https://github.com/shotori-screenshot/shotori/issues) 反馈（中文即可）。

## 开发与贡献

```bash
git clone https://github.com/shotori-screenshot/shotori
cd shotori
cargo build --release
cargo test    # 单元测试，无需合成器
```

提 issue、反馈毛边与写代码同样有价值；构建依赖、CI 检查项、代码规约与 PR 清单见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

[MIT](LICENSE)
