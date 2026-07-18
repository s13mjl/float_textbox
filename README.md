# 📝 Float TextBox

一个基于 PyQt5 的桌面置顶悬浮文本框，可自由调整位置和大小，支持系统托盘和透明度调节。

![Python](https://img.shields.io/badge/Python-3.8%2B-blue)
![PyQt5](https://img.shields.io/badge/PyQt5-5.15%2B-green)
![License](https://img.shields.io/badge/License-MIT-yellow)

## ✨ 特性

- 🎯 **桌面置顶** - 始终悬浮在其他窗口之上
- 📐 **自由调整** - 拖动标题栏移动，右下角缩放
- 🎨 **深色主题** - 仿 B 站弹幕悬浮窗设计
- 🔍 **透明度调节** - 标题栏滑块或 Shift+滚轮
- 📌 **置顶切换** - 一键切换置顶状态
- 🗂️ **系统托盘** - 最小化到托盘，单击显示/隐藏
- ⌨️ **快捷键支持** - Ctrl+Q 退出 / Ctrl+L 清空 / Ctrl+T 切换置顶
- 🪶 **轻量紧凑** - 最小尺寸仅 160×100

## 📸 界面预览

![screenshot](screenshot.png)

## 🚀 快速开始

### 直接运行（需安装 PyQt5）

```bash
pip install PyQt5
python float_textbox.pyw
```

### 运行打包版本

下载 [Releases](https://github.com/s13mjl/float-textbox/releases) 里的 `FloatTextBox.exe`，双击即可运行。

## 📦 从源码打包

```bash
pip install PyQt5 pyinstaller
pyinstaller --noconfirm --onefile --windowed --name "FloatTextBox" float_textbox.pyw
```

打包完成后在 `dist/FloatTextBox.exe`。

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

- **Python 3.8+** - 主语言
- **PyQt5** - GUI 框架
- **PyInstaller** - 打包工具

## 📄 许可证

本项目基于 [MIT License](LICENSE) 开源。

## 🙏 致谢

- UI 风格参考 [bilibili-live-float](https://github.com/Dawnnnnnn/bilibili-live-float)
- 项目作者：[@s13mjl](https://github.com/s13mjl)
