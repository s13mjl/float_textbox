# 📝 Float TextBox

一个基于 Rust + Win32 API 的桌面置顶悬浮文本框，可自由调整位置和大小，支持系统托盘和透明度调节。

![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)
![Win32](https://img.shields.io/badge/Win32-API-blue)
![License](https://img.shields.io/badge/License-MIT-yellow)

## ✨ 特性

- 🎯 **桌面置顶** - 始终悬浮在其他窗口之上
- 📐 **自由调整** - 拖动标题栏移动，右下角缩放
- 🎨 **深色主题** - 仿 B 站弹幕悬浮窗设计
- 🔍 **透明度调节** - 标题栏滑块或 Shift+滚轮
- 📌 **置顶切换** - 一键切换置顶状态
- 🗂️ **系统托盘** - 最小化到托盘，单击显示/隐藏
- ⌨️ **快捷键支持** - Ctrl+Q 退出 / Ctrl+L 清空 / Ctrl+T 切换置顶
- 🪶 **轻量紧凑** - 最小尺寸仅 160×100，原生单文件 exe，无运行时依赖

## 📸 界面预览

![screenshot](screenshot.png)

## 🚀 快速开始

### 从源码构建（需 Rust 工具链）

```bash
cargo build --release
```

构建完成后在 `target/release/float_textbox.exe`，双击即可运行。

### 运行已编译版本

下载 [Releases](https://github.com/s13mjl/float-textbox/releases) 里的 `float_textbox.exe`，双击即可运行，无需安装任何运行时。

## ⌨️ 快捷键

| 快捷键 | 功能 |
|--------|------|
| **Ctrl+Q** | 退出程序 |
| **Ctrl+L** | 清空文本 |
| **Ctrl+T** | 切换置顶 |
| **Shift+滚轮** | 调整透明度 |
| 标题栏拖动 | 移动窗口 |
| 右下角三角 | 调整大小 |
| 托盘单击 | 显示/隐藏 |
| 托盘右键 | 打开菜单 |

## 🛠️ 技术栈

- **Rust 1.70+** - 主语言
- **Win32 API** - 原生 GUI 实现（窗口管理、GDI 绘制、控件子类化、系统托盘）
- [windows-rs](https://github.com/microsoft/windows-rs) `0.58` - Win32 API 绑定
- [embed-manifest](https://crates.io/crates/embed-manifest) - 嵌入 Windows 应用清单（Common Controls v6 + PerMonitor V2 DPI 感知）

### 实现要点

- **无边框窗口**：`WS_POPUP | WS_THICKFRAME` + 自定义 `WM_NCCALCSIZE` 移除可视边框
- **分层透明**：`WS_EX_LAYERED` + `SetLayeredWindowAttributes` 实现整窗透明度
- **圆角裁剪**：`CreateRoundRectRgn` + `SetWindowRgn`
- **自绘控件**：`BS_OWNERDRAW` + `WM_DRAWITEM` 实现深色主题按钮
- **托盘集成**：`Shell_NotifyIconW` + 自定义 `WM_APP` 消息回调
- **滑块子类化**：`SetWindowSubclass` 拦截编辑区 `WM_MOUSEWHEEL`，实现 Shift+滚轮调节透明度
- **DPI 自适应**：PerMonitor V2 感知 + `GetDpiForWindow` 缩放布局

## 📦 项目结构

```
float_textbox_2/
├── src/main.rs        # 主程序：窗口过程、控件、托盘、消息循环
├── build.rs           # 嵌入 Windows 清单（Common Controls v6 + DPI）
├── Cargo.toml         # 依赖与 release 配置（opt-level="z", LTO, panic=abort）
├── float_textbox.pyw  # 旧版 PyQt5 实现（保留参考）
└── README.md
```

## 📄 许可证

本项目基于 [MIT License](LICENSE) 开源。

## 🙏 致谢

- UI 风格参考 [bilibili-live-float](https://github.com/Dawnnnnnn/bilibili-live-float)
- 项目作者：[@s13mjl](https://github.com/s13mjl)
