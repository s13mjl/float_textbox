//! 悬浮文本框 (Rust + Win32 API)
//!
//! 仿 B 站弹幕悬浮窗的桌面置顶文本框，使用纯 Win32 API 实现。
//! - 无边框 + 置顶 + 系统托盘
//! - 自定义标题栏：透明度滑块 / 置顶 / 最小化 / 关闭
//! - 多行文本编辑区 + 字符计数
//! - 右下角拖拽缩放、Shift+滚轮调节透明度
//! - 快捷键 Ctrl+Q 退出 / Ctrl+L 清空 / Ctrl+T 切换置顶

#![windows_subsystem = "windows"]

use std::mem;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---- windows-rs 0.58 未导出的常量，按 Win32 SDK 原值定义 ----
const TBM_GETPOS: u32 = 1024; // WM_USER
const TBM_SETPOS: u32 = 1029; // WM_USER + 5
const TBM_SETRANGE: u32 = 1030; // WM_USER + 6
const MK_SHIFT: u16 = 0x0004;
const SS_LEFT: u32 = 0x0000_0000;
const SS_CENTERIMAGE: u32 = 0x0000_0200;

// ---- 主题色 (仿 B 站深灰) ----
const C_BG: u32 = 0x001E1E1E;
const C_BAR: u32 = 0x002B2B2B;
const C_BAR_DARK: u32 = 0x00252525;
const C_TEXT: u32 = 0x00E0E0E0;
const C_TEXT_MUTED: u32 = 0x00888888;
const C_ACCENT: u32 = 0x00ECAE00; // #00aeec -> COLORREF (B,G,R)
const C_BTN_HOVER: u32 = 0x003D3D3D;
const C_BTN_CLOSE: u32 = 0x006B6BFF; // #ff6b6b -> COLORREF

// ---- 控件 ID ----
const ID_TITLE_LABEL: u16 = 101;
const ID_OPACITY_SLIDER: u16 = 102;
const ID_PIN_BTN: u16 = 103;
const ID_MIN_BTN: u16 = 104;
const ID_CLOSE_BTN: u16 = 105;
const ID_TEXT_EDIT: u16 = 106;
const ID_COUNT_LABEL: u16 = 107;
const ID_CLEAR_BTN: u16 = 108;

// ---- 菜单/加速键命令 ID ----
const IDM_SHOW: u16 = 201;
const IDM_HIDE: u16 = 202;
const IDM_TOGGLE_TOP: u16 = 203;
const IDM_CLEAR: u16 = 204;
const IDM_QUIT: u16 = 205;

// ---- 自定义消息 ----
const WM_TRAYICON: u32 = WM_APP + 1;

// ---- 窗口类名 ----
const CLASS_NAME: PCWSTR = w!("FloatTextBoxMainWnd");

// ---- 应用状态 ----
struct AppState {
    hwnd: HWND,
    hinst: HINSTANCE,
    dpi: u32,
    title_label: HWND,
    opacity_slider: HWND,
    pin_btn: HWND,
    min_btn: HWND,
    close_btn: HWND,
    text_edit: HWND,
    count_label: HWND,
    clear_btn: HWND,
    bg_brush: HBRUSH,
    bar_brush: HBRUSH,
    bar_dark_brush: HBRUSH,
    accent_brush: HBRUSH,
    btn_hover_brush: HBRUSH,
    btn_close_brush: HBRUSH,
    text_font: HFONT,
    title_font: HFONT,
    small_font: HFONT,
    symbol_font: HFONT,
    app_icon: HICON,
    tray_icon: HICON,
    topmost: bool,
    tray_added: bool,
    first_minimize: bool,
}

impl AppState {
    fn scale(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
}

// ---- 辅助函数 ----
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fill_wide_array(buf: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let n = wide.len().min(buf.len().saturating_sub(1));
    buf[..n].copy_from_slice(&wide[..n]);
    if n < buf.len() {
        buf[n] = 0;
    }
}

fn set_text(hwnd: HWND, s: &str) {
    let w = to_wide(s);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(w.as_ptr()));
    }
}

