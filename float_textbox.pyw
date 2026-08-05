"""
悬浮文本框 v5 - PyQt5 仿 B 站悬浮窗
打包: pyinstaller --noconfirm --onefile --windowed --name "FloatTextBox" float_textbox.pyw
"""

import sys
from PyQt5.QtCore import Qt, QPoint, QSize
from PyQt5.QtGui import (
    QFont, QIcon, QPixmap, QPainter, QColor, QBrush, QPen, QCursor, QKeySequence
)
from PyQt5.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPlainTextEdit, QLabel, QPushButton, QSlider, QSizeGrip,
    QSystemTrayIcon, QMenu, QAction, QShortcut, QFrame
)

# --- 主题色 (仿 B 站深灰) ---
COLOR_BG = "#1e1e1e"               # 主体背景
COLOR_BAR = "#2b2b2b"              # 标题栏/底栏
COLOR_BAR_DARK = "#252525"         # 边框/分割
COLOR_TEXT = "#e0e0e0"            # 主文本
COLOR_TEXT_MUTED = "#888888"      # 次要文本
COLOR_ACCENT = "#00aeec"           # 强调色（蓝青色，对应 B 站蓝）
COLOR_ACCENT_HOVER = "#33beff"
COLOR_BTN_HOVER = "#3d3d3d"
COLOR_BTN_CLOSE = "#ff6b6b"
COLOR_SCROLL = "rgba(30,30,30,220)"


def _make_app_icon():
    """应用图标：圆角蓝色方块 + 白色 "T" """
    size = 64
    pix = QPixmap(size, size)
    pix.fill(Qt.transparent)
    p = QPainter(pix)
    p.setRenderHint(QPainter.Antialiasing, True)
    p.setBrush(QBrush(QColor(COLOR_ACCENT)))
    p.setPen(QPen(QColor(COLOR_ACCENT), 0))
    p.drawRoundedRect(2, 2, size - 4, size - 4, 12, 12)
    p.setPen(QColor("white"))
    f = QFont("Microsoft YaHei UI", 36)
    f.setBold(True)
    p.setFont(f)
    p.drawText(pix.rect(), Qt.AlignCenter, "T")
    p.end()
    return QIcon(pix)


def _make_tray_icon():
    """托盘图标"""
    size = 32
    pix = QPixmap(size, size)
    pix.fill(Qt.transparent)
    p = QPainter(pix)
    p.setRenderHint(QPainter.Antialiasing, True)
    p.setBrush(QBrush(QColor(COLOR_ACCENT)))
    p.setPen(QPen(QColor(COLOR_ACCENT), 0))
    p.drawRoundedRect(1, 1, size - 2, size - 2, 6, 6)
    p.setPen(QColor("white"))
    f = QFont("Microsoft YaHei UI", 18)
    f.setBold(True)
    p.setFont(f)
    p.drawText(pix.rect(), Qt.AlignCenter, "T")
    p.end()
    return QIcon(pix)


