> 适用范围：Windows 10 1703 及以上；快捷键仅在应用窗口激活时有效，不是全局热键。

# 📝 Float TextBox

一个基于 Rust + Win32 API 的桌面置顶悬浮文本框，可自由调整位置和大小，支持系统托盘和透明度调节。

![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange)
![Win32](https://img.shields.io/badge/Win32-API-blue)
![License](https://img.shields.io/badge/License-MIT-yellow)

## ✨ 特性

- 🎯 **桌面置顶** - 始终悬浮在其他窗口之上
- 📐 **自由调整** - 拖动标题栏移动，右下角缩放
- 🎨 **深色主题** - 仿 B 站弹幕悬浮窗设计
- 🔍 **透明度调节** - 标题栏滑块或 Shift+滚轮
- 📌 **置顶切换** - 一键切换置顶状态
- 🗂️ **系统托盘** - 关闭到托盘，单击显示/隐藏
- ⌨️ **快捷键支持** - Ctrl+Q 直接退出（不弹确认框）/ Ctrl+L 清空（无确认，正文仍为空时可单级 Ctrl+Z 恢复）/ Ctrl+T 切换置顶
- 🪶 **轻量紧凑** - 最小尺寸仅 160×100，原生单文件 exe（依赖 Microsoft Visual C++ 运行库）

## 🚀 快速开始

### 从源码构建（需 Rust 工具链）

```bash
cargo build --release
```

构建完成后在 `target/release/float_textbox.exe`，双击即可运行。

### 运行已编译版本

下载 [Releases](https://github.com/s13mjl/float_textbox/releases) 里的 `float_textbox.exe`，双击即可运行。发布版由 MSVC 工具链构建，依赖 Microsoft Visual C++ 2015–2022 运行库；如果系统提示缺少 `VCRUNTIME140.dll`，请先安装对应架构的 Redistributable。

## ⌨️ 快捷键

| 快捷键 | 功能 |
|--------|------|
| **Ctrl+Q** | 直接退出程序，不弹确认框 |
| **Ctrl+L** | 清空文本；不弹确认；清空后正文仍为空时可按 Ctrl+Z 恢复 |
| **Ctrl+Z** | 清空后且正文仍为空时恢复清空前内容（单级；正文重新变为非空后失效）；其他情况交给编辑框原生撤销 |
| **Ctrl+T** | 切换置顶 |
| **Shift+滚轮** | 调整透明度 |
| **Tab / Shift+Tab** | 在编辑框、按钮和滑块之间移动焦点 |
| **空格 / 回车** | 激活当前聚焦的按钮 |
| 标题栏拖动 | 移动窗口 |
| 右下角斜线手柄 | 调整大小 |
| 托盘单击 | 显示/隐藏 |
| 托盘右键 | 打开菜单 |

以上快捷键仅在应用窗口激活时有效，窗口隐藏到托盘后无效；不是全局热键。
其中 `Ctrl+Q`、`Ctrl+L`、`Ctrl+T` 通过窗口加速键处理；`Ctrl+Shift`、`Ctrl+Alt` 或 `Ctrl+Win` 组合不会误触发这些命令。`Ctrl+Z`、Tab、空格/回车和 Shift+滚轮由控件子类化或窗口消息处理。
标准多行 EDIT 只保留最近一次操作的撤销队列，没有原生重做，因此不提供 `Ctrl+Shift+Z` 重做。
文本框内容、窗口位置、大小、透明度和置顶状态均不持久化，退出前请自行复制保存。
编辑框最多接受 1,000,000 个 UTF-16 单元；达到上限时底栏显示“字符: 已达上限”。字符数按 Unicode 码点统计，CRLF 只算一个换行，组合附加符和 ZWJ emoji 序列会拆成多个码点。

## 🛠️ 技术栈

- **Rust 1.85+** - 主语言（`rust-version` 已在 `Cargo.toml` 中声明；实际构建使用 1.98.0）
- **Win32 API** - 原生 GUI 实现（窗口管理、GDI 绘制、控件子类化、系统托盘）
- [windows-rs](https://github.com/microsoft/windows-rs) `0.58` - Win32 API 绑定
- [embed-manifest](https://crates.io/crates/embed-manifest) - 嵌入 Windows 应用清单（Common Controls v6 + PerMonitor V2 DPI 感知）

### 实现要点

- **无边框窗口**：`WS_POPUP`（不使用 `WS_THICKFRAME`）+ 自定义 `WM_NCCALCSIZE` / `WM_NCPAINT` 移除系统非客户区，再用 DWM `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE` 关掉 DWM 合成边框，避免出现白色或灰色描边
- **分层透明**：`WS_EX_LAYERED` + `SetLayeredWindowAttributes` 实现整窗透明度
- **圆角裁剪**：`CreateRoundRectRgn` + `SetWindowRgn` 自绘圆角，并关闭 Windows 11 的 DWM 圆角叠加，避免边角出现细缝
- **自绘控件**：`BS_OWNERDRAW` + `WM_DRAWITEM` 实现深色主题按钮
- **深色自绘滚动条**：编辑框不带 `WS_VSCROLL`（标准 EDIT 的滚动条由控件自己绘制，无法子类化着色），右侧改用独立的自绘滚动条子窗口；轨道与编辑区同色、滑块为深灰，上下箭头按钮、轨道翻页、滚轮和拖动均自绘实现
- **托盘集成**：`Shell_NotifyIconW` + 自定义 `WM_APP` 消息回调
- **控件子类化**：`SetWindowSubclass` 在编辑区、按钮和滑块上统一处理 Shift+滚轮、Tab 焦点环和键盘激活
- **DPI 自适应**：PerMonitor V2 感知 + `GetDpiForWindow` 缩放布局
- **离屏绘制**：复用兼容 DC 和位图完成双缓冲；`DeferWindowPos` 批量布局；缩放时同步刷新整窗和子控件；字符计数 120 ms 防抖
- **清空撤销**：清空前保存单级应用快照，文本仍为空时 `Ctrl+Z` 恢复；不弹出确认框，正文重新变为非空后快照失效

## 📦 项目结构

```
float_textbox/
├── src/main.rs        # 主程序：窗口过程、控件、托盘、消息循环
├── build.rs           # 嵌入 Windows 清单（Common Controls v6 + DPI）
├── Cargo.toml         # 依赖与 release 配置（opt-level="z", LTO, panic=abort）
├── Cargo.lock         # 锁定依赖版本，保证可复现构建
└── README.md
```

## 📄 许可证

本项目基于 [MIT License](LICENSE) 开源。

## 🙏 致谢

- UI 风格参考 [bilibili-live-float](https://github.com/Dawnnnnnn/bilibili-live-float)
- 项目作者：[@s13mjl](https://github.com/s13mjl)