unsafe fn state_of(hwnd: HWND) -> *mut AppState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState
}

fn hmenu_id(id: u16) -> HMENU {
    HMENU(id as usize as *mut core::ffi::c_void)
}

unsafe fn send(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    SendMessageW(hwnd, msg, wp, lp)
}

unsafe fn get_text_length(hwnd: HWND) -> i32 {
    GetWindowTextLengthW(hwnd)
}

// ---- 字体创建 ----
fn create_font(height: i32, bold: bool, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            face,
        )
    }
}

// ---- 图标生成：圆角蓝色方块 + 白色 "T" ----
fn create_app_icon(size: i32) -> HICON {
    unsafe {
        let hdc_screen = GetDC(None);
        let hdc_color = CreateCompatibleDC(hdc_screen);
        let hdc_mask = CreateCompatibleDC(hdc_screen);
        let hbm_color = CreateCompatibleBitmap(hdc_screen, size, size);
        let hbm_mask = CreateBitmap(size, size, 1, 1, None);

        let full_rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };

        // 颜色位图：蓝底白 "T"
        let old_color = SelectObject(hdc_color, hbm_color);
        let blue_brush = CreateSolidBrush(COLORREF(C_ACCENT));
        FillRect(hdc_color, &full_rect, blue_brush);
        DeleteObject(blue_brush);

        let font = CreateFontW(
            -(size * 3 / 4),
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            DEFAULT_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Microsoft YaHei UI"),
        );
        let old_font = SelectObject(hdc_color, font);
        SetBkMode(hdc_color, TRANSPARENT);
        SetTextColor(hdc_color, COLORREF(0x00FFFFFF));
        let mut r = full_rect;
        let mut t_buf: Vec<u16> = "T".encode_utf16().collect();
        let _ = DrawTextW(hdc_color, &mut t_buf, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(hdc_color, old_font);
        DeleteObject(font);
        SelectObject(hdc_color, old_color);
        DeleteDC(hdc_color);

        // 蒙版位图：白底(透明) + 黑色圆角矩形(不透明)
        let old_mask = SelectObject(hdc_mask, hbm_mask);
        let white_brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        FillRect(hdc_mask, &full_rect, white_brush);
        DeleteObject(white_brush);
        let black_brush = CreateSolidBrush(COLORREF(0));
        let black_pen = CreatePen(PS_SOLID, 0, COLORREF(0));
        let old_pen = SelectObject(hdc_mask, black_pen);
        let old_br = SelectObject(hdc_mask, black_brush);
        let _ = RoundRect(hdc_mask, 1, 1, size - 1, size - 1, size / 3, size / 3);
        SelectObject(hdc_mask, old_br);
        SelectObject(hdc_mask, old_pen);
        DeleteObject(black_brush);
        DeleteObject(black_pen);
        SelectObject(hdc_mask, old_mask);
        DeleteDC(hdc_mask);

        ReleaseDC(None, hdc_screen);

        let ii = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&ii).unwrap_or_default();
        DeleteObject(hbm_color);
        DeleteObject(hbm_mask);
        hicon
    }
}

// ---- 圆角区域 ----
fn apply_rounded_region(hwnd: HWND, w: i32, h: i32, radius: i32) {
    unsafe {
        if w <= 0 || h <= 0 {
            return;
        }
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius);
        let _ = SetWindowRgn(hwnd, rgn, true);
    }
}