class FloatTextBox(QMainWindow):

    def __init__(self):
        super().__init__()
        self.drag_position = None
        self._init_ui()
        self._apply_style()
        self._init_tray()
        self._init_shortcuts()

    def _init_ui(self):
        # 无边框 + 置顶 + 任务栏可见
        self.setWindowFlags(
            Qt.FramelessWindowHint |
            Qt.WindowStaysOnTopHint
        )
        self.setAttribute(Qt.WA_TranslucentBackground)
        self.setWindowTitle("悬浮文本框")
        self.setWindowIcon(_make_app_icon())
        self.setMinimumSize(160, 100)
        self.resize(280, 200)

        # 外层容器（用于圆角背景）
        self.container = QWidget(self)
        self.container.setObjectName("container")
        self.setCentralWidget(self.container)
        self._layout = QVBoxLayout(self.container)
        self._layout.setContentsMargins(0, 0, 0, 0)
        self._layout.setSpacing(0)

        # ---- 标题栏 ----
        self.title_bar = QWidget()
        self.title_bar.setObjectName("titleBar")
        self.title_bar.setFixedHeight(28)
        tl = QHBoxLayout(self.title_bar)
        tl.setContentsMargins(12, 0, 8, 0)
        tl.setSpacing(8)

        self.title_label = QLabel("📝  悬浮文本框")
        self.title_label.setObjectName("titleLabel")
        self.title_label.setMaximumWidth(220)
        tl.addWidget(self.title_label)

        tl.addStretch()

        # 透明度滑块
        self.opacity_slider = QSlider(Qt.Horizontal)
        self.opacity_slider.setObjectName("opacitySlider")
        self.opacity_slider.setRange(30, 100)
        self.opacity_slider.setValue(95)
        self.opacity_slider.setFixedWidth(50)
        self.opacity_slider.setToolTip("透明度 (Shift+滚轮也可调整)")
        self.opacity_slider.valueChanged.connect(self._on_opacity_changed)
        tl.addWidget(self.opacity_slider)

        # 置顶按钮
        self.pin_btn = QPushButton("📌")
        self.pin_btn.setObjectName("pinBtn")
        self.pin_btn.setFixedSize(22, 22)
        self.pin_btn.setToolTip("切换置顶")
        self.pin_btn.setCheckable(True)
        self.pin_btn.setChecked(True)
        self.pin_btn.clicked.connect(self._toggle_topmost)
        tl.addWidget(self.pin_btn)

        # 最小化按钮
        min_btn = QPushButton("—")
        min_btn.setObjectName("minBtn")
        min_btn.setFixedSize(22, 22)
        min_btn.setToolTip("最小化")
        min_btn.setCursor(QCursor(Qt.PointingHandCursor))
        min_btn.clicked.connect(self.showMinimized)
        tl.addWidget(min_btn)

        # 关闭按钮（最小化到托盘）
        close_btn = QPushButton("✕")
        close_btn.setObjectName("closeBtn")
        close_btn.setFixedSize(22, 22)
        close_btn.setToolTip("最小化到托盘")
        close_btn.setCursor(QCursor(Qt.PointingHandCursor))
        close_btn.clicked.connect(self.hide)
        tl.addWidget(close_btn)

        self._layout.addWidget(self.title_bar)

        # ---- 文本编辑区 ----
        self.text_edit = QPlainTextEdit()
        self.text_edit.setObjectName("textEdit")
        self.text_edit.setPlaceholderText("在这里输入文字…")
        self.text_edit.setFrameShape(QFrame.NoFrame)
        self._layout.addWidget(self.text_edit, 1)

        # ---- 底部状态栏 ----
        self.bottom_bar = QWidget()
        self.bottom_bar.setObjectName("bottomBar")
        self.bottom_bar.setFixedHeight(20)
        bl = QHBoxLayout(self.bottom_bar)
        bl.setContentsMargins(12, 0, 8, 0)
        bl.setSpacing(8)

        self.count_label = QLabel("字符: 0")
        self.count_label.setObjectName("countLabel")
        bl.addWidget(self.count_label)

        bl.addStretch()

        clear_btn = QPushButton("清空")
        clear_btn.setObjectName("clearBtn")
        clear_btn.setFixedHeight(18)
        clear_btn.setCursor(QCursor(Qt.PointingHandCursor))
        clear_btn.clicked.connect(self._clear_text)
        bl.addWidget(clear_btn)

        # 右下角 size grip
        size_grip = QSizeGrip(self.bottom_bar)
        size_grip.setObjectName("sizeGrip")
        size_grip.setFixedSize(14, 14)
        size_grip.setCursor(QCursor(Qt.SizeFDiagCursor))
        bl.addWidget(size_grip)

        self._layout.addWidget(self.bottom_bar)

        # 字符计数监听
        self.text_edit.textChanged.connect(self._update_count)

    def _apply_style(self):
        # 用 setStyleSheet 加载到 centralWidget 即可
        self.setStyleSheet(f"""
            #container {{
                background-color: {COLOR_BG};
                border: 1px solid {COLOR_BAR_DARK};
                border-radius: 10px;
            }}
            #titleBar {{
                background-color: {COLOR_BAR};
                border-top-left-radius: 10px;
                border-top-right-radius: 10px;
                border-bottom: 1px solid {COLOR_BAR_DARK};
            }}
            #titleLabel {{
                color: {COLOR_TEXT};
                font-size: 12px;
                font-weight: 600;
                font-family: "Microsoft YaHei UI", "微软雅黑", "PingFang SC", sans-serif;
            }}
            #textEdit {{
                background-color: {COLOR_BG};
                color: {COLOR_TEXT};
                border: none;
                padding: 6px 10px;
                font-size: 13px;
                font-family: "Microsoft YaHei UI", "微软雅黑", "Consolas", "PingFang SC", sans-serif;
                selection-background-color: {COLOR_ACCENT};
                selection-color: white;
            }}
            #bottomBar {{
                background-color: {COLOR_BAR};
                border-bottom-left-radius: 10px;
                border-bottom-right-radius: 10px;
                border-top: 1px solid {COLOR_BAR_DARK};
            }}
            #countLabel {{
                color: {COLOR_TEXT_MUTED};
                font-size: 10px;
                font-family: "Microsoft YaHei UI", "微软雅黑", "PingFang SC", sans-serif;
            }}
            #minBtn, #closeBtn, #pinBtn {{
                background-color: transparent;
                color: {COLOR_TEXT};
                border: none;
                border-radius: 3px;
                font-size: 11px;
                font-weight: bold;
            }}
            #minBtn:hover {{
                background-color: {COLOR_BTN_HOVER};
            }}
            #pinBtn:checked {{
                color: {COLOR_ACCENT};
            }}
            #pinBtn:hover {{
                background-color: {COLOR_BTN_HOVER};
            }}
            #closeBtn:hover {{
                background-color: {COLOR_BTN_CLOSE};
                color: white;
            }}
            #clearBtn {{
                background-color: {COLOR_BTN_HOVER};
                color: {COLOR_TEXT};
                border: none;
                border-radius: 3px;
                padding: 1px 8px;
                font-size: 10px;
                font-family: "Microsoft YaHei UI", "微软雅黑", sans-serif;
            }}
            #clearBtn:hover {{
                background-color: #4d4d4d;
            }}
            #opacitySlider {{
                background-color: transparent;
            }}
            #opacitySlider::groove:horizontal {{
                height: 3px;
                background: #4d4d4d;
                border-radius: 1.5px;
            }}
            #opacitySlider::handle:horizontal {{
                width: 10px;
                height: 10px;
                background: {COLOR_ACCENT};
                border-radius: 5px;
                margin: -3.5px 0;
            }}
            QScrollBar:vertical {{
                background-color: transparent;
                width: 6px;
                border-radius: 3px;
            }}
            QScrollBar::handle:vertical {{
                background-color: #4d4d4d;
                border-radius: 3px;
                min-height: 20px;
            }}
            QScrollBar::handle:vertical:hover {{
                background-color: #5d5d5d;
            }}
            QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical {{
                height: 0px;
            }}
        """)

    # ---------- 系统托盘 ----------
    def _init_tray(self):
        if not QSystemTrayIcon.isSystemTrayAvailable():
            self.tray_icon = None
            return
        self.tray_icon = QSystemTrayIcon(self)
        self.tray_icon.setIcon(_make_tray_icon())
        self.tray_icon.setToolTip("悬浮文本框")

        menu = QMenu()
        show_action = QAction("显示", self)
        show_action.triggered.connect(self._show_window)
        menu.addAction(show_action)

        hide_action = QAction("隐藏", self)
        hide_action.triggered.connect(self.hide)
        menu.addAction(hide_action)

        menu.addSeparator()

        pin_action = QAction("切换置顶", self)
        pin_action.setCheckable(True)
        pin_action.setChecked(self.windowFlags() & Qt.WindowStaysOnTopHint)
        pin_action.triggered.connect(self._toggle_topmost)
        # 同步菜单选中状态
        self.pin_action = pin_action
        self.pin_btn.toggled.connect(lambda checked: pin_action.setChecked(checked))
        menu.addAction(pin_action)

        menu.addSeparator()

        quit_action = QAction("退出", self)
        quit_action.triggered.connect(self._quit)
        menu.addAction(quit_action)

        self.tray_icon.setContextMenu(menu)
        self.tray_icon.activated.connect(self._on_tray_activated)
        self.tray_icon.show()

    def _init_shortcuts(self):
        QShortcut(QKeySequence("Ctrl+Q"), self).activated.connect(self._quit)
        QShortcut(QKeySequence("Ctrl+L"), self).activated.connect(self._clear_text)
        QShortcut(QKeySequence("Ctrl+T"), self).activated.connect(self._toggle_topmost)

    # ---------- 拖动移动 ----------
    def mousePressEvent(self, event):
        if event.button() == Qt.LeftButton:
            if self.title_bar.geometry().contains(event.pos()):
                self.drag_position = event.globalPos() - self.frameGeometry().topLeft()
                event.accept()

    def mouseMoveEvent(self, event):
        if event.buttons() == Qt.LeftButton and self.drag_position is not None:
            self.move(event.globalPos() - self.drag_position)
            event.accept()

    def mouseReleaseEvent(self, event):
        self.drag_position = None

    # ---------- 滚轮透明度（Shift+Wheel） ----------
    def wheelEvent(self, event):
        if event.modifiers() & Qt.ShiftModifier:
            delta = event.angleDelta().y()
            new_val = self.opacity_slider.value() + (1 if delta > 0 else -1)
            self.opacity_slider.setValue(max(30, min(100, new_val)))
            event.accept()
        else:
            super().wheelEvent(event)

    # ---------- 事件处理 ----------
    def _on_opacity_changed(self, value):
        self.setWindowOpacity(value / 100.0)

    def _toggle_topmost(self):
        flags = self.windowFlags()
        if flags & Qt.WindowStaysOnTopHint:
            flags &= ~Qt.WindowStaysOnTopHint
            self.pin_btn.setChecked(False)
            self.setWindowFlags(flags)
            self.show()
        else:
            flags |= Qt.WindowStaysOnTopHint
            self.pin_btn.setChecked(True)
            self.setWindowFlags(flags)
            self.show()

    def _clear_text(self):
        self.text_edit.clear()

    def _update_count(self):
        text = self.text_edit.toPlainText()
        self.count_label.setText(f"字符: {len(text)}")

    def _show_window(self):
        self.show()
        self.raise_()
        self.activateWindow()

    def _on_tray_activated(self, reason):
        if reason == QSystemTrayIcon.Trigger:  # 单击
            if self.isVisible():
                self.hide()
            else:
                self._show_window()

    def _quit(self):
        if self.tray_icon:
            self.tray_icon.hide()
        QApplication.quit()

    def closeEvent(self, event):
        # 拦截关闭，改为隐藏到托盘
        if self.tray_icon and self.tray_icon.isVisible():
            event.ignore()
            self.hide()
            self.tray_icon.showMessage(
                "悬浮文本框",
                "窗口已最小化到托盘，双击图标或右键菜单可恢复",
                QSystemTrayIcon.Information,
                2000,
            )
        else:
            event.accept()


def main():
    app = QApplication(sys.argv)
    app.setQuitOnLastWindowClosed(False)  # 关闭时不退出
    win = FloatTextBox()
    win.show()
    sys.exit(app.exec_())


if __name__ == "__main__":
    main()