// ---- 控件布局 ----
// 优先级（窗口过小时按顺序隐藏）：
//   1. pin/min/close 三个按钮 — 始终显示
//   2. 透明度滑块       — 空间不足时隐藏
//   3. 标题文字         — 最先隐藏
fn layout_controls(state: &AppState) {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetClientRect(state.hwnd, &mut rect);
        let w = rect.right;
        let h = rect.bottom;

        let title_h = state.scale(28);
        let bottom_h = state.scale(20);
        let btn = state.scale(22);
        let slider_w = state.scale(50);
        let slider_h = state.scale(16);
        let margin = state.scale(8);
        let left_pad = state.scale(12);
        let gap = state.scale(4);
        let cy = title_h / 2;

        // —— 右对齐：close / min / pin（始终显示）——
        let mut x = w - margin;
        let _ = MoveWindow(state.close_btn, x - btn, cy - btn / 2, btn, btn, true);
        x -= btn + gap;
        let _ = MoveWindow(state.min_btn, x - btn, cy - btn / 2, btn, btn, true);
        x -= btn + gap;
        let _ = MoveWindow(state.pin_btn, x - btn, cy - btn / 2, btn, btn, true);
        x -= btn + gap;

        // —— 次优先：透明度滑块 ——
        // 需剩余空间 >= slider_w + 左侧留白
        if x - slider_w >= left_pad + state.scale(8) {
            let _ = MoveWindow(
                state.opacity_slider,
                x - slider_w,
                cy - slider_h / 2,
                slider_w,
                slider_h,
                true,
            );
            let _ = ShowWindow(state.opacity_slider, SW_SHOWNOACTIVATE);
            x -= slider_w + gap;
        } else {
            let _ = ShowWindow(state.opacity_slider, SW_HIDE);
        }

        // —— 最低优先：标题文字 ——
        // 剩余宽度需 > 80 scaled px，否则隐藏避免与按钮重叠
        let title_min = state.scale(80);
        if x - left_pad > title_min {
            let title_w = x - left_pad - gap;
            let _ = MoveWindow(state.title_label, left_pad, 0, title_w, title_h, true);
            let _ = ShowWindow(state.title_label, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(state.title_label, SW_HIDE);
        }

        // —— 文本编辑区 ——
        let _ = MoveWindow(
            state.text_edit,
            0,
            title_h,
            w,
            (h - title_h - bottom_h).max(1),
            true,
        );

        // —— 底栏：清空按钮优先(始终显示)，字符计数次之(空间不足时隐藏) ——
        let by = h - bottom_h;
        let bcy = by + bottom_h / 2;
        let clear_w = state.scale(40);
        let grip_w = state.scale(14);

        // 清空按钮：靠右紧邻 grip
        let clear_x = w - margin - grip_w - gap - clear_w;
        let _ = MoveWindow(state.clear_btn, clear_x, bcy - state.scale(9), clear_w, state.scale(18), true);
        let _ = ShowWindow(state.clear_btn, SW_SHOWNOACTIVATE);

        // 字符计数：剩余空间不足时隐藏
        let count_min = state.scale(40);
        if clear_x - left_pad > count_min + gap {
            let count_w = (clear_x - gap - left_pad).min(state.scale(100));
            let _ = MoveWindow(
                state.count_label,
                left_pad,
                bcy - state.scale(8),
                count_w,
                state.scale(16),
                true,
            );
            let _ = ShowWindow(state.count_label, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(state.count_label, SW_HIDE);
        }
    }
}

// ---- 透明度 ----
fn apply_opacity(state: &AppState) {
    unsafe {
        let pos = send(
            state.opacity_slider,
            TBM_GETPOS,
            WPARAM(0),
            LPARAM(0),
        )
        .0 as i32;
        let alpha = ((pos.max(0).min(100)) * 255 / 100).max(1) as u8;
        let _ = SetLayeredWindowAttributes(state.hwnd, COLORREF(0), alpha, LWA_ALPHA);
    }
}

fn set_topmost(state: &mut AppState, topmost: bool) {
    state.topmost = topmost;
    unsafe {
        let insert_after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let _ = SetWindowPos(
            state.hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(state.pin_btn, None, true);
    }
}

fn toggle_topmost(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let state = &mut *p;
        set_topmost(state, !state.topmost);
    }
}

fn clear_text(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let state = &*p;
        set_text(state.text_edit, "");
    }
}

fn update_count(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let state = &*p;
        let len = get_text_length(state.text_edit);
        set_text(state.count_label, &format!("字符: {}", len));
    }
}

fn show_window(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn hide_to_tray(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let state = &mut *p;
        let _ = ShowWindow(hwnd, SW_HIDE);
        if state.first_minimize {
            state.first_minimize = false;
            show_tray_balloon(state, "悬浮文本框", "窗口已最小化到托盘，单击图标可恢复");
        }
    }
}

fn quit_app(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

// ---- 托盘 ----
fn build_nid(state: &AppState) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = state.hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = state.tray_icon;
    fill_wide_array(&mut nid.szTip, "悬浮文本框");
    nid
}

fn add_tray_icon(state: &AppState) {
    unsafe {
        let nid = build_nid(state);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray_icon(state: &AppState) {
    unsafe {
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = state.hwnd;
        nid.uID = 1;
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn show_tray_balloon(state: &AppState, title: &str, info: &str) {
    unsafe {
        let mut nid = build_nid(state);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        fill_wide_array(&mut nid.szInfoTitle, title);
        fill_wide_array(&mut nid.szInfo, info);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let state = &*p;
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let _ = AppendMenuW(menu, MF_STRING, IDM_SHOW as usize, w!("显示"));
        let _ = AppendMenuW(menu, MF_STRING, IDM_HIDE as usize, w!("隐藏"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
        let top_flag = if state.topmost { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, top_flag, IDM_TOGGLE_TOP as usize, w!("切换置顶"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
        let _ = AppendMenuW(menu, MF_STRING, IDM_QUIT as usize, w!("退出"));

        let mut cur = POINT::default();
        let _ = GetCursorPos(&mut cur);
        // 必须前置窗口，否则点击菜单外无法关闭
        let _ = SetForegroundWindow(hwnd);
        // 不使用 TPM_RETURNCMD：选中项由系统自动派发 WM_COMMAND 到 hwnd
        let _ = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            cur.x,
            cur.y,
            0,
            hwnd,
            None,
        );
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

// ---- 文本编辑区子类化：Shift+滚轮调节透明度 ----
unsafe extern "system" fn text_edit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MOUSEWHEEL {
        let keys = (wp.0 as u32 & 0xFFFF) as u16;
        if keys & (MK_SHIFT as u16) != 0 {
            let parent = GetParent(hwnd).unwrap_or_default();
            let p = state_of(parent);
            if !p.is_null() {
                let state = &*p;
                let delta = ((wp.0 as u32 >> 16) & 0xFFFF) as i16;
                let cur = send(state.opacity_slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32;
                let new_val = (cur + if delta > 0 { 1 } else { -1 }).max(30).min(100);
                let _ = send(
                    state.opacity_slider,
                    TBM_SETPOS,
                    WPARAM(1),
                    LPARAM(new_val as isize),
                );
                apply_opacity(state);
            }
            return LRESULT(0);
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

// ---- 窗口过程 ----
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCALCSIZE => {
            // 移除 WS_THICKFRAME 的可视边框，使窗口保持无边框
            if wp.0 != 0 {
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_CREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            let hinst = cs.hInstance;

            let dpi = GetDpiForWindow(hwnd).max(96);

            // 画刷
            let bg_brush = CreateSolidBrush(COLORREF(C_BG));
            let bar_brush = CreateSolidBrush(COLORREF(C_BAR));
            let bar_dark_brush = CreateSolidBrush(COLORREF(C_BAR_DARK));
            let accent_brush = CreateSolidBrush(COLORREF(C_ACCENT));
            let btn_hover_brush = CreateSolidBrush(COLORREF(C_BTN_HOVER));
            let btn_close_brush = CreateSolidBrush(COLORREF(C_BTN_CLOSE));

            // 字体
            let text_font = create_font(-13 * dpi as i32 / 96, false, w!("Microsoft YaHei UI"));
            let title_font = create_font(-12 * dpi as i32 / 96, true, w!("Microsoft YaHei UI"));
            let small_font = create_font(-10 * dpi as i32 / 96, false, w!("Microsoft YaHei UI"));
            let symbol_font = create_font(-11 * dpi as i32 / 96, true, w!("Microsoft YaHei UI"));

            let state_box = Box::new(AppState {
                hwnd,
                hinst,
                dpi,
                title_label: HWND::default(),
                opacity_slider: HWND::default(),
                pin_btn: HWND::default(),
                min_btn: HWND::default(),
                close_btn: HWND::default(),
                text_edit: HWND::default(),
                count_label: HWND::default(),
                clear_btn: HWND::default(),
                bg_brush,
                bar_brush,
                bar_dark_brush,
                accent_brush,
                btn_hover_brush,
                btn_close_brush,
                text_font,
                title_font,
                small_font,
                symbol_font,
                app_icon: HICON::default(),
                tray_icon: HICON::default(),
                topmost: true,
                tray_added: false,
                first_minimize: true,
            });
            let state_ptr = Box::into_raw(state_box);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            let state = &mut *state_ptr;

            // 创建子控件
            let base = (WS_CHILD | WS_VISIBLE).0;
            let static_style = WINDOW_STYLE(base | SS_CENTERIMAGE);
            let title_label = CreateWindowExW(
                WS_EX_TRANSPARENT,
                w!("STATIC"),
                w!("悬浮文本框"),
                static_style,
                0,
                0,
                0,
                0,
                hwnd,
                hmenu_id(ID_TITLE_LABEL),
                hinst,
                None,
            )
            .unwrap_or_default();
            state.title_label = title_label;
            let _ = send(title_label, WM_SETFONT, WPARAM(text_font.0 as usize), LPARAM(1));

            let slider_style = WINDOW_STYLE(base | TBS_NOTICKS);
            let opacity_slider = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                TRACKBAR_CLASSW,
                w!(""),
                slider_style,
                0,
                0,
                0,
                0,
                hwnd,
                hmenu_id(ID_OPACITY_SLIDER),
                hinst,
                None,
            )
            .unwrap_or_default();
            state.opacity_slider = opacity_slider;
            let _ = send(opacity_slider, TBM_SETRANGE, WPARAM(1), LPARAM(MAKELONG(30, 100) as isize));
            let _ = send(opacity_slider, TBM_SETPOS, WPARAM(1), LPARAM(95));

            let btn_style = WINDOW_STYLE(base | BS_OWNERDRAW as u32);
            let make_btn = |id: u16| -> HWND {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!(""),
                    btn_style,
                    0,
                    0,
                    0,
                    0,
                    hwnd,
                    hmenu_id(id),
                    hinst,
                    None,
                )
                .unwrap_or_default()
            };
            state.pin_btn = make_btn(ID_PIN_BTN);
            state.min_btn = make_btn(ID_MIN_BTN);
            state.close_btn = make_btn(ID_CLOSE_BTN);
            state.clear_btn = make_btn(ID_CLEAR_BTN);

            let edit_style = WINDOW_STYLE(
                base
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | ES_NOHIDESEL as u32,
            );
            let text_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                0,
                0,
                hwnd,
                hmenu_id(ID_TEXT_EDIT),
                hinst,
                None,
            )
            .unwrap_or_default();
            state.text_edit = text_edit;
            let _ = send(text_edit, WM_SETFONT, WPARAM(text_font.0 as usize), LPARAM(1));

            state.count_label = CreateWindowExW(
                WS_EX_TRANSPARENT,
                w!("STATIC"),
                w!("字符: 0"),
                static_style,
                0,
                0,
                0,
                0,
                hwnd,
                hmenu_id(ID_COUNT_LABEL),
                hinst,
                None,
            )
            .unwrap_or_default();
            let _ = send(
                state.count_label,
                WM_SETFONT,
                WPARAM(small_font.0 as usize),
                LPARAM(1),
            );

            // 子类化文本框以拦截 Shift+滚轮
            let _ = SetWindowSubclass(text_edit, Some(text_edit_subclass_proc), 1, 0);

            // 图标
            let app_icon = create_app_icon(64 * dpi as i32 / 96);
            let tray_icon = create_app_icon(32 * dpi as i32 / 96);
            state.app_icon = app_icon;
            state.tray_icon = tray_icon;
            let _ = send(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(app_icon.0 as isize));
            let _ = send(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(tray_icon.0 as isize));

            // 托盘
            add_tray_icon(state);
            state.tray_added = true;

            // 初始尺寸 (居中)
            let sw = GetSystemMetricsForDpi(SM_CXSCREEN, dpi);
            let sh = GetSystemMetricsForDpi(SM_CYSCREEN, dpi);
            let init_w = 280 * dpi as i32 / 96;
            let init_h = 200 * dpi as i32 / 96;
            let x = (sw - init_w) / 2;
            let y = (sh - init_h) / 2;
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                init_w,
                init_h,
                SWP_NOACTIVATE,
            );
            apply_rounded_region(hwnd, init_w, init_h, 10 * dpi as i32 / 96);
            apply_opacity(state);

            LRESULT(0)
        }

        WM_SIZE => {
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &mut *p;
                if !state.text_edit.is_invalid() {
                    layout_controls(state);
                    let w = LOWORD(lp.0 as u32) as i32;
                    let h = HIWORD(lp.0 as u32) as i32;
                    apply_rounded_region(hwnd, w, h, state.scale(10));
                }
            }
            LRESULT(0)
        }

        WM_GETMINMAXINFO => {
            let dpi = {
                let p = state_of(hwnd);
                if p.is_null() {
                    GetDpiForSystem()
                } else {
                    (*p).dpi
                }
            };
            let scale = |v: i32| v * dpi as i32 / 96;
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT {
                x: scale(160),
                y: scale(100),
            };
            LRESULT(0)
        }

        WM_NCHITTEST => {
            let screen_pt = POINT {
                x: GET_X_LPARAM(lp) as i32,
                y: GET_Y_LPARAM(lp) as i32,
            };
            let mut client_pt = screen_pt;
            let _ = ScreenToClient(hwnd, &mut client_pt);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);

            let p = state_of(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let state = &*p;

            let grip = state.scale(14);
            // 右下角缩放手柄
            if client_pt.x >= rect.right - grip && client_pt.y >= rect.bottom - grip {
                return LRESULT(HTBOTTOMRIGHT as isize);
            }

            // 标题栏区域：除交互控件外均可拖动
            // (STATIC 标签已加 WS_EX_TRANSPARENT，CWP_SKIPTRANSPARENT 会跳过)
            let title_h = state.scale(28);
            if client_pt.y < title_h {
                let child = ChildWindowFromPointEx(
                    hwnd,
                    client_pt,
                    CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT,
                );
                let interactive = child == state.close_btn
                    || child == state.min_btn
                    || child == state.pin_btn
                    || child == state.opacity_slider;
                if !interactive {
                    return LRESULT(HTCAPTION as isize);
                }
            }

            LRESULT(HTCLIENT as isize)
        }

        WM_SETCURSOR => {
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &*p;
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let mut client_pt = cur;
                let _ = ScreenToClient(hwnd, &mut client_pt);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let grip = state.scale(14);
                if client_pt.x >= rect.right - grip && client_pt.y >= rect.bottom - grip {
                    let _ = SetCursor(LoadCursorW(None, IDC_SIZENESW).unwrap_or_default());
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_ERASEBKGND => {
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &*p;
                let hdc = HDC(wp.0 as *mut _);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                FillRect(hdc, &rect, state.bg_brush);
            }
            LRESULT(1)
        }

        WM_PAINT => {
            let p = state_of(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let state = &*p;
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.is_invalid() {
                return LRESULT(0);
            }

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right;
            let h = rect.bottom;
            let title_h = state.scale(28);
            let bottom_h = state.scale(20);

            // 标题栏背景
            let title_rect = RECT { left: 0, top: 0, right: w, bottom: title_h };
            FillRect(hdc, &title_rect, state.bar_brush);
            // 底栏背景
            let bottom_rect = RECT { left: 0, top: h - bottom_h, right: w, bottom: h };
            FillRect(hdc, &bottom_rect, state.bar_brush);
            // 分割线
            let dark_pen = CreatePen(PS_SOLID, 1, COLORREF(C_BAR_DARK));
            let old_pen = SelectObject(hdc, dark_pen);
            MoveToEx(hdc, 0, title_h, None);
            LineTo(hdc, w, title_h);
            MoveToEx(hdc, 0, h - bottom_h, None);
            LineTo(hdc, w, h - bottom_h);
            // 外边框
            let _ = SelectObject(hdc, dark_pen);
            let _ = MoveToEx(hdc, 0, 0, None);
            LineTo(hdc, w, 0);
            LineTo(hdc, w, h);
            LineTo(hdc, 0, h);
            LineTo(hdc, 0, 0);
            SelectObject(hdc, old_pen);
            DeleteObject(dark_pen);

            // 右下角缩放手柄 (两条对角小线)
            let grip = state.scale(14);
            let grip_pen = CreatePen(PS_SOLID, 1, COLORREF(C_TEXT_MUTED));
            let old_gpen = SelectObject(hdc, grip_pen);
            let gx = w - grip + state.scale(2);
            let gy = h - state.scale(2);
            MoveToEx(hdc, gx, gy, None);
            LineTo(hdc, w - state.scale(2), gy - (gy - (h - grip)));
            // 简化：画一条对角线
            MoveToEx(hdc, w - grip + state.scale(2), h - state.scale(3), None);
            LineTo(hdc, w - state.scale(3), h - grip + state.scale(2));
            MoveToEx(hdc, w - grip + state.scale(6), h - state.scale(3), None);
            LineTo(hdc, w - state.scale(3), h - grip + state.scale(6));
            SelectObject(hdc, old_gpen);
            DeleteObject(grip_pen);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        WM_DRAWITEM => {
            let dis = &*(lp.0 as *const DRAWITEMSTRUCT);
            let id = dis.CtlID as u16;
            let p = state_of(hwnd);
            if p.is_null() {
                return LRESULT(0);
            }
            let state = &*p;
            let hdc = dis.hDC;
            let r = dis.rcItem;
            let selected = dis.itemState.0 & ODS_SELECTED.0 != 0;

            let (bg, text_color, label, font) = match id {
                ID_PIN_BTN => (
                    if selected { state.btn_hover_brush } else { state.bar_brush },
                    if state.topmost { COLORREF(C_ACCENT) } else { COLORREF(C_TEXT) },
                    "顶",
                    state.symbol_font,
                ),
                ID_MIN_BTN => (
                    if selected { state.btn_hover_brush } else { state.bar_brush },
                    COLORREF(C_TEXT),
                    "—",
                    state.symbol_font,
                ),
                ID_CLOSE_BTN => (
                    if selected { state.btn_close_brush } else { state.bar_brush },
                    if selected { COLORREF(0x00FFFFFF) } else { COLORREF(C_TEXT) },
                    "✕",
                    state.symbol_font,
                ),
                ID_CLEAR_BTN => (
                    if selected { state.bar_dark_brush } else { state.btn_hover_brush },
                    COLORREF(C_TEXT),
                    "清空",
                    state.small_font,
                ),
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            };

            FillRect(hdc, &r, bg);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, text_color);
            let old_font = SelectObject(hdc, font);
            let mut rr = r;
            let mut buf: Vec<u16> = label.encode_utf16().collect();
            let _ = DrawTextW(hdc, &mut buf, &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, old_font);
            LRESULT(1)
        }

        WM_CTLCOLORSTATIC | WM_CTLCOLORDLG => {
            let ctl = HWND(lp.0 as *mut _);
            let p = state_of(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let state = &*p;
            let hdc = HDC(wp.0 as *mut _);
            if ctl == state.count_label || ctl == state.title_label {
                SetTextColor(hdc, if ctl == state.title_label {
                    COLORREF(C_TEXT)
                } else {
                    COLORREF(C_TEXT_MUTED)
                });
                SetBkMode(hdc, TRANSPARENT);
                return LRESULT(state.bar_brush.0 as isize);
            }
            // 滑块背景：与标题栏一致
            SetBkMode(hdc, TRANSPARENT);
            LRESULT(state.bar_brush.0 as isize)
        }

        WM_CTLCOLOREDIT => {
            let p = state_of(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let state = &*p;
            let hdc = HDC(wp.0 as *mut _);
            SetTextColor(hdc, COLORREF(C_TEXT));
            SetBkColor(hdc, COLORREF(C_BG));
            LRESULT(state.bg_brush.0 as isize)
        }

        WM_HSCROLL => {
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &*p;
                if HWND(lp.0 as *mut _) == state.opacity_slider {
                    apply_opacity(state);
                }
            }
            LRESULT(0)
        }

        WM_COMMAND => {
            let id = LOWORD(wp.0 as u32) as u16;
            let notif = HIWORD(wp.0 as u32) as u16;
            match id {
                ID_PIN_BTN if notif == BN_CLICKED as u16 => toggle_topmost(hwnd),
                ID_MIN_BTN if notif == BN_CLICKED as u16 => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                ID_CLOSE_BTN if notif == BN_CLICKED as u16 => hide_to_tray(hwnd),
                ID_CLEAR_BTN if notif == BN_CLICKED as u16 => clear_text(hwnd),
                ID_TEXT_EDIT if notif == EN_CHANGE as u16 => update_count(hwnd),
                IDM_TOGGLE_TOP => toggle_topmost(hwnd),
                IDM_CLEAR => clear_text(hwnd),
                IDM_SHOW => show_window(hwnd),
                IDM_HIDE => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                IDM_QUIT => quit_app(hwnd),
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            LRESULT(0)
        }

        WM_TRAYICON => {
            let mouse_msg = (lp.0 as u32) as u32;
            if mouse_msg == WM_LBUTTONUP {
                if IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                } else {
                    show_window(hwnd);
                }
            } else if mouse_msg == WM_RBUTTONUP {
                show_tray_menu(hwnd);
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            // 拦截关闭，改为最小化到托盘
            let p = state_of(hwnd);
            if !p.is_null() && (*p).tray_added {
                hide_to_tray(hwnd);
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_DESTROY => {
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &*p;
                if state.tray_added {
                    remove_tray_icon(state);
                }
                // 回收 Box
                let _ = Box::from_raw(p);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---- 工具宏 (LOWORD/HIWORD/MAKELONG) ----
fn LOWORD(l: u32) -> u16 {
    (l & 0xFFFF) as u16
}
fn HIWORD(l: u32) -> u16 {
    ((l >> 16) & 0xFFFF) as u16
}
fn MAKELONG(a: u16, b: u16) -> u32 {
    (a as u32) | ((b as u32) << 16)
}

// GET_X_LPARAM / GET_Y_LPARAM
fn GET_X_LPARAM(lp: LPARAM) -> i16 {
    (lp.0 as u32 as u16) as i16
}
fn GET_Y_LPARAM(lp: LPARAM) -> i16 {
    ((lp.0 as u32 >> 16) as u16) as i16
}

fn main() {
    // DPI 感知 (清单已声明 PerMonitor V2，此处兜底)
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let hinst = unsafe { GetModuleHandleW(None).unwrap_or_default() };

    // 类背景画刷 (应用生命周期内常驻)
    let class_bg = unsafe { CreateSolidBrush(COLORREF(C_BG)) };

    // 注册窗口类
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        hbrBackground: class_bg,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        return;
    }

    // 初始化 Common Controls (确保 trackbar / 子类化可用)
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }

    // 创建主窗口 (无边框 + 分层；缩放由 WM_NCHITTEST 自定义处理)
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_LAYERED,
            CLASS_NAME,
            w!("悬浮文本框"),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            280,
            200,
            None,
            None,
            hinst,
            None,
        )
    };
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(_) => return,
    };

    // 显示窗口
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }

    // 加速键: Ctrl+Q 退出 / Ctrl+L 清空 / Ctrl+T 切换置顶
    let haccel = unsafe {
        let accels = [
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL,
                key: b'Q' as u16,
                cmd: IDM_QUIT,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL,
                key: b'L' as u16,
                cmd: IDM_CLEAR,
            },
            ACCEL {
                fVirt: FVIRTKEY | FCONTROL,
                key: b'T' as u16,
                cmd: IDM_TOGGLE_TOP,
            },
        ];
        CreateAcceleratorTableW(&accels).unwrap_or_default()
    };

    // 消息循环
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if TranslateAcceleratorW(hwnd, haccel, &msg) == 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
