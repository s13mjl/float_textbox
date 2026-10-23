//! 悬浮文本框 (Rust + Win32 API)
//!
//! 仿 B 站弹幕悬浮窗的桌面置顶文本框，使用纯 Win32 API 实现。
//! - 无边框 + 置顶 + 系统托盘
//! - 自定义标题栏：透明度滑块 / 置顶 / 最小化 / 关闭
//! - 多行文本编辑区 + 字符计数
//! - 右下角拖拽缩放、Shift+滚轮调节透明度
//! - 快捷键 Ctrl+Q 退出 / Ctrl+L 清空 / Ctrl+Z 撤销 / Ctrl+Shift+Z 重做 / Ctrl+T 切换置顶

#![windows_subsystem = "windows"]

use std::collections::VecDeque;
use std::mem;
use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---- windows-rs 0.58 未导出的常量，按 Win32 SDK 原值定义 ----
const TBM_GETPOS: u32 = 1024; // WM_USER
const TBM_SETPOS: u32 = 1029; // WM_USER + 5
const TBM_SETRANGE: u32 = 1030; // WM_USER + 6
const MK_SHIFT: u16 = 0x0004;
const MK_LBUTTON: u16 = 0x0001;
const SS_CENTERIMAGE: u32 = 0x0000_0200;
const SS_SINGLELINE: u32 = 0x0000_0020;

// ---- 主题色 (仿 B 站深灰) ----
const C_BG: u32 = 0x001E1E1E;
const C_BAR: u32 = 0x002B2B2B;
const C_BAR_DARK: u32 = 0x00252525;
const C_TEXT: u32 = 0x00E0E0E0;
const C_TEXT_MUTED: u32 = 0x00888888;
const C_ACCENT: u32 = 0x00ECAE00; // #00aeec -> COLORREF (B,G,R)
const C_BTN_HOVER: u32 = 0x003D3D3D;
const C_BTN_CLOSE: u32 = 0x006B6BFF; // #ff6b6b -> COLORREF

// ---- 自绘滚动条配色 ----
// 轨道与编辑区背景一致，滑块比背景略亮，避免系统主题滚动条在深色界面里
// 出现刺眼的白色竖条。
const C_SCROLL_TRACK: u32 = 0x001E1E1E;
const C_SCROLL_THUMB: u32 = 0x004A4A4A;
const C_SCROLL_THUMB_HOVER: u32 = 0x00606060;
// 箭头颜色与底栏计数文字一致，存在感弱于滑块，不会破坏深色基调。
const C_SCROLL_ARROW: u32 = 0x00888888;
// 箭头不可用时（已经到顶/到底）使用的更暗的颜色，和系统滚动条一致。
const C_SCROLL_ARROW_DIM: u32 = 0x00505050;
// 滑块最小高度（逻辑像素）。文本很长时按比例算出的滑块可能只剩几个像素，
// 那时反而点不中；这里给一个下限。
const SCROLL_THUMB_MIN_H: i32 = 14;
// 单个箭头按钮的高度（逻辑像素），与系统滚动条一致。
const SCROLL_ARROW_SIZE: i32 = 17;
// 箭头按钮按下后的自动重复：首次延迟和连续重复间隔（毫秒）。
const SCROLL_REPEAT_FIRST_DELAY: u32 = 400;
const SCROLL_REPEAT_INTERVAL: u32 = 50;
// 自绘滚动条子窗口使用的定时器 ID。子窗口的定时器空间与主窗口独立，
// 不会与主窗口的三个定时器冲突。
const SCROLL_REPEAT_TIMER: usize = 1;

// ---- 控件 ID ----
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
const WM_SHOW_AND_FOCUS: u32 = WM_APP + 2;
// NOTIFYICON_VERSION_4 的键盘选择通知。windows-rs 0.58 只导出了 NIN_SELECT，
// 这里按 SDK 定义补上 NIN_KEYSELECT = NIN_SELECT | NINF_KEY。
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x0001;

// ---- 定时器与文本限制 ----
const TIMER_UPDATE_COUNT: usize = 1;
const TIMER_CLEAR_NOTICE: usize = 2;
// 缩放拖动期间的兜底刷新定时器。
const TIMER_RESIZE: usize = 3;
const TEXT_LIMIT: usize = 1_000_000;

// ---- 撤销/重做 ----
// 每条历史只保存两次状态之间的差异片段，内存与改动量成正比。
// 这两个上限约束最坏情况下的历史内存占用。
const UNDO_MAX_STEPS: usize = 500;
const UNDO_MAX_CHARS: usize = 8_000_000;

// ---- 窗口类名 ----
const CLASS_NAME: PCWSTR = w!("FloatTextBoxMainWnd");
// 编辑区右侧的自绘垂直滚动条。标准 EDIT 的滚动条不是子窗口（由 EDIT
// 自己绘制），无法通过子类化替换，因此这里使用完全自有的窗口。
const SCROLLBAR_CLASS_NAME: PCWSTR = w!("FloatTextBoxScrollBar");

// Explorer 重启后广播的消息；必须在创建窗口前注册。
static TASKBAR_CREATED_MSG: OnceLock<u32> = OnceLock::new();

// ---- 应用状态 ----
struct AppState {
    hwnd: HWND,
    dpi: u32,
    opacity_slider: HWND,
    pin_btn: HWND,
    min_btn: HWND,
    close_btn: HWND,
    text_edit: HWND,
    // 编辑区右侧的自绘滚动条子窗口。它由本程序创建，不是 EDIT 的系统
    // 滚动条（标准 EDIT 根本没有该子窗口）。
    scroll_bar: HWND,
    // 自绘滚动条的子类化过程是否已经安装成功。SetWindowSubclass 可能
    // 失败（资源不足、comctl32 异常），失败时保留句柄供后续重试。
    scroll_subclassed: bool,
    // 自绘滚动条的状态和画刷，随编辑框一起创建和释放。
    scroll_paint: Box<ScrollBarPaint>,
    count_label: HWND,
    clear_btn: HWND,
    bg_brush: HBRUSH,
    bar_brush: HBRUSH,
    bar_dark_brush: HBRUSH,
    btn_hover_brush: HBRUSH,
    btn_close_brush: HBRUSH,
    dark_pen: HPEN,
    grip_pen: HPEN,
    text_font: HFONT,
    title_font: HFONT,
    small_font: HFONT,
    symbol_font: HFONT,
    app_icon: HICON,
    tray_icon: HICON,
    topmost: bool,
    tray_added: bool,
    first_hide: bool,
    // 鼠标当前悬停的标题栏/底栏按钮；自绘按钮需要它来绘制 hover 态。
    hovered_btn: HWND,
    // 已注册 TME_LEAVE 的按钮。WM_MOUSEMOVE 会持续到达，不能每次移动都
    // 重新注册；WM_MOUSELEAVE 或按钮隐藏后这里必须清空，否则下一次悬停
    // 会失去离开通知。
    hover_tracking: HWND,
    // 清空前的正文和选区。保留原始 UTF-16 码元，避免 from_utf16_lossy
    // 把孤立代理项替换掉后撤销清空时无法逐字还原；撤销时一并恢复光标和
    // 选区，避免长文本清空后只能回到末尾重新定位。
    cleared_snapshot: Option<ClearedSnapshot>,
    // 文本编辑的多级撤销/重做历史。标准 EDIT 只有单级原生撤销，
    // 不能提供多级撤销和重做，因此由应用自己记录。
    edit_history: EditHistory,
    // 字符计数专用的复用缓冲。长文本每次变化都会重新读取全文；若每次
    // 都新建 Vec，接近上限时会反复分配并复制约 2 MB。
    count_scratch: Vec<u16>,
    // 底栏当前是否显示“Ctrl+Z 撤销”。延迟到达的计数定时器必须尊重
    // 这个状态，否则会在提示出现后把它静默覆盖。
    clear_notice_active: bool,
    // 高精度滚轮（精密触控板）会以 1~119 的 delta 连续到达，累计到一档再调节。
    // 用“向外截断”的除法（而不是 Rust 的 % 取模）保持余量符号与滚动方向
    // 一致：Rust 的 % 保留被除数符号，负 delta 累加后会得到负余量，若直接
    // 用 % 截断，反向滚动会先被负余量抵消一小段，手感发涩。
    wheel_remainder: i32,
    // 编辑框上普通滚轮（滚动正文）的高精度余量，语义与 wheel_remainder
    // 相同，但两者互不干扰：Shift+滚轮调透明度，普通滚轮滚正文。
    text_wheel_remainder: i32,
    region_key: Option<(i32, i32, i32)>,
    // 最小化期间显示器布局可能变化；恢复时先夹取一次工作区，避免窗口
    // 回到已经拔掉的显示器上。
    restore_visibility_fix: bool,
    // DPI 切换时被替换下来的图标句柄。窗口消息只表示“消息已投递”，
    // 不能保证不再被引用；延迟到窗口销毁时统一释放，避免悬空句柄。
    retired_icons: Vec<HICON>,
    // 手动缩放状态。窗口不再使用 WS_THICKFRAME，右下角手柄的拖动由
    // 这里自己处理：记录起点矩形和起点鼠标位置，移动时按差值调整尺寸。
    resizing: bool,
    resize_start_mouse: POINT,
    resize_start_rect: RECT,
    last_alpha: u8,
    back_buffer: Option<BackBuffer>,
}

struct ClearedSnapshot {
    text: Vec<u16>,
    selection_start: usize,
    selection_end: usize,
}

// 一次文本修改。pos 是 UTF-16 码元偏移；removed 是修改前该位置的内容，
// inserted 是修改后的内容。撤销时用 removed 替换 inserted，重做反之。
struct EditChange {
    pos: usize,
    removed: Vec<u16>,
    inserted: Vec<u16>,
}

impl EditChange {
    fn chars(&self) -> usize {
        self.removed.len() + self.inserted.len()
    }
}

struct EditHistory {
    undo: VecDeque<EditChange>,
    redo: Vec<EditChange>,
    // 与当前正文一致的最新状态，用来计算下一次修改的差异。
    last_text: Vec<u16>,
    // 清空、恢复清空以及撤销/重做自身都会改文本；这些程序化修改不能
    // 再记录成新历史。
    suppress: bool,
    // undo + redo 中保存的差异总字符数。
    stored_chars: usize,
}

impl EditHistory {
    fn new() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            last_text: Vec::new(),
            suppress: false,
            stored_chars: 0,
        }
    }

    // 丢弃所有历史，并把基准文本同步为给定内容。
    fn reset(&mut self, text: Vec<u16>) {
        self.undo.clear();
        self.redo.clear();
        self.last_text = text;
        self.stored_chars = 0;
        self.suppress = false;
    }

    fn push(&mut self, change: EditChange) {
        // 产生新修改后，之前撤销出来的重做分支失效。
        for step in self.redo.drain(..) {
            self.stored_chars = self.stored_chars.saturating_sub(step.chars());
        }
        self.stored_chars = self.stored_chars.saturating_add(change.chars());
        self.undo.push_back(change);
        while self.undo.len() > UNDO_MAX_STEPS
            || (self.stored_chars > UNDO_MAX_CHARS && self.undo.len() > 1)
        {
            let Some(old) = self.undo.pop_front() else {
                break;
            };
            self.stored_chars = self.stored_chars.saturating_sub(old.chars());
        }
    }
}

// 复用的离屏缓冲，避免每次 WM_PAINT 都创建/销毁兼容位图。
struct BackBuffer {
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    width: i32,
    height: i32,
}

impl BackBuffer {
    unsafe fn release(&mut self) {
        if !self.mem_dc.is_invalid() && !self.old_bitmap.is_invalid() {
            SelectObject(self.mem_dc, self.old_bitmap);
        }
        if !self.bitmap.is_invalid() {
            let _ = DeleteObject(self.bitmap);
        }
        if !self.mem_dc.is_invalid() {
            let _ = DeleteDC(self.mem_dc);
        }
        self.mem_dc = HDC::default();
        self.bitmap = HBITMAP::default();
        self.old_bitmap = HGDIOBJ::default();
    }
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

fn set_text(hwnd: HWND, s: &str) -> bool {
    let w = to_wide(s);
    unsafe { SetWindowTextW(hwnd, PCWSTR::from_raw(w.as_ptr())).is_ok() }
}

unsafe fn state_of(hwnd: HWND) -> *mut AppState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState
}

unsafe fn state_ptr(hwnd: HWND) -> Result<*mut AppState, ()> {
    let p = state_of(hwnd);
    if p.is_null() {
        Err(())
    } else {
        Ok(p)
    }
}

fn hmenu_id(id: u16) -> HMENU {
    HMENU(id as usize as *mut core::ffi::c_void)
}

// 键盘焦点环。按视觉顺序：编辑框 -> 透明度滑块 -> 置顶 -> 最小化
// -> 关闭 -> 清空 -> 编辑框。
fn focus_ring(state: &AppState) -> [HWND; 6] {
    [
        state.text_edit,
        state.opacity_slider,
        state.pin_btn,
        state.min_btn,
        state.close_btn,
        state.clear_btn,
    ]
}

fn move_focus(state: &AppState, current: HWND, backward: bool) -> bool {
    unsafe {
        let ring = focus_ring(state);
        let Some(index) = ring.iter().position(|&h| h == current) else {
            return false;
        };
        for offset in 1..=ring.len() {
            let next = if backward {
                (index + ring.len() - offset) % ring.len()
            } else {
                (index + offset) % ring.len()
            };
            let target = ring[next];
            if !target.is_invalid() && IsWindowVisible(target).as_bool() {
                let _ = SetFocus(target);
                return true;
            }
        }
        false
    }
}

// 焦点落在主窗口本身时（例如点击底栏空白后被系统交给顶层窗口），
// Tab 也应进入同一焦点环，而不是停留在没有键盘语义的窗口上。
fn focus_from_frame(state: &AppState, backward: bool) -> bool {
    unsafe {
        let ring = focus_ring(state);
        for offset in 0..ring.len() {
            let index = if backward {
                ring.len() - 1 - offset
            } else {
                offset
            };
            let target = ring[index];
            if !target.is_invalid() && IsWindowVisible(target).as_bool() {
                let _ = SetFocus(target);
                return true;
            }
        }
        false
    }
}

// Tab / Shift+Tab 只在本窗口的子控件之间循环。编辑框、按钮和滑块都要走
// 同一条路径，否则某个控件会把焦点困住。
unsafe fn navigate_with_tab(hwnd: HWND) -> bool {
    let parent = GetParent(hwnd).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return false;
    }
    let state = &*p;
    let backward = GetKeyState(VK_SHIFT.0 as i32) < 0;
    move_focus(state, hwnd, backward)
}

// 撤销只接受“纯 Ctrl+Z”，重做只接受“纯 Ctrl+Shift+Z”。AltGr 在部分
// 布局下会同时置位 Ctrl 和 Alt；带 Win 的组合属于系统快捷键，都不能截获。
fn plain_ctrl_z_down() -> bool {
    unsafe {
        GetKeyState(VK_CONTROL.0 as i32) < 0
            && GetKeyState(VK_SHIFT.0 as i32) >= 0
            && GetKeyState(VK_MENU.0 as i32) >= 0
            && GetKeyState(VK_LWIN.0 as i32) >= 0
            && GetKeyState(VK_RWIN.0 as i32) >= 0
    }
}

fn plain_ctrl_shift_z_down() -> bool {
    unsafe {
        GetKeyState(VK_CONTROL.0 as i32) < 0
            && GetKeyState(VK_SHIFT.0 as i32) < 0
            && GetKeyState(VK_MENU.0 as i32) >= 0
            && GetKeyState(VK_LWIN.0 as i32) >= 0
            && GetKeyState(VK_RWIN.0 as i32) >= 0
    }
}

// 按当前鼠标位置调整窗口尺寸。窗口没有 WS_THICKFRAME，右下角手柄的
// 缩放完全由这里实现：以按下时的矩形和鼠标位置为基准计算差值。
// 启动一次缩放拖动。返回是否成功启动。
unsafe fn start_resize(hwnd: HWND) -> bool {
    let p = state_of(hwnd);
    if p.is_null() {
        return false;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return false;
    }
    let mut mouse = POINT::default();
    if GetCursorPos(&mut mouse).is_err() {
        return false;
    }
    (*p).resizing = true;
    (*p).resize_start_rect = rect;
    (*p).resize_start_mouse = mouse;
    let _ = SetCapture(hwnd);
    // 启动兜底定时器，保证鼠标移出窗口时尺寸仍跟随。
    let _ = SetTimer(hwnd, TIMER_RESIZE, 50, None);
    true
}

unsafe fn apply_resize(hwnd: HWND) {
    let p = state_of(hwnd);
    if p.is_null() {
        return;
    }
    let mut mouse = POINT::default();
    if GetCursorPos(&mut mouse).is_err() {
        return;
    }
    let start_mouse = (*p).resize_start_mouse;
    let start_rect = (*p).resize_start_rect;
    let dpi = (*p).dpi.max(96) as i32;
    let min_w = 160 * dpi / 96;
    let min_h = 100 * dpi / 96;
    let width = (start_rect.right - start_rect.left + (mouse.x - start_mouse.x)).max(min_w);
    let height = (start_rect.bottom - start_rect.top + (mouse.y - start_mouse.y)).max(min_h);
    // 以窗口当前的实际尺寸作为“是否变化”的基准。不能用拖动开始时的
    // 尺寸：窗口缩到最小值后继续往内拉时，目标尺寸恒为最小尺寸，但
    // 永远不等于开始尺寸，于是每次鼠标移动和定时器都会重复 SetWindowPos
    // 并强制重绘所有子控件，表现为滑块、按钮和滚动条持续闪烁。
    let mut current = RECT::default();
    if GetWindowRect(hwnd, &mut current).is_err() {
        return;
    }
    let current_w = current.right - current.left;
    let current_h = current.bottom - current.top;
    if width == current_w && height == current_h {
        return;
    }
    let _ = SetWindowPos(
        hwnd,
        None,
        start_rect.left,
        start_rect.top,
        width,
        height,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    // 改变窗口尺寸会让 DWM 重新合成边框，无边框属性可能被重置。
    // 必须在同步重绘之前重新声明，否则这一帧可能先画出白边再被覆盖。
    // 缩放是高频操作，这里只做属性声明，不触发 SWP_FRAMECHANGED 重排。
    disable_dwm_border(hwnd);
    // 尺寸、子控件位置和圆角区域都已经变化，立即把整窗和所有子控件
    // 同步重画到新位置。仅调用 InvalidateRect 时，WM_PAINT 的优先级
    // 低于持续到达的鼠标消息，拖动期间会积压出一连串中间状态，屏幕上
    // 交替出现旧位置的正文和新背景，看起来就是画面撕裂。
    let _ = RedrawWindow(
        hwnd,
        None,
        None,
        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
    );
}

// ---- 自绘垂直滚动条 ----
// 编辑框带 WS_VSCROLL 时会出现一条系统主题滚动条，在深色界面里表现为
// 突兀的白色竖条。而且标准 EDIT 的滚动条不是子窗口，是 EDIT 自己绘制的，
// 无法通过子类化替换。因此 EDIT 不再带 WS_VSCROLL，右侧改由一个独立的
// 自绘子窗口承担滚动条职责：
// 轨道用编辑区同色背景，滑块用略亮的深灰，悬停或拖动时再提亮一档。
//
// 滚动状态一律以正文所在的 EDIT 控件为准（EDIT 的实际语义）：
//   EM_GETFIRSTVISIBLELINE  当前第一可见行，也就是滚动位置 y_offset；
//   EM_GETLINECOUNT          正文总行数 line_count；
//   GetScrollInfo(SB_VERT)   nPage，一屏可见的行数；控件始终没有
//                            WS_VSCROLL 时该调用会失败，此时按格式化
//                            矩形高度和字体行高自行计算（见
//                            visible_line_count）。
// 滑块几何只依赖 EDIT 的真实状态，因此不会出现滑块和正文脱节的问题。
//
// 系统滚动条上下各有一个箭头按钮，提供单行步进。完全接管绘制后如果不
// 再绘制箭头，用户就失去了这个能力，因此这里把箭头也一并自绘。
//
// 箭头按钮的命中区域。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrowHit {
    None,
    Up,
    Down,
}

// 判断客户区 y 坐标落在哪个箭头按钮上。
//
// arrow 是“单个箭头按钮的高度”：上箭头占据 [0, arrow)，
// 下箭头占据 [client_h - arrow, client_h)。
fn arrow_at(y: i32, client_h: i32, arrow: i32) -> ArrowHit {
    let arrow = effective_arrow(arrow, client_h);
    if arrow <= 0 || client_h <= 0 {
        return ArrowHit::None;
    }
    if y < arrow {
        return ArrowHit::Up;
    }
    if y >= client_h - arrow {
        return ArrowHit::Down;
    }
    ArrowHit::None
}

// 箭头按钮实际占用的高度（单个按钮）。窗口很矮时必须压缩箭头，否则
// 上下两个箭头会把轨道完全吃掉，滑块就没有可移动的空间了。
//
// 命中判定、绘制和几何计算必须用同一个值，否则会出现“画了箭头但点不到”
// 或者“箭头压在滑块上”的错位。
fn effective_arrow(arrow: i32, client_h: i32) -> i32 {
    if client_h <= 0 || arrow <= 0 {
        return 0;
    }
    // 连一个箭头加一条能用的轨道都放不下时干脆不画箭头，
    // 免得出现“按钮高 0、命中区却是半个窗口”的错位。
    if client_h < 8 {
        return 0;
    }
    // 上下两个箭头合计不超过滚动条高度的一半，轨道始终有可用空间。
    arrow.min((client_h / 4).max(1))
}

// 自绘滚动条的宽度（物理像素）。与系统纵向滚动条同宽，编辑区右侧
// 因此获得和以前相同的视觉宽度。
fn scroll_bar_width(dpi: u32) -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, dpi).max(1) }
}

struct ScrollBarPaint {
    track: HBRUSH,
    thumb: HBRUSH,
    thumb_hover: HBRUSH,
    arrow_brush: HBRUSH,
    // 箭头不可用时（已经到顶/到底）使用的更暗颜色。
    arrow_dim_brush: HBRUSH,
    // 被子类化的滚动条窗口自身。
    bar: HWND,
    // 正文所在的 EDIT 控件。滚动范围和位置都以它为准。
    edit: HWND,
    // 窗口 DPI。滑块最小高度按 DPI 缩放，保证高 DPI 下仍然点得中。
    dpi: u32,
    // 滑块是否处于悬停态（鼠标正好在滑块上），由 WM_MOUSEMOVE/WM_MOUSELEAVE 维护。
    hovered: bool,
    // 是否正在拖动滑块。拖动期间系统会持续发 WM_MOUSEMOVE。
    dragging: bool,
    // 拖动开始时鼠标相对滑块顶部的偏移，避免滑块在按下瞬间跳到鼠标位置。
    drag_offset: i32,
    // 高精度触控板会送来大量 1~119 的小滚轮 delta，不足一整档的余量
    // 累计在这里，满 120 才真正滚动一次。
    wheel_remainder: i32,
    // 按住箭头按钮时用于自动重复滚动的方向：-1 向上，1 向下，0 未按下。
    repeat_dir: i32,
    // 当前被按下的箭头：ArrowHit 的方向。None 表示没有按下。
    arrow_pressed: ArrowHit,
}

impl ScrollBarPaint {
    fn new() -> Self {
        unsafe {
            Self {
                track: CreateSolidBrush(COLORREF(C_SCROLL_TRACK)),
                thumb: CreateSolidBrush(COLORREF(C_SCROLL_THUMB)),
                thumb_hover: CreateSolidBrush(COLORREF(C_SCROLL_THUMB_HOVER)),
                arrow_brush: CreateSolidBrush(COLORREF(C_SCROLL_ARROW)),
                arrow_dim_brush: CreateSolidBrush(COLORREF(C_SCROLL_ARROW_DIM)),
                bar: HWND::default(),
                edit: HWND::default(),
                dpi: 96,
                hovered: false,
                dragging: false,
                drag_offset: 0,
                wheel_remainder: 0,
                repeat_dir: 0,
                arrow_pressed: ArrowHit::None,
            }
        }
    }

    // 滑块最小高度（物理像素）。
    fn min_thumb_h(&self) -> i32 {
        SCROLL_THUMB_MIN_H * self.dpi.max(96) as i32 / 96
    }

    fn release(&mut self) {
        unsafe {
            for brush in [
                self.track,
                self.thumb,
                self.thumb_hover,
                self.arrow_brush,
                self.arrow_dim_brush,
            ] {
                if !brush.is_invalid() {
                    let _ = DeleteObject(brush);
                }
            }
        }
    }

    // 箭头按钮边长（物理像素）。
    fn arrow_size(&self) -> i32 {
        SCROLL_ARROW_SIZE * self.dpi.max(96) as i32 / 96
    }

    // 停止箭头按钮的自动重复。
    //
    // 返回停止前是否有箭头处于按下态。调用方需要据此决定要不要重画：
    // 本函数会把 arrow_pressed 清成 None，调用方在返回后再判断就永远
    // 得到 false，箭头的按下高亮会一直留在屏幕上。
    fn stop_repeat(&mut self, bar: HWND) -> bool {
        unsafe {
            if self.repeat_dir != 0 && !bar.is_invalid() {
                let _ = KillTimer(bar, SCROLL_REPEAT_TIMER);
            }
        }
        self.repeat_dir = 0;
        let was_pressed = self.arrow_pressed != ArrowHit::None;
        self.arrow_pressed = ArrowHit::None;
        was_pressed
    }
}

// 某个客户区 y 坐标是否落在滑块上。参数显式传入，避免在查询 EDIT 状态
// （会向 EDIT 发消息）期间持有对 ScrollBarPaint 的可变借用。
unsafe fn thumb_contains(bar: HWND, edit: HWND, y: i32, min_thumb_h: i32, arrow: i32) -> bool {
    if bar.is_invalid() {
        return false;
    }
    let mut client = RECT::default();
    if GetClientRect(bar, &mut client).is_err() {
        return false;
    }
    let client_h = client.bottom - client.top;
    let Some(geom) = scroll_geometry(edit, client_h, min_thumb_h, arrow) else {
        return false;
    };
    y >= geom.thumb_top && y < geom.thumb_top + geom.thumb_h
}

// 创建编辑区右侧的自绘滚动条并安装子类化过程。
//
// 标准多行 EDIT 的滚动条不是子窗口：即使样式带 WS_VSCROLL，
// FindWindowExW(..., "ScrollBar", ...) 也找不到任何子窗口，滚动条由
// EDIT 自己绘制。子类化一个不存在的窗口，这正是此前“滚动条仍然是
// 白色”的原因。现在 EDIT 不再带 WS_VSCROLL，滚动条是这个独立子窗口，
// 绘制和命中测试全部由本程序负责。
unsafe fn create_scroll_bar(p: *mut AppState) -> bool {
    if p.is_null() {
        return false;
    }
    let hwnd = (*p).hwnd;
    let hinstance = GetModuleHandleW(None).unwrap_or_default();
    let bar = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        SCROLLBAR_CLASS_NAME,
        w!(""),
        WS_CHILD | WS_VISIBLE,
        0,
        0,
        0,
        0,
        hwnd,
        HMENU::default(),
        hinstance,
        None,
    )
    .unwrap_or_default();
    if bar.is_invalid() {
        return false;
    }
    (*p).scroll_bar = bar;
    let paint = &mut *(*p).scroll_paint as *mut ScrollBarPaint;
    (*paint).bar = bar;
    (*paint).edit = (*p).text_edit;
    (*paint).dpi = (*p).dpi;
    if SetWindowSubclass(bar, Some(scroll_bar_subclass_proc), 1, paint as usize).as_bool() {
        (*p).scroll_subclassed = true;
    }
    true
}

// 刷新自绘滚动条持有的编辑框/DPI，并请求重画。
//
// 滚动条窗口随编辑框在 WM_CREATE 中创建，此后只随布局移动，系统不会
// 销毁或替换它。因此这里不再需要每次 EN_CHANGE/WM_SIZE 重新查找窗口。
unsafe fn attach_scroll_bar(p: *mut AppState) {
    if p.is_null() {
        return;
    }
    let paint = &mut *(*p).scroll_paint as *mut ScrollBarPaint;
    (*paint).edit = (*p).text_edit;
    (*paint).dpi = (*p).dpi;
    let bar = (*p).scroll_bar;
    if bar.is_invalid() || !IsWindow(bar).as_bool() {
        return;
    }
    // SetWindowSubclass 可能因资源不足失败。失败时不能把窗口当成已接管，
    // 后续 WM_SIZE / EN_CHANGE 会再次调用这里重试。
    if !(*p).scroll_subclassed
        && SetWindowSubclass(bar, Some(scroll_bar_subclass_proc), 1, paint as usize).as_bool()
    {
        (*p).scroll_subclassed = true;
    }
    let _ = InvalidateRect(bar, None, false);
}

// 解除对自绘滚动条的接管，并把状态清空。滚动条是主窗口的子窗口，
// 会随主窗口一起销毁，这里不主动 DestroyWindow。
unsafe fn detach_scroll_bar(p: *mut AppState) {
    if p.is_null() {
        return;
    }
    let bar = (*p).scroll_bar;
    if (*p).scroll_subclassed && !bar.is_invalid() && IsWindow(bar).as_bool() {
        let _ = RemoveWindowSubclass(bar, Some(scroll_bar_subclass_proc), 1);
    }
    (*p).scroll_subclassed = false;
    (*p).scroll_bar = HWND::default();
    let paint = &mut *(*p).scroll_paint as *mut ScrollBarPaint;
    (*paint).bar = HWND::default();
    (*paint).edit = HWND::default();
}

// 停止自绘滚动条的自动重复与拖动状态。
//
// 窗口隐藏或最小化时子窗口并不会被销毁，滚动条上的定时器会继续触发，
// 导致窗口看不见却仍在滚动正文。隐藏前必须显式停掉。
unsafe fn stop_scroll_interaction(p: *mut AppState) {
    if p.is_null() {
        return;
    }
    let paint = &mut *(*p).scroll_paint as *mut ScrollBarPaint;
    let bar = (*paint).bar;
    (*paint).stop_repeat(bar);
    (*paint).dragging = false;
    (*paint).hovered = false;
    if !bar.is_invalid() {
        // 只释放滚动条自己持有的捕获，避免误伤主窗口的缩放拖动或
        // 滑块拖动。
        if GetCapture() == bar {
            let _ = ReleaseCapture();
        }
        let _ = InvalidateRect(bar, None, false);
    }
}

// EDIT 的垂直滚动状态。全部以“行”为单位：
//   pos     当前第一可见行；
//   max_pos 能滚到的最大第一可见行（正文末尾刚好填满视口时的值）；
//   page    一屏可见的行数；
//   total   正文总行数。
//
// 上下限的语义曾经混淆过：EDIT 的 nMax 是“内容高度减去一屏”的位移量，
// 不是总行数，直接拿 nMax 当滚动上限会让正文永远差一行到底。这里统一用
// EM_GETLINECOUNT 换算行数上限，避免两套互相矛盾的公式并存。
#[derive(Clone, Copy)]
struct ScrollState {
    pos: i32,
    max_pos: i32,
    page: i32,
    total: i32,
}

// 一屏可见的行数。标准 EDIT 从创建起就没有 WS_VSCROLL 时，GetScrollInfo
// 不返回页信息（调用失败），因此不能用 nPage 作为唯一数据源。这里用文本
// 格式化矩形的高度除以单行高度，行高取自编辑框自身的字体。
unsafe fn visible_line_count(edit: HWND) -> i32 {
    let mut rect = RECT::default();
    let _ = send(
        edit,
        EM_GETRECT,
        WPARAM(0),
        LPARAM(&mut rect as *mut RECT as isize),
    );
    let height = rect.bottom - rect.top;
    if height <= 0 {
        return 1;
    }
    let font = HFONT(send(edit, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut _);
    let dc = GetDC(edit);
    if dc.is_invalid() {
        return 1;
    }
    let old = if font.is_invalid() {
        HGDIOBJ::default()
    } else {
        SelectObject(dc, font)
    };
    let mut tm = TEXTMETRICW::default();
    let line_height = if GetTextMetricsW(dc, &mut tm).as_bool() {
        tm.tmHeight + tm.tmExternalLeading
    } else {
        0
    };
    if !old.is_invalid() {
        SelectObject(dc, old);
    }
    ReleaseDC(edit, dc);
    if line_height <= 0 {
        return 1;
    }
    (height / line_height).max(1)
}

// 读取 EDIT 的垂直滚动状态。所有数据都以正文控件本身为准，
// 与滚动条窗口无关。
unsafe fn edit_scroll_state(edit: HWND) -> Option<ScrollState> {
    if edit.is_invalid() {
        return None;
    }
    // 第一可见行和总行数由 EM_GETFIRSTVISIBLELINE / EM_GETLINECOUNT 提供，
    // 两者都不依赖滚动条。
    let pos = send(edit, EM_GETFIRSTVISIBLELINE, WPARAM(0), LPARAM(0)).0 as i32;
    let total = (send(edit, EM_GETLINECOUNT, WPARAM(0), LPARAM(0)).0 as i32).max(1);
    // GetScrollInfo 在控件始终没有 WS_VSCROLL 时会失败，此时退回到
    // 按格式化矩形和字体行高计算。页数不能超过总行数，否则 max_pos
    // 会变成负数、滑块比例失真。
    let mut si = SCROLLINFO {
        cbSize: mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_PAGE,
        ..Default::default()
    };
    let page = if GetScrollInfo(edit, SB_VERT, &mut si).is_ok() && si.nPage > 0 {
        si.nPage as i32
    } else {
        visible_line_count(edit)
    }
    .clamp(1, total);
    // 最后一屏的起始行 = 总行数 - 一屏可见行数。
    let max_pos = (total - page).max(0);
    Some(ScrollState {
        pos: pos.clamp(0, max_pos),
        max_pos,
        page,
        total,
    })
}

// 滑块几何。thumb_top/thumb_h 是滚动条客户区内的像素坐标。
struct ScrollGeometry {
    // 轨道顶部在滚动条客户区内的 y 坐标。等于单个箭头按钮的高度。
    // 拖动换算时必须先减去它，否则滑块位置会整体偏移一个箭头的距离。
    track_top: i32,
    thumb_top: i32,
    thumb_h: i32,
    // 滑块顶部可以移动的总行程。
    travel: i32,
    state: ScrollState,
}

// 由滚动状态换算滑块的位置和长度。返回 None 表示内容不足一屏，没有滑块。
//
// arrow 是单个箭头按钮的高度。轨道区域被压缩到上下箭头之间，滑块只能在
// 轨道内移动，与系统滚动条的行为一致。
unsafe fn scroll_geometry(
    edit: HWND,
    client_h: i32,
    min_thumb_h: i32,
    arrow: i32,
) -> Option<ScrollGeometry> {
    if client_h <= 0 {
        return None;
    }
    let state = edit_scroll_state(edit)?;
    scroll_geometry_from_state(state, client_h, min_thumb_h, arrow)
}

// 已取到滚动状态时的几何计算。绘制路径需要同时拿到状态（判断箭头是否
// 变暗）和几何（画滑块），分成两个函数可以避免向 EDIT 重复查询。
fn scroll_geometry_from_state(
    state: ScrollState,
    client_h: i32,
    min_thumb_h: i32,
    arrow: i32,
) -> Option<ScrollGeometry> {
    if client_h <= 0 {
        return None;
    }
    if state.max_pos <= 0 {
        return None;
    }
    // 箭头按钮不能把轨道完全吃掉。窗口很矮时按比例压缩箭头，优先保证
    // 轨道和滑块可用。effective_arrow 保证返回的是单个按钮的高度。
    let arrow = effective_arrow(arrow, client_h);
    // 上下各一个箭头，轨道夹在中间。
    let track_top = arrow;
    let track_h = (client_h - arrow * 2).max(1);
    // 滑块长度按可见比例计算，并保证一个下限，否则长文本时只剩几像素，
    // 反而点不中。
    let mut thumb_h = (track_h as i64 * state.page as i64 / state.total as i64) as i32;
    // 下限必须落在 [1, track_h] 内：i32::clamp 在 min > max 时会 panic，
    // 而 track_h 极小（窗口很矮）时 min_thumb_h 可能超过它。
    let lower = min_thumb_h.clamp(1, track_h.max(1));
    thumb_h = thumb_h.clamp(lower, track_h);
    let travel = track_h - thumb_h;
    // 滑块位置与滚动位置成线性比例，走到 max_pos 时正好贴到底部。
    let thumb_top = if travel > 0 {
        (travel as i64 * state.pos as i64 / state.max_pos as i64) as i32
    } else {
        0
    };
    Some(ScrollGeometry {
        track_top,
        thumb_top: track_top + thumb_top,
        thumb_h,
        travel,
        state,
    })
}

// 绘制自绘滚动条。返回是否绘制了滑块。
unsafe fn paint_scroll_bar(hwnd: HWND, hdc: HDC, paint: &ScrollBarPaint) -> bool {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_err() {
        return false;
    }
    let client_h = client.bottom - client.top;
    // 轨道铺满整个滚动条区域，颜色与编辑区背景一致。
    FillRect(hdc, &client, paint.track);
    // 命中判定、绘制和几何计算必须用同一个箭头高度，否则会出现
    // “画了箭头但点不到”或“箭头压在滑块上”的错位。
    let arrow = effective_arrow(paint.arrow_size(), client_h);
    // 画箭头。即使正文不足一屏（没有滑块），箭头按钮依然要显示，
    // 否则用户会失去“单行步进”这个能力。
    // 滚动位置决定箭头是否还有用：到顶时上箭头变暗，到底时下箭头变暗。
    let state = edit_scroll_state(paint.edit);
    draw_scroll_arrows(hdc, &client, paint, arrow, state);
    let Some(geom) =
        state.and_then(|s| scroll_geometry_from_state(s, client_h, paint.min_thumb_h(), arrow))
    else {
        return false;
    };
    let brush = if paint.hovered || paint.dragging {
        paint.thumb_hover
    } else {
        paint.thumb
    };
    // 滑块左右各留 2 像素，边距让滑块看起来更贴合深色风格。
    let mut rect = RECT {
        left: client.left + 2,
        top: geom.thumb_top,
        right: client.right - 2,
        bottom: geom.thumb_top + geom.thumb_h,
    };
    if rect.right <= rect.left {
        rect.right = rect.left + 1;
    }
    if rect.bottom <= rect.top {
        rect.bottom = rect.top + 1;
    }
    // 圆角滑块。用 NULL_PEN 去掉描边，否则会留下一条比背景更亮的硬边。
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
    let radius = (rect.right - rect.left).min(rect.bottom - rect.top) / 2;
    let _ = RoundRect(
        hdc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radius,
        radius,
    );
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old_brush);
    true
}

// 绘制上下箭头按钮。系统滚动条的这个能力不能因为自绘而丢失：
// 用户可以用它逐行滚动，比拖动滑块更精确。
//
// state 为 None 表示拿不到正文状态，此时两个箭头都按可用处理，
// 避免因为一次查询失败就丢失逐行滚动能力。
unsafe fn draw_scroll_arrows(
    hdc: HDC,
    client: &RECT,
    paint: &ScrollBarPaint,
    arrow: i32,
    state: Option<ScrollState>,
) {
    if arrow <= 0 {
        return;
    }
    let w = client.right - client.left;
    if w <= 0 {
        return;
    }
    // arrow 是单个箭头按钮的高度，三角形取其一半作为边长。
    let half = arrow / 2;
    if half <= 0 {
        return;
    }
    // 按下中的箭头用更亮的颜色，和系统滚动条“按下高亮”的反馈一致。
    // 已经滚到顶/底的一侧用更暗的颜色，表示该方向没有更多内容。
    let pressed = paint.arrow_pressed;
    let (at_top, at_bottom) = match state {
        Some(s) => (s.pos <= 0, s.pos >= s.max_pos),
        None => (false, false),
    };
    let up_brush = if pressed == ArrowHit::Up {
        paint.thumb_hover
    } else if at_top {
        paint.arrow_dim_brush
    } else {
        paint.arrow_brush
    };
    let down_brush = if pressed == ArrowHit::Down {
        paint.thumb_hover
    } else if at_bottom {
        paint.arrow_dim_brush
    } else {
        paint.arrow_brush
    };
    // 箭头用文字绘制时基线和对齐都很难控制，改用三角形多边形：
    // 上箭头三个顶点朝上，下箭头朝下，形状与系统滚动条一致。
    let cx = client.left + w / 2;
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));

    // 上箭头：顶点在上，底边在下。按钮占 [client.top, client.top + arrow)。
    let up = [
        POINT {
            x: cx,
            y: client.top + (arrow - half) / 2,
        },
        POINT {
            x: cx - half / 2,
            y: client.top + (arrow - half) / 2 + half,
        },
        POINT {
            x: cx + half / 2,
            y: client.top + (arrow - half) / 2 + half,
        },
    ];
    let old_brush = SelectObject(hdc, up_brush);
    let _ = Polygon(hdc, &up);
    SelectObject(hdc, old_brush);

    // 下箭头：顶边在上，顶点在下。按钮占 [client.bottom - arrow, client.bottom)。
    let bottom = client.bottom;
    let down_top = bottom - arrow;
    let down = [
        POINT {
            x: cx - half / 2,
            y: down_top + (arrow - half) / 2,
        },
        POINT {
            x: cx + half / 2,
            y: down_top + (arrow - half) / 2,
        },
        POINT {
            x: cx,
            y: down_top + (arrow - half) / 2 + half,
        },
    ];
    let old_brush = SelectObject(hdc, down_brush);
    let _ = Polygon(hdc, &down);

    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
}

// Shift+滚轮统一在这里处理，编辑框、按钮、滑块和标题栏空白处都可用。
fn adjust_opacity_with_wheel(hwnd: HWND, wp: WPARAM) -> bool {
    let keys = (wp.0 as u32 & 0xFFFF) as u16;
    if keys & MK_SHIFT == 0 {
        return false;
    }
    let Some((slider, current)) = opacity_slider_state(hwnd) else {
        return false;
    };
    let delta = ((wp.0 as u32 >> 16) & 0xFFFF) as i16;
    if delta == 0 {
        return true;
    }
    // 鼠标滚轮通常一次给 120；Windows Precision Touchpad 会给出大量
    // 1~119 的小 delta。先累计到完整一档再改透明度，避免一次手势把
    // 透明度从 30 直接推到 100，也避免每个小事件都调用一次分层窗口 API。
    let steps = unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return true;
        }
        let remainder = (*p).wheel_remainder;
        // 方向反转时丢弃上一方向残留的余量：否则反向滚动会先“回弹”
        // 一小段再真正生效，手感发涩。
        let remainder = if remainder != 0 && (remainder > 0) != (delta > 0) {
            0
        } else {
            remainder
        };
        let accumulated = remainder.saturating_add(delta as i32);
        // 向零截断的除法：余量保持在 (-120, 120) 且保留符号，
        // 这样下一次同方向滚动能接着累计。
        let steps = accumulated / 120;
        (*p).wheel_remainder = accumulated - steps * 120;
        steps
    };
    if steps == 0 {
        return true;
    }
    let next = (current + steps).clamp(30, 100);
    if next == current {
        return true;
    }
    set_slider_position(hwnd, slider, next);
    true
}

// 读取滑块状态时立刻释放对 AppState 的借用。发送 TBM_* 消息可能同步进入
// 子类化/父窗口过程，不能跨消息调用持有 &mut AppState。
fn opacity_slider_state(hwnd: HWND) -> Option<(HWND, i32)> {
    unsafe {
        let p = state_ptr(hwnd).ok()?;
        let slider = (*p).opacity_slider;
        if slider.is_invalid() {
            return None;
        }
        let current = send(slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32;
        Some((slider, current))
    }
}

fn set_slider_position(hwnd: HWND, slider: HWND, pos: i32) {
    unsafe {
        // TBM_SETPOS 的 wParam 为 TRUE 时会自动重绘滑块，不需要再手动失效。
        let _ = send(slider, TBM_SETPOS, WPARAM(1), LPARAM(pos as isize));
        // 同时更新分层窗口的 alpha；WM_HSCROLL 会再走一次，但 alpha 未变时
        // 会被 last_alpha 挡住，不会重复调用 SetLayeredWindowAttributes。
        // 若 API 失败，last_alpha 不会更新，后续调用仍会重试。
        apply_opacity_alpha(hwnd, pos);
    }
}

fn apply_opacity_alpha(hwnd: HWND, pos: i32) {
    unsafe {
        let alpha = (pos.clamp(30, 100) * 255 / 100) as u8;
        let p = state_of(hwnd);
        if p.is_null() || (*p).last_alpha == alpha {
            return;
        }
        // 只有 API 真正成功后才记住 alpha；失败时保留旧值，下一次滑块
        // 消息或滚轮事件仍会重试，而不是误以为已经生效。
        if SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).is_ok() {
            (*p).last_alpha = alpha;
        }
    }
}

fn screen_work_area(point: POINT) -> RECT {
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }
}

// 拖动时允许窗口跨屏，只把标题栏限制在虚拟桌面内。
fn clamp_title_bar_to_virtual_screen(rect: &mut RECT, dpi: u32) {
    unsafe {
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let title_h = (28 * dpi as i32 / 96).min(height).max(1);
        let min_visible = (width / 3).max(48 * dpi as i32 / 96);
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if vw <= 0 || vh <= 0 {
            return;
        }
        let min_left = vx - width + min_visible;
        let max_left = vx + vw - min_visible;
        if min_left <= max_left {
            rect.left = rect.left.clamp(min_left, max_left);
            rect.right = rect.left + width;
        }
        let max_top = (vy + vh - title_h).max(vy);
        rect.top = rect.top.clamp(vy, max_top);
        // WM_MOVING 直接使用这里返回的 RECT。只改 left/top 会让窗口在撞到
        // 屏幕边缘时被压扁，必须同步平移 right/bottom 以保持宽高。
        rect.bottom = rect.top + height;
    }
}

// 窗口被移到显示器之外、或显示器布局变化后，把整窗放回最近的工作区。
fn clamp_rect_to_work_area(rect: &mut RECT, dpi: u32) {
    unsafe {
        let monitor = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let work = info.rcWork;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let dpi = dpi.max(96) as i32;
        let title_h = (28 * dpi / 96).min(height).max(1);
        let min_visible = (width / 3).max(48 * dpi / 96);
        let min_left = work.left - width + min_visible;
        let max_left = work.right - min_visible;
        if min_left <= max_left {
            rect.left = rect.left.clamp(min_left, max_left);
            rect.right = rect.left + width;
        }
        let max_top = (work.bottom - title_h).max(work.top);
        rect.top = rect.top.clamp(work.top, max_top);
        // 移动/缩放后仅调整位置，不能顺带改变窗口尺寸。
        rect.bottom = rect.top + height;
    }
}

// 仅在窗口可见且未最小化时调用。显示器被拔掉、任务栏位置变化或从托盘
// 恢复后，把窗口拉回最近工作区；只移动，不改变用户设置的尺寸。
fn ensure_window_visible(hwnd: HWND) {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        let before = rect;
        let dpi = GetDpiForWindow(hwnd).max(96);
        clamp_rect_to_work_area(&mut rect, dpi);
        if rect != before {
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

unsafe fn send(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    SendMessageW(hwnd, msg, wp, lp)
}

// 原样读取 UTF-16 码元。编辑框允许接收孤立代理项，若先走 String 再写回，
// 这些码元会被替换字符覆盖，撤销清空就不再是逐字还原。
unsafe fn read_text_into(hwnd: HWND, buf: &mut Vec<u16>) -> usize {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        buf.clear();
        return 0;
    }
    let len = len as usize;
    if buf.len() < len + 1 {
        buf.resize(len + 1, 0);
    }
    let n = GetWindowTextW(hwnd, &mut buf[..len + 1]);
    if n <= 0 {
        buf.clear();
        return 0;
    }
    n as usize
}

unsafe fn get_text_wide(hwnd: HWND) -> Vec<u16> {
    let mut buf = Vec::new();
    let n = read_text_into(hwnd, &mut buf);
    buf.truncate(n);
    buf
}

fn set_text_wide(hwnd: HWND, text: &[u16]) -> bool {
    let mut buf = Vec::with_capacity(text.len() + 1);
    buf.extend_from_slice(text);
    buf.push(0);
    unsafe { SetWindowTextW(hwnd, PCWSTR::from_raw(buf.as_ptr())).is_ok() }
}

// “字符”按 Unicode 码点计数：CRLF 只算一个换行，单独 CR 和单独 LF 各算
// 一个换行；UTF-16 代理对算一个码点，孤立代理项算一个。组合附加符和 ZWJ
// emoji 序列会拆成多个码点。直接扫描 UTF-16 可以省掉先构造 UTF-8 String
// 的整段分配和转码；长文本仍是一次 O(n) 读取。
fn count_chars(text: &[u16]) -> usize {
    let mut count = 0;
    let mut i = 0;
    while i < text.len() {
        let unit = text[i];
        if unit == 0x000D {
            // CRLF 是一个换行，孤立的 CR 也是一个换行；不能因为后一个
            // 码元不是 LF 就跳过 CR，否则仅含 CR 的文本会被计为 0 个字符。
            if i + 1 < text.len() && text[i + 1] == 0x000A {
                i += 2;
            } else {
                i += 1;
            }
            count += 1;
            continue;
        }
        let is_pair = (0xD800..=0xDBFF).contains(&unit)
            && i + 1 < text.len()
            && (0xDC00..=0xDFFF).contains(&text[i + 1]);
        i += if is_pair { 2 } else { 1 };
        count += 1;
    }
    count
}

// ---- 字体创建 ----
fn create_font(height: i32, bold: bool, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold {
                FW_BOLD.0 as i32
            } else {
                FW_NORMAL.0 as i32
            },
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
        let _ = DeleteObject(blue_brush);

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
        let _ = DrawTextW(
            hdc_color,
            &mut t_buf,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(hdc_color, old_font);
        let _ = DeleteObject(font);
        SelectObject(hdc_color, old_color);
        let _ = DeleteDC(hdc_color);

        // 蒙版位图：白底(透明) + 黑色圆角矩形(不透明)
        let old_mask = SelectObject(hdc_mask, hbm_mask);
        let white_brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        FillRect(hdc_mask, &full_rect, white_brush);
        let _ = DeleteObject(white_brush);
        let black_brush = CreateSolidBrush(COLORREF(0));
        let black_pen = CreatePen(PS_SOLID, 0, COLORREF(0));
        let old_pen = SelectObject(hdc_mask, black_pen);
        let old_br = SelectObject(hdc_mask, black_brush);
        let _ = RoundRect(hdc_mask, 1, 1, size - 1, size - 1, size / 3, size / 3);
        SelectObject(hdc_mask, old_br);
        SelectObject(hdc_mask, old_pen);
        let _ = DeleteObject(black_brush);
        let _ = DeleteObject(black_pen);
        SelectObject(hdc_mask, old_mask);
        let _ = DeleteDC(hdc_mask);

        ReleaseDC(None, hdc_screen);

        let ii = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(hbm_color);
        let _ = DeleteObject(hbm_mask);
        hicon
    }
}

// ---- 圆角区域 ----
// 关闭 DWM 合成边框。WS_THICKFRAME 已从窗口样式中去掉，但 DWM 仍可能
// 对顶层窗口绘制一圈系统描边（失焦时尤其明显，呈白色或浅灰色）。
// DWMWA_BORDER_COLOR 设为 DWMWA_COLOR_NONE 表示“不绘制边框”。
//
// 同时关闭 Windows 11 的窗口圆角：圆角由 DWM 合成在窗口外沿，而本程序
// 用 SetWindowRgn 自己做圆角，两者叠加会在边角留下一圈细缝，看上去像
// 一条浅色描边。DWMWCP_DONOTROUND 让 DWM 不再叠加自己的圆角。
//
// 这两个属性从 Windows 11 才支持，早期版本调用失败属于预期情况，直接忽略。
fn disable_dwm_border(hwnd: HWND) {
    unsafe {
        let color: u32 = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &color as *const u32 as *const core::ffi::c_void,
            mem::size_of::<u32>() as u32,
        );
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const DWM_WINDOW_CORNER_PREFERENCE as *const core::ffi::c_void,
            mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

// 重新应用无边框设置。系统主题切换、Explorer 重启或窗口从托盘恢复后，
// DWM 可能重新合成出一圈描边，此时再调用一次即可去掉。
fn refresh_dwm_border(hwnd: HWND) {
    disable_dwm_border(hwnd);
    unsafe {
        // 强制重绘非客户区，确认边框真的消失了。
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

fn apply_rounded_region(hwnd: HWND, w: i32, h: i32, radius: i32) -> bool {
    unsafe {
        if w <= 0 || h <= 0 {
            return false;
        }
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius);
        if rgn.is_invalid() {
            return false;
        }
        // bRedraw 传 FALSE：缩放时尺寸每个像素都在变，让系统为每次区域
        // 切换额外做一次整窗重绘，会和随后的 WM_PAINT/子控件重绘交错，
        // 表现为拖动时画面撕裂。区域本身参与裁剪，下一次统一的失效重绘
        // 即可把新区域画对。
        if SetWindowRgn(hwnd, rgn, false) == 0 {
            let _ = DeleteObject(rgn);
            return false;
        }
        true
    }
}

fn update_region_if_needed(hwnd: HWND, w: i32, h: i32) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        update_region_for_state(p, w, h);
    }
}

// 只在真正需要时重建区域。SetWindowRgn 会同步派发 WM_WINDOWPOSCHANGING 等
// 消息，因此先把 key/radius/hwnd 复制出来，成功后再写回缓存。
unsafe fn update_region_for_state(p: *mut AppState, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    if p.is_null() {
        return;
    }
    let hwnd = (*p).hwnd;
    let radius = 10 * (*p).dpi as i32 / 96;
    let key = (w, h, radius);
    if (*p).region_key == Some(key) {
        return;
    }
    // 只有区域真正切换成功才记住尺寸；否则下次 WM_SIZE 仍会重试。
    if apply_rounded_region(hwnd, w, h, radius) {
        (*p).region_key = Some(key);
    }
}

// 标题文字在客户区中的可用区域。返回 None 表示当前宽度下标题应隐藏，
// 避免压到右侧的透明度滑块或按钮。
//
// 标题文字不再使用 STATIC 子控件，而是直接画进主窗口的离屏缓冲：
// 透明子控件的绘制与父窗口不是同一帧，缩放时会出现文字消失/闪烁。
// 这里与 layout_controls 使用同一套宽度计算。
fn title_text_rect(state: &AppState, w: i32) -> Option<RECT> {
    let margin = state.scale(8);
    let btn = state.scale(22);
    let gap = state.scale(4);
    let slider_w = state.scale(50);
    let left_pad = state.scale(12);
    let title_h = state.scale(28);

    let mut x = w - margin;
    // close / min / pin 三个按钮始终显示
    x -= (btn + gap) * 3;
    if x - slider_w >= left_pad + state.scale(8) {
        x -= slider_w + gap;
    }
    if x - left_pad > state.scale(80) {
        Some(RECT {
            left: left_pad,
            top: 0,
            right: x - gap,
            // 底部分割线那一行留给主窗口绘制。
            bottom: (title_h - 1).max(1),
        })
    } else {
        None
    }
}

// ---- 控件布局 ----
// 优先级（窗口过小时按顺序让出空间）：
//   1. pin/min/close 三个按钮 — 始终显示
//   2. 透明度滑块       — 合法最小宽度下仍会保留
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

        let mut batch = LayoutBatch::begin();

        // —— 右对齐：close / min / pin（始终显示）——
        let mut x = w - margin;
        batch.place(state.close_btn, x - btn, cy - btn / 2, btn, btn);
        x -= btn + gap;
        batch.place(state.min_btn, x - btn, cy - btn / 2, btn, btn);
        x -= btn + gap;
        batch.place(state.pin_btn, x - btn, cy - btn / 2, btn, btn);
        x -= btn + gap;

        // —— 次优先：透明度滑块 ——
        // 需剩余空间 >= slider_w + 左侧留白
        if x - slider_w >= left_pad + state.scale(8) {
            batch.place(
                state.opacity_slider,
                x - slider_w,
                cy - slider_h / 2,
                slider_w,
                slider_h,
            );
            let _ = ShowWindow(state.opacity_slider, SW_SHOWNOACTIVATE);
        } else {
            // 窗口缩窄时滑块会被隐藏；若它正持有键盘焦点，键击会落到
            // 一个看不见的控件。把焦点交回正文，避免键盘操作“失踪”。
            if !state.text_edit.is_invalid() && GetFocus() == state.opacity_slider {
                let _ = SetFocus(state.text_edit);
            }
            let _ = ShowWindow(state.opacity_slider, SW_HIDE);
        }

        // 标题文字由主窗口在 WM_PAINT 中直接绘制（见 title_text_rect），
        // 不再作为子控件参与布局，避免缩放时两边绘制不同步。

        // —— 文本编辑区 + 自绘滚动条 ——
        // 左右各让出 1 个物理像素，给圆角区域留出余量，避免编辑框在圆角
        // 处被硬生生切成直角。编辑区背景与窗口背景同色，这 1 像素看不出来。
        //
        // EDIT 不再带 WS_VSCROLL（系统滚动条由 EDIT 自己绘制，无法着色），
        // 右侧宽度让给独立的自绘滚动条，文本可用宽度与之前一致。
        let bar_w = scroll_bar_width(state.dpi);
        let edit_h = (h - title_h - bottom_h).max(1);
        batch.place(state.text_edit, 1, title_h, (w - 2 - bar_w).max(1), edit_h);
        batch.place(
            state.scroll_bar,
            (w - 1 - bar_w).max(1),
            title_h,
            bar_w,
            edit_h,
        );

        // —— 底栏：清空按钮优先(始终显示)，字符计数次之(空间不足时隐藏) ——
        let by = h - bottom_h;
        let bcy = by + bottom_h / 2;
        let clear_w = state.scale(40);
        let grip_w = state.scale(14);

        // 清空按钮：靠右紧邻 grip
        // 窄窗口时不要让它越过左边界，否则会压在字符计数上或跑出客户区。
        let clear_x = (w - margin - grip_w - gap - clear_w).max(0);
        batch.place(
            state.clear_btn,
            clear_x,
            bcy - state.scale(9),
            clear_w,
            state.scale(18),
        );
        let _ = ShowWindow(state.clear_btn, SW_SHOWNOACTIVATE);

        // 字符计数：剩余空间不足时隐藏
        let count_min = state.scale(40);
        if clear_x - left_pad > count_min + gap {
            let count_w = (clear_x - gap - left_pad).min(state.scale(100));
            batch.place(
                state.count_label,
                left_pad,
                bcy - state.scale(8),
                count_w,
                state.scale(16),
            );
            let _ = ShowWindow(state.count_label, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(state.count_label, SW_HIDE);
        }

        batch.finish(state.hwnd);
    }
}

// 批量布局。DeferWindowPos 在资源不足时会丢弃整组更新，官方要求放弃而不是
// 调用 EndDeferWindowPos；这里保存已排队项，失败时逐项 SetWindowPos 补上。
struct LayoutBatch {
    hdwp: Option<HDWP>,
    pending: Vec<(HWND, i32, i32, i32, i32)>,
}

impl LayoutBatch {
    fn begin() -> Self {
        // 最多同时排队 9 个控件（含编辑区和自绘滚动条），留一点余量。
        Self {
            hdwp: unsafe { BeginDeferWindowPos(10) }.ok(),
            pending: Vec::with_capacity(10),
        }
    }

    fn place(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> bool {
        if hwnd.is_invalid() || w <= 0 || h <= 0 {
            return false;
        }
        if let Some(handle) = self.hdwp {
            match unsafe {
                DeferWindowPos(
                    handle,
                    hwnd,
                    None,
                    x,
                    y,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
                )
            } {
                Ok(next) => {
                    self.hdwp = Some(next);
                    self.pending.push((hwnd, x, y, w, h));
                    return true;
                }
                Err(_) => {
                    self.hdwp = None;
                    for (queued, qx, qy, qw, qh) in self.pending.drain(..) {
                        place_now(queued, qx, qy, qw, qh);
                    }
                }
            }
        }
        place_now(hwnd, x, y, w, h);
        true
    }

    fn finish(mut self, parent: HWND) {
        if let Some(handle) = self.hdwp.take() {
            if unsafe { EndDeferWindowPos(handle) }.is_err() {
                for (hwnd, x, y, w, h) in self.pending.drain(..) {
                    place_now(hwnd, x, y, w, h);
                }
            }
        }
        let _ = unsafe { RedrawWindow(parent, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN) };
    }
}

fn place_now(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
        )
    };
}

fn ensure_back_buffer(
    buffer: &mut Option<BackBuffer>,
    target: HDC,
    width: i32,
    height: i32,
) -> HDC {
    unsafe {
        if width <= 0 || height <= 0 {
            return target;
        }
        if let Some(buf) = buffer.as_mut() {
            // 允许复用略大于客户区的缓冲，避免拖动缩放时每变化几个像素
            // 就销毁/重建整张兼容位图。过大的旧缓冲仍会释放，避免长期占用。
            let slack_w = buf.width - width;
            let slack_h = buf.height - height;
            if slack_w >= 0 && slack_h >= 0 && slack_w <= 128 && slack_h <= 128 {
                return buf.mem_dc;
            }
            buf.release();
        }

        // 按 64 像素取整分配，进一步减少连续缩放过程中的重建次数。
        let alloc_w = width.saturating_add(63) / 64 * 64;
        let alloc_h = height.saturating_add(63) / 64 * 64;

        let mem_dc = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, alloc_w, alloc_h);
        if mem_dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() {
                let _ = DeleteObject(bitmap);
            }
            if !mem_dc.is_invalid() {
                let _ = DeleteDC(mem_dc);
            }
            *buffer = None;
            return target;
        }
        let old_bitmap = SelectObject(mem_dc, bitmap);
        if old_bitmap.is_invalid() {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(mem_dc);
            *buffer = None;
            return target;
        }
        *buffer = Some(BackBuffer {
            mem_dc,
            bitmap,
            old_bitmap,
            width: alloc_w,
            height: alloc_h,
        });
        mem_dc
    }
}

// ---- 透明度 ----
fn apply_opacity(state: &AppState) {
    unsafe {
        // 滑块创建失败时仍要设置分层窗口的 alpha；否则 WS_EX_LAYERED 窗口
        // 在没有 LWA_ALPHA 的情况下可能完全不显示，用户将无法找回窗口。
        let pos = if state.opacity_slider.is_invalid() {
            95
        } else {
            send(state.opacity_slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32
        };
        // 统一走带缓存的重试路径，避免这里成功/失败后与 last_alpha 脱节。
        apply_opacity_alpha(state.hwnd, pos);
    }
}

fn apply_edit_margins(state: &AppState) {
    unsafe {
        if state.text_edit.is_invalid() {
            return;
        }
        let margin = state.scale(8);
        let _ = send(
            state.text_edit,
            EM_SETMARGINS,
            WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize),
            LPARAM(makelong(margin as u16, margin as u16) as isize),
        );
    }
}

// 只创建字体并更新状态，不向子控件发送消息。调用方必须先结束对 AppState 的
// 可变借用，再执行 WM_SETFONT 和重绘，避免消息回调重入时产生别名。
fn rebuild_fonts(state: &mut AppState) -> ([(HWND, HFONT); 2], [HFONT; 4]) {
    let dpi = state.dpi as i32;
    let text_font = create_font(-13 * dpi / 96, false, w!("Microsoft YaHei UI"));
    let title_font_local = create_font(-12 * dpi / 96, true, w!("Microsoft YaHei UI"));
    let small_font = create_font(-10 * dpi / 96, false, w!("Microsoft YaHei UI"));
    let symbol_font = create_font(-11 * dpi / 96, true, w!("Microsoft YaHei UI"));

    // 单个字体创建失败时保留旧句柄；不能先删除旧字体，否则控件仍引用已释放对象。
    let mut old_fonts = [HFONT::default(); 4];
    if !text_font.is_invalid() {
        old_fonts[0] = state.text_font;
        state.text_font = text_font;
    }
    if !title_font_local.is_invalid() {
        old_fonts[1] = state.title_font;
        state.title_font = title_font_local;
    }
    if !small_font.is_invalid() {
        old_fonts[2] = state.small_font;
        state.small_font = small_font;
    }
    if !symbol_font.is_invalid() {
        old_fonts[3] = state.symbol_font;
        state.symbol_font = symbol_font;
    }

    let targets = [
        (state.text_edit, state.text_font),
        (state.count_label, state.small_font),
    ];
    (targets, old_fonts)
}

fn on_dpi_changed(hwnd: HWND, dpi: u32, suggested: RECT) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        // 先一次性写完状态，再调用任何可能同步派发窗口消息的 API。
        let (font_targets, old_fonts, old_icons, tray_added) = {
            let state = &mut *p;
            state.dpi = dpi.max(96);
            state.region_key = None;
            let (font_targets, old_fonts) = rebuild_fonts(state);
            let old_icons = [state.app_icon, state.tray_icon];
            let app_icon = create_app_icon(64 * state.dpi as i32 / 96);
            if !app_icon.is_invalid() {
                state.app_icon = app_icon;
            }
            let tray_icon = create_app_icon(32 * state.dpi as i32 / 96);
            if !tray_icon.is_invalid() {
                state.tray_icon = tray_icon;
            }

            (font_targets, old_fonts, old_icons, state.tray_added)
        };

        // WM_SETFONT 的 lParam 传 FALSE，避免在状态写入期间同步重绘子控件。
        for (child, font) in font_targets {
            if !child.is_invalid() && !font.is_invalid() {
                let _ = send(child, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            }
        }
        let app_icon = (*p).app_icon;
        let tray_icon = (*p).tray_icon;
        let mut app_icon_replaced = false;
        let mut tray_icon_replaced = false;
        if app_icon != old_icons[0] && !app_icon.is_invalid() {
            // WM_SETICON 返回的是旧图标句柄；没有旧图标时同样返回 0，
            // 不能借此判断是否成功。
            let _ = send(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(app_icon.0 as isize),
            );
            app_icon_replaced = true;
        }
        if tray_icon != old_icons[1] && !tray_icon.is_invalid() {
            let mut accepted = true;
            if tray_added {
                let nid = build_nid(&*p);
                // 先更新托盘。失败时窗口还没换图标，新句柄可以直接销毁，
                // 不会出现窗口继续引用已销毁句柄的情况。
                accepted = Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool();
            }
            if accepted {
                let _ = send(
                    hwnd,
                    WM_SETICON,
                    WPARAM(ICON_SMALL as usize),
                    LPARAM(tray_icon.0 as isize),
                );
                tray_icon_replaced = true;
            } else {
                (*p).tray_icon = old_icons[1];
                let _ = DestroyIcon(tray_icon);
            }
        }
        // WM_SETICON 没有可靠的失败返回值；即使消息已投递，也无法确认
        // 窗口是否已停止引用旧句柄。先保留旧图标，等 WM_NCDESTROY 再释放。
        if app_icon_replaced && !old_icons[0].is_invalid() {
            (*p).retired_icons.push(old_icons[0]);
        }
        if tray_icon_replaced && !old_icons[1].is_invalid() {
            (*p).retired_icons.push(old_icons[1]);
        }
        for font in old_fonts {
            if !font.is_invalid() {
                let _ = DeleteObject(font);
            }
        }
        let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);

        let _ = SetWindowPos(
            hwnd,
            None,
            suggested.left,
            suggested.top,
            suggested.right - suggested.left,
            suggested.bottom - suggested.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // DPI 切换会触发 DWM 重新合成窗口边框，无边框属性可能被重置，
        // 表现为跨屏拖动后窗口多出一条白色描边。
        disable_dwm_border(hwnd);

        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client).is_ok() {
            layout_controls(&*p);
            // 尺寸没变时 WM_SIZE 不一定到达，滚动条的 DPI（箭头和滑块
            // 最小高度）需要在这里显式同步。
            attach_scroll_bar(p);
            update_region_if_needed(hwnd, client.right, client.bottom);
        }
        apply_edit_margins(&*p);
    }
}

fn toggle_topmost(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let topmost = !(*p).topmost;
        (*p).topmost = topmost;
        let pin_btn = (*p).pin_btn;
        let insert_after = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        // 失败时回滚状态，避免按钮显示“取消置顶”但窗口仍在最前。
        if SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
        .is_err()
        {
            (*p).topmost = !topmost;
            return;
        }
        // 按钮是自绘的，文本不可见，但仍要同步可访问名称，避免读屏仍报“置顶”。
        set_pin_accessible_name(pin_btn, topmost);
        let _ = InvalidateRect(pin_btn, None, true);
    }
}

// 置顶按钮的可访问名称与关闭按钮一致，描述“点击后会发生什么”：
// 已置顶时读作“取消置顶”，未置顶时读作“置顶”。
fn set_pin_accessible_name(button: HWND, topmost: bool) {
    let name = if topmost { "取消置顶" } else { "置顶" };
    let _ = set_text(button, name);
}

fn set_close_accessible_name(button: HWND, tray_available: bool) {
    let name = if tray_available {
        "关闭到托盘"
    } else {
        "关闭"
    };
    let _ = set_text(button, name);
}

// 清空提示和延迟计数共用底栏。提示定时器创建失败时必须立刻释放
// clear_notice_active，否则字符计数会被永久压制在“Ctrl+Z 撤销”。
unsafe fn show_clear_notice(hwnd: HWND) {
    let p = state_of(hwnd);
    if p.is_null() {
        return;
    }
    let count_label = (*p).count_label;
    (*p).clear_notice_active = true;
    set_text(count_label, "Ctrl+Z 撤销");
    if SetTimer(hwnd, TIMER_CLEAR_NOTICE, 2500, None) == 0 {
        (*p).clear_notice_active = false;
        update_count(hwnd);
    }
}

fn clear_text(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let text_edit = (*p).text_edit;
        let current = get_text_wide(text_edit);
        if current.is_empty() {
            // 正文已经为空时再次按 Ctrl+L：旧快照仍然有效，重新显示提示
            // 和计时，而不是让用户面对一个看起来没有任何反应的快捷键。
            if (*p).cleared_snapshot.is_some() {
                // 上一轮输入预约的计数已经失去意义；提示结束后会重新计数。
                let _ = KillTimer(hwnd, TIMER_UPDATE_COUNT);
                show_clear_notice(hwnd);
            }
            if !text_edit.is_invalid() {
                let _ = SetFocus(text_edit);
            }
            return;
        }
        // EM_GETSEL 的返回值只有 16 位坐标，超过 65535 个字符会截断；
        // 传入两个 DWORD 指针取得完整位置，撤销时才能回到清空前的光标/选区。
        let mut start = 0u32;
        let mut end = 0u32;
        let _ = send(
            text_edit,
            EM_GETSEL,
            WPARAM((&mut start as *mut u32) as usize),
            LPARAM((&mut end as *mut u32) as isize),
        );
        let (selection_start, selection_end) = (start as usize, end as usize);
        (*p).cleared_snapshot = Some(ClearedSnapshot {
            text: current,
            selection_start,
            selection_end,
        });
        // SetWindowTextW 会同步触发 EN_CHANGE，EN_CHANGE 又会写同一份状态；
        // 因此这里只通过裸指针读写，不跨调用持有 &mut。
        // 清空本身不进入多级编辑历史：它由 cleared_snapshot 单独提供
        // 一次 Ctrl+Z 恢复，恢复后历史重新以恢复内容为基准。
        (*p).edit_history.suppress = true;
        let cleared = set_text_wide(text_edit, &[]);
        (*p).edit_history.suppress = false;
        if !cleared {
            // 清空失败时文本仍在，撤销快照必须一并撤回，避免 UI 文案与内容不一致。
            (*p).cleared_snapshot = None;
            let _ = SetFocus(text_edit);
            return;
        }
        (*p).edit_history.reset(Vec::new());
        // WM_SETTEXT 不应再留下清空前的编辑框撤销记录。显式清空它，
        // 确保 Ctrl+Z 只由应用快照恢复；重新输入后快照失效时也不会
        // 通过原生撤销把已经声明失效的旧内容拉回来。
        let _ = send(text_edit, EM_EMPTYUNDOBUFFER, WPARAM(0), LPARAM(0));
        // SetWindowTextW 会同步触发 EN_CHANGE 并启动计数防抖；先取消它，
        // 否则 120ms 后提示会被“字符: 0”覆盖。
        let _ = KillTimer(hwnd, TIMER_UPDATE_COUNT);
        show_clear_notice(hwnd);
        if !text_edit.is_invalid() {
            let _ = SetFocus(text_edit);
        }
    }
}

fn update_count(hwnd: HWND) {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return;
        }
        let text_edit = (*p).text_edit;
        let count_label = (*p).count_label;
        if text_edit.is_invalid() || count_label.is_invalid() {
            return;
        }
        // 达到上限后计数已经没有增长空间；直接给出提示，避免每次防抖都
        // 复制并扫描一百万个 UTF-16 单元。
        if GetWindowTextLengthW(text_edit) >= TEXT_LIMIT as i32 {
            set_text(count_label, "字符: 已达上限");
            return;
        }
        // 只读取 UTF-16 并扫描，不构造 UTF-8 String，避免长文本每 120 ms
        // 产生一次转码分配；计数口径与直接编辑 Unicode 文本一致。
        // 复用缓冲，避免每次计数都重新分配约 2 MB；先 take 出状态，防止
        // 读取/写回期间同步派发的窗口消息重入并形成别名借用。
        let mut scratch = mem::take(&mut (*p).count_scratch);
        let len = read_text_into(text_edit, &mut scratch);
        let count = count_chars(&scratch[..len]);
        (*p).count_scratch = scratch;
        set_text(count_label, &format!("字符: {count}"));
    }
}

fn undo_clear(hwnd: HWND) -> bool {
    unsafe {
        let parent = GetParent(hwnd).unwrap_or_default();
        let p = state_of(parent);
        if p.is_null() {
            return false;
        }
        let count_label = (*p).count_label;
        if GetWindowTextLengthW(hwnd) > 0 {
            return false;
        }
        let Some(snapshot) = (*p).cleared_snapshot.take() else {
            return false;
        };
        let text = &snapshot.text;
        // 恢复清空是程序化修改，不进入多级历史；恢复后历史以恢复内容
        // 为基准重新开始。
        (*p).edit_history.suppress = true;
        let restored = set_text_wide(hwnd, text);
        (*p).edit_history.suppress = false;
        if !restored {
            // 恢复失败说明正文仍为空；把快照放回去，允许用户再次尝试。
            (*p).cleared_snapshot = Some(snapshot);
            return false;
        }
        (*p).edit_history.reset(snapshot.text.clone());
        let _ = KillTimer(parent, TIMER_CLEAR_NOTICE);
        (*p).clear_notice_active = false;
        // WM_SETTEXT 已经同步触发 EN_CHANGE 并预约了一次计数；这里立即
        // 更新计数，同时取消这次重复的全文复制。
        let _ = KillTimer(parent, TIMER_UPDATE_COUNT);
        send(hwnd, EM_EMPTYUNDOBUFFER, WPARAM(0), LPARAM(0));
        let len = text.len();
        let selection_start = snapshot.selection_start.min(len);
        let selection_end = snapshot.selection_end.min(len);
        send(
            hwnd,
            EM_SETSEL,
            WPARAM(selection_start),
            LPARAM(selection_end as isize),
        );
        send(hwnd, EM_SCROLLCARET, WPARAM(0), LPARAM(0));
        // 已经有恢复文本的完整副本，直接据此更新计数，避免 update_count
        // 再次向编辑框读取并复制一遍大段文本。
        if len >= TEXT_LIMIT {
            set_text(count_label, "字符: 已达上限");
        } else {
            set_text(count_label, &format!("字符: {}", count_chars(text)));
        }
        // 无论撤销是从编辑框还是从标题栏按钮触发，都把焦点交回正文。
        let _ = SetFocus(hwnd);
        true
    }
}

// 记录一次用户文本修改。EN_CHANGE 到达时编辑框已经是新状态，与
// last_text 做前缀/后缀比较，只保存真正变化的片段。
unsafe fn record_edit_change(edit: HWND) {
    if edit.is_invalid() {
        return;
    }
    let parent = GetParent(edit).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() || (*p).edit_history.suppress {
        return;
    }
    let new_text = get_text_wide(edit);
    let old = &(*p).edit_history.last_text;
    if old.as_slice() == new_text.as_slice() {
        return;
    }

    let max_prefix = old.len().min(new_text.len());
    let mut prefix = 0;
    while prefix < max_prefix && old[prefix] == new_text[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < max_prefix - prefix
        && old[old.len() - 1 - suffix] == new_text[new_text.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = old[prefix..old.len() - suffix].to_vec();
    let inserted = new_text[prefix..new_text.len() - suffix].to_vec();
    if removed.is_empty() && inserted.is_empty() {
        return;
    }

    let history = &mut (*p).edit_history;
    history.push(EditChange {
        pos: prefix,
        removed,
        inserted,
    });
    history.last_text = new_text;
}

// 应用一步撤销 (redo = false) 或重做 (redo = true)。
unsafe fn apply_edit_history(edit: HWND, redo: bool) -> bool {
    if edit.is_invalid() {
        return false;
    }
    let parent = GetParent(edit).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return false;
    }

    let step = {
        let history = &mut (*p).edit_history;
        if redo {
            history.redo.pop()
        } else {
            history.undo.pop_back()
        }
    };
    let Some(step) = step else {
        return false;
    };

    // 撤销：文档中当前存在的是 inserted，要换回 removed。
    // 重做：文档中当前存在的是 removed，要换回 inserted。
    let select_len = if redo {
        step.removed.len()
    } else {
        step.inserted.len()
    };
    let replacement = if redo {
        step.inserted.clone()
    } else {
        step.removed.clone()
    };
    let end = step.pos.saturating_add(select_len);
    if end > (*p).edit_history.last_text.len() {
        // 历史与正文不一致（理论上不会发生）。丢弃历史并以当前正文
        // 重新建立基准，避免把错误区间写进正文。
        let current = get_text_wide(edit);
        (*p).edit_history.reset(current);
        return false;
    }

    let mut replacement_wide = replacement.clone();
    replacement_wide.push(0);
    (*p).edit_history.suppress = true;
    let _ = send(edit, EM_SETSEL, WPARAM(step.pos), LPARAM(end as isize));
    // wParam = FALSE：不把这次替换写入 EDIT 的原生撤销队列。
    let _ = send(
        edit,
        EM_REPLACESEL,
        WPARAM(0),
        LPARAM(replacement_wide.as_ptr() as isize),
    );
    let caret = step.pos + replacement.len();
    let _ = send(edit, EM_SETSEL, WPARAM(caret), LPARAM(caret as isize));
    let _ = send(edit, EM_SCROLLCARET, WPARAM(0), LPARAM(0));
    // 原生撤销只保留一级且会在两次撤销间来回切换；这里显式清空，
    // 避免任何遗漏路径再次触发它。
    let _ = send(edit, EM_EMPTYUNDOBUFFER, WPARAM(0), LPARAM(0));
    (*p).edit_history.suppress = false;

    {
        let history = &mut (*p).edit_history;
        history
            .last_text
            .splice(step.pos..step.pos + select_len, replacement.iter().copied());
        if redo {
            history.undo.push_back(step);
        } else {
            history.redo.push(step);
        }
    }

    // 撤销/重做改变了正文：结束清空提示，立刻刷新字符计数和滚动条。
    let _ = KillTimer(parent, TIMER_UPDATE_COUNT);
    let _ = KillTimer(parent, TIMER_CLEAR_NOTICE);
    (*p).clear_notice_active = false;
    update_count(parent);
    attach_scroll_bar(p);
    let _ = SetFocus(edit);
    true
}

unsafe fn edit_undo(edit: HWND) -> bool {
    apply_edit_history(edit, false)
}

unsafe fn edit_redo(edit: HWND) -> bool {
    apply_edit_history(edit, true)
}

// 按钮/滑块聚焦时的撤销、重做入口。Ctrl+Z 优先尝试“清空快照”，
// 其余情况使用多级历史；重做只走多级历史。
unsafe fn handle_edit_undo(control: HWND, redo: bool) -> bool {
    let parent = GetParent(control).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return false;
    }
    let edit = (*p).text_edit;
    if edit.is_invalid() {
        return false;
    }
    if redo {
        edit_redo(edit)
    } else if undo_clear(edit) {
        true
    } else {
        edit_undo(edit)
    }
}

fn show_window(hwnd: HWND) {
    unsafe {
        // 从托盘恢复时 DWM 可能重新合成出一圈描边（主题切换、Explorer
        // 重启后尤其明显），恢复前先重新关闭一次边框。
        refresh_dwm_border(hwnd);
        // ShowWindow 只返回“之前是否可见”，不能表示恢复是否成功。用实际
        // 状态确认最小化已解除；失败时仍尝试正常显示，避免窗口停在任务栏
        // 或屏幕外，用户以为点击托盘没有反应。
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            }
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        // 窗口可能在隐藏期间经历显示器拔插或分辨率变化。先夹取到最近的
        // 工作区，再交给前台和焦点逻辑，避免恢复到屏幕外而无法找回。
        ensure_window_visible(hwnd);
        // 前台锁可能拒绝 SetForegroundWindow；用 BringWindowToTop 做一次
        // 同进程兜底，至少把窗口从其他窗口后面抬出来。
        if !SetForegroundWindow(hwnd).as_bool() {
            let _ = BringWindowToTop(hwnd);
            // BringWindowToTop 只调整 Z 序，不会激活窗口；前台锁拒绝时
            // 再设置本线程的活动窗口，随后 SetFocus 才能落到正文。
            let _ = SetActiveWindow(hwnd);
        }
        if let Ok(p) = state_ptr(hwnd) {
            let state = &*p;
            if !state.text_edit.is_invalid() {
                let _ = SetFocus(state.text_edit);
            }
        }
    }
}

fn hide_to_tray(hwnd: HWND) -> bool {
    unsafe {
        let p = state_of(hwnd);
        if p.is_null() {
            return false;
        }
        // 初始化时 Explorer 可能尚未就绪，NIM_ADD 会暂时失败；之后不一定还有
        // TaskbarCreated 可等。关闭到托盘前再尝试一次，避免一次瞬时失败就永久
        // 退化成“关闭即退出”。
        if !(*p).tray_added {
            let icon = (*p).tray_icon;
            let added = add_tray_icon_raw(hwnd, icon);
            (*p).tray_added = added;
            let close_btn = (*p).close_btn;
            set_close_accessible_name(close_btn, added);
            if !added {
                return false;
            }
        }
        // 先完成所有状态写入，再调用会同步派发窗口消息的 ShowWindow，
        // 避免 WM_SIZE 等回调重入时与 &mut AppState 形成别名。
        let first_hide = (*p).first_hide;
        (*p).first_hide = false;
        // 隐藏前停掉滚动条的自动重复：子窗口不会随窗口一起销毁，
        // 定时器若继续触发，窗口看不见却仍在滚动正文。
        stop_scroll_interaction(p);
        let _ = ShowWindow(hwnd, SW_HIDE);
        // 窗口隐藏后，鼠标仍可能停在某个自绘按钮上；如果不清理，下一次
        // 显示时会先画出“悬停”底色，直到鼠标真正移动才恢复。
        if let Some(mut buffer) = (*p).back_buffer.take() {
            buffer.release();
        }
        (*p).hovered_btn = HWND::default();
        (*p).hover_tracking = HWND::default();
        if first_hide {
            let state = &*p;
            show_tray_balloon(state, "悬浮文本框", "窗口已隐藏到托盘，单击图标可恢复");
        }
        true
    }
}

fn close_or_hide(hwnd: HWND) {
    // 关闭按钮优先隐藏到托盘；托盘不可用时直接退出，不再弹确认框。
    if hide_to_tray(hwnd) {
        return;
    }
    quit_app(hwnd);
}

fn quit_app(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

// ---- 托盘 ----
fn build_nid(state: &AppState) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: state.tray_icon,
        ..Default::default()
    };
    fill_wide_array(&mut nid.szTip, "悬浮文本框");
    nid
}

// 与 add_tray_icon 相同，但不持有 &mut AppState：WM_CREATE 和 TaskbarCreated
// 回调里调用 Shell_NotifyIconW 可能重入窗口过程，不能跨调用持有可变引用。
unsafe fn add_tray_icon_raw(hwnd: HWND, icon: HICON) -> bool {
    if icon.is_invalid() {
        return false;
    }
    let mut nid = NOTIFYICONDATAW {
        cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: icon,
        ..Default::default()
    };
    fill_wide_array(&mut nid.szTip, "悬浮文本框");
    if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
        return false;
    }
    // 升级到 v4 后，Shell 才会发送 NIN_SELECT / NIN_KEYSELECT 和
    // WM_CONTEXTMENU，键盘用户才有办法选择和打开托盘菜单。版本设置失败
    // 时必须撤销已加入的图标，否则调用方会记录“托盘可用”，但回调协议
    // 仍是旧版，出现状态与实际行为不一致的半初始化状态。
    if !set_tray_icon_version(hwnd) {
        let remove = NOTIFYICONDATAW {
            cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &remove);
        return false;
    }
    true
}

unsafe fn set_tray_icon_version(hwnd: HWND) -> bool {
    let mut nid = NOTIFYICONDATAW {
        cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &nid).as_bool()
}

fn remove_tray_icon(state: &AppState) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: state.hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn show_tray_balloon(state: &AppState, title: &str, info: &str) {
    if !state.tray_added {
        return;
    }
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
        let topmost = (*p).topmost;
        let visible = IsWindowVisible(hwnd).as_bool();
        let minimized = IsIconic(hwnd).as_bool();
        let can_show = !visible || minimized;
        let can_hide = visible && !minimized;
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        // 只把当前有意义的“显示”或“隐藏”设成默认项，避免回车上执行相反操作。
        // 托盘菜单不提供“清空”：窗口隐藏时没有即时的 Ctrl+Z 撤销入口。
        // 注意 MF_DEFAULT 与 MF_REMOVE 数值相同，不能直接传给 AppendMenuW；
        // 默认项必须在插入后用 SetMenuDefaultItem 设置。
        let show_flags = if can_show {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let hide_flags = if can_hide {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let _ = AppendMenuW(menu, show_flags, IDM_SHOW as usize, w!("显示(&S)"));
        let _ = AppendMenuW(menu, hide_flags, IDM_HIDE as usize, w!("隐藏(&H)"));
        let default_id = if can_show { IDM_SHOW } else { IDM_HIDE };
        let _ = SetMenuDefaultItem(menu, default_id as u32, 0);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
        let top_flag = if topmost {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, top_flag, IDM_TOGGLE_TOP as usize, w!("切换置顶(&T)"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
        let _ = AppendMenuW(menu, MF_STRING, IDM_QUIT as usize, w!("退出(&X)"));

        // 托盘菜单优先贴着图标弹出。键盘打开菜单时鼠标可能离图标很远，
        // 只用 GetCursorPos 会让菜单出现在随机位置；Shell_NotifyIconGetRect
        // 在部分 Shell/被折叠到溢出区的图标上可能失败，此时回退到鼠标位置。
        let mut cur = POINT::default();
        let _ = GetCursorPos(&mut cur);
        let mut x = cur.x;
        let mut y = cur.y;
        let mut place_flags = TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON;
        let mut anchored = false;
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        if let Ok(rect) = Shell_NotifyIconGetRect(&ident) {
            let mut info = MONITORINFO {
                cbSize: mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                x = (rect.left + rect.right) / 2;
                // 任务栏在屏幕上半部时菜单向下展开，否则向上展开，避免菜单
                // 贴着任务栏被挤出工作区。
                if (rect.top + rect.bottom) / 2 < (info.rcWork.top + info.rcWork.bottom) / 2 {
                    y = rect.bottom;
                    place_flags = TPM_CENTERALIGN | TPM_TOPALIGN;
                } else {
                    y = rect.top;
                    place_flags = TPM_CENTERALIGN | TPM_BOTTOMALIGN;
                }
                anchored = true;
            }
        }
        if !anchored {
            x = cur.x;
            y = cur.y;
        }
        // 必须前置窗口，否则点击菜单外无法关闭
        let _ = SetForegroundWindow(hwnd);
        // 不使用 TPM_RETURNCMD：选中项由系统自动派发 WM_COMMAND 到 hwnd
        // TPM_WORKAREA 让系统在菜单贴近屏幕边缘时自动调整，避免托盘在任务栏
        // 角落时菜单被裁掉。
        let _ = TrackPopupMenu(menu, place_flags | TPM_WORKAREA, x, y, 0, hwnd, None);
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
    if msg == WM_KEYDOWN && wp.0 as u16 == VK_Z.0 {
        // 撤销/重做全部由应用层历史处理；必须吞掉按键，绝不能回退到
        // 标准 EDIT 的单级原生撤销（它会在最近两个状态之间来回切换）。
        if plain_ctrl_shift_z_down() {
            let _ = edit_redo(hwnd);
            return LRESULT(0);
        }
        if plain_ctrl_z_down() {
            if !undo_clear(hwnd) {
                let _ = edit_undo(hwnd);
            }
            return LRESULT(0);
        }
    }
    if msg == WM_KEYDOWN && wp.0 as u16 == VK_TAB.0 {
        let _ = navigate_with_tab(hwnd);
        return LRESULT(0);
    }
    if msg == WM_CHAR && wp.0 == VK_TAB.0 as usize {
        return LRESULT(0);
    }
    if msg == WM_CHAR && wp.0 == 0x1A {
        // Ctrl+Z 经 TranslateMessage 生成的 SUB 控制字符。撤销已在
        // WM_KEYDOWN 中完成，这里吞掉它，防止 EDIT 默认过程再撤销一次。
        return LRESULT(0);
    }
    if msg == WM_MOUSEWHEEL {
        let parent = GetParent(hwnd).unwrap_or_default();
        if adjust_opacity_with_wheel(parent, wp) {
            return LRESULT(0);
        }
        // 去掉 WS_VSCROLL 后不能依赖 EDIT 的默认滚轮处理，这里自己
        // 换算并滚动，同时刷新自绘滑块。
        let delta = ((wp.0 as u32 >> 16) & 0xFFFF) as i16;
        scroll_edit_by_wheel(hwnd, delta);
        return LRESULT(0);
    }
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    // 去掉 WS_VSCROLL 后不能假设 EN_VSCROLL 一定还会送达。键盘翻页、
    // 方向键、点击定位和选区拖到边缘时的自动滚动都可能在默认处理中
    // 移动视图，这里在默认处理完成后再刷新一次滑块。
    if msg == WM_KEYDOWN
        || msg == WM_LBUTTONDOWN
        || msg == WM_LBUTTONUP
        || (msg == WM_MOUSEMOVE && (wp.0 as u32 & MK_LBUTTON as u32) != 0)
    {
        invalidate_scroll_bar(hwnd);
    }
    result
}

// ---- 标题栏按钮子类化：完整的 Tab 环路与空格/回车激活 ----
// 编辑框 -> 透明度滑块 -> 置顶 -> 最小化 -> 关闭 -> 清空 -> 编辑框。
// 自绘按钮不会自动处理键盘，因此这里显式接管。
unsafe extern "system" fn button_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            let repeat = (lp.0 & (1 << 30)) != 0;
            let ctrl_down = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let extra_modifier = extra_accel_modifier_down();
            if vk == VK_TAB.0 {
                let _ = navigate_with_tab(hwnd);
                return LRESULT(0);
            }
            if vk == VK_Z.0 {
                if plain_ctrl_shift_z_down() {
                    let _ = handle_edit_undo(hwnd, true);
                    return LRESULT(0);
                }
                if plain_ctrl_z_down() {
                    let _ = handle_edit_undo(hwnd, false);
                    return LRESULT(0);
                }
            }
            if vk == VK_SPACE.0 || vk == VK_RETURN.0 {
                // 带修饰键的空格/回车属于系统、输入法或其他语义。不能把它
                // 交给 BUTTON 默认过程：标准按钮仍可能把它当成空格并触发
                // BN_CLICKED。这里直接吞掉该按键，保证只有纯空格/回车才点击。
                // 自动重复只吞消息，不重复触发动作。
                if ctrl_down || extra_modifier {
                    return LRESULT(0);
                }
                if !repeat {
                    send(hwnd, BM_CLICK, WPARAM(0), LPARAM(0));
                }
                return LRESULT(0);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_MOUSEWHEEL => {
            let parent = GetParent(hwnd).unwrap_or_default();
            if adjust_opacity_with_wheel(parent, wp) {
                return LRESULT(0);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_MOUSEMOVE => {
            // TrackMouseEvent 每次 WM_MOUSELEAVE 后都会失效，因此在首次进入
            // 按钮时重新注册。鼠标在按钮内移动会持续产生 WM_MOUSEMOVE，若每次
            // 都调用会产生大量多余的系统调用；用父窗口中的 hover_tracking 去重。
            let parent = GetParent(hwnd).unwrap_or_default();
            let p = state_of(parent);
            if !p.is_null() {
                let mut tracked = (*p).hover_tracking == hwnd;
                if !tracked {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: HOVER_DEFAULT,
                    };
                    if TrackMouseEvent(&mut tme).is_ok() {
                        (*p).hover_tracking = hwnd;
                        tracked = true;
                    }
                }
                // 注册失败时不进入 hover 态。否则收不到 WM_MOUSELEAVE，
                // 按钮会一直停在悬停底色，直到窗口隐藏或重新布局。
                if tracked {
                    set_hovered_button(hwnd, true);
                }
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_MOUSELEAVE => {
            // 只清理由本按钮注册的跟踪。鼠标快速从 A 移到 B 时，A 的
            // WM_MOUSELEAVE 可能晚于 B 的 WM_MOUSEMOVE 到达；若不加判断，
            // 迟到的 leave 会把 B 的跟踪状态清掉，B 之后的移动又重复注册。
            let parent = GetParent(hwnd).unwrap_or_default();
            let p = state_of(parent);
            if !p.is_null() && (*p).hover_tracking == hwnd {
                (*p).hover_tracking = HWND::default();
            }
            set_hovered_button(hwnd, false);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_CHAR if wp.0 == VK_TAB.0 as usize => LRESULT(0),
        // 空格/回车的字符消息全部吞掉：纯按键已经由上面的 WM_KEYDOWN 处理，
        // 带修饰键的则不应让标准按钮过程再补一次点击。
        WM_CHAR if wp.0 == VK_RETURN.0 as usize || wp.0 == b' ' as usize => LRESULT(0),
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

// 按钮悬停由父窗口统一保存，绘制时 WM_DRAWITEM 才能读到一致状态。切换
// 按钮时旧按钮也要失效重绘，否则鼠标快速跨过边界会残留上一块的 hover 色。
unsafe fn set_hovered_button(button: HWND, hovered: bool) {
    let parent = GetParent(button).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return;
    }
    let previous = (*p).hovered_btn;
    let next = if hovered {
        button
    } else if previous == button {
        HWND::default()
    } else {
        previous
    };
    if previous == next {
        return;
    }
    (*p).hovered_btn = next;
    if !previous.is_invalid() {
        let _ = InvalidateRect(previous, None, false);
    }
    if !next.is_invalid() {
        let _ = InvalidateRect(next, None, false);
    }
}

// ---- 透明度滑块子类化：Tab 离开滑块，Shift+滚轮仍可调节 ----
unsafe extern "system" fn slider_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN if wp.0 as u16 == VK_TAB.0 => {
            let _ = navigate_with_tab(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN
            if wp.0 as u16 == VK_Z.0 && (plain_ctrl_z_down() || plain_ctrl_shift_z_down()) =>
        {
            let _ = handle_edit_undo(hwnd, plain_ctrl_shift_z_down());
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let parent = GetParent(hwnd).unwrap_or_default();
            if adjust_opacity_with_wheel(parent, wp) {
                return LRESULT(0);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_CHAR if wp.0 == VK_TAB.0 as usize => LRESULT(0),
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

// ---- 窗口过程 ----
// 编辑框的垂直滚动条子类。只接管绘制和鼠标交互，滚动位置仍由系统维护。
unsafe extern "system" fn scroll_bar_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    data: usize,
) -> LRESULT {
    // data 指向 AppState 持有的 ScrollBarPaint。指针缺失或无效时不能
    // 裸解引用，否则会直接访问野内存。退回默认过程是最安全的降级。
    let paint = data as *mut ScrollBarPaint;
    if paint.is_null() {
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => {
            // 滚动条是纯滚动控件，不需要激活和键盘焦点。返回 MA_NOACTIVATE
            // 可以避免点击滚动条时把焦点从正文抢走，导致用户接着敲的字符
            // 没有输入到编辑框。
            LRESULT(MA_NOACTIVATE as isize)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.is_invalid() {
                return LRESULT(0);
            }
            paint_scroll_bar(hwnd, hdc, &*paint);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            // PrintWindow / 截图工具不会给子窗口单独发送 WM_PAINT，
            // 而是通过 WM_PRINTCLIENT 要求把内容画到指定 DC。
            let hdc = HDC(wp.0 as *mut _);
            if !hdc.is_invalid() {
                paint_scroll_bar(hwnd, hdc, &*paint);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            // 滚动条高度变化会改变滑块几何。布局结束后立即失效重画。
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let y = get_y_lparam(lp) as i32;
            let edit = (*paint).edit;
            let min_thumb_h = (*paint).min_thumb_h();
            let arrow = (*paint).arrow_size();
            let over_thumb = thumb_contains(hwnd, edit, y, min_thumb_h, arrow);
            if over_thumb != (*paint).hovered {
                (*paint).hovered = over_thumb;
                // 离开通知只注册一次；WM_MOUSEMOVE 会持续到达，
                // 每次都注册会造成大量无用系统调用。
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: HOVER_DEFAULT,
                };
                let _ = TrackMouseEvent(&mut tme);
                let _ = InvalidateRect(hwnd, None, false);
            }
            // 拖动中把鼠标位置换算成新的滚动位置。
            if (*paint).dragging {
                let drag_offset = (*paint).drag_offset;
                set_scroll_by_drag(hwnd, edit, y - drag_offset, min_thumb_h, arrow);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_MOUSELEAVE => {
            (*paint).hovered = false;
            // 这里不结束拖动。拖动滑块时鼠标被按住并快速移动，滑块本身
            // 完全可能短暂移出鼠标位置，WM_MOUSELEAVE 会在拖动过程中
            // 频繁到达；此时释放捕获会让拖动莫名其妙中断。
            // 拖动的结束统一由 WM_LBUTTONUP 和 WM_CAPTURECHANGED 负责。
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_err() {
                return DefSubclassProc(hwnd, msg, wp, lp);
            }
            let y = get_y_lparam(lp) as i32;
            let client_h = client.bottom - client.top;
            let edit = (*paint).edit;
            let min_thumb_h = (*paint).min_thumb_h();
            let arrow = (*paint).arrow_size();
            // 先判箭头按钮：正文不足一屏时没有滑块几何，但仍要保留单行步进。
            match arrow_at(y, client_h, arrow) {
                ArrowHit::Up => {
                    scroll_edit_by_lines(edit, -1);
                    start_arrow_repeat(paint, hwnd, -1);
                    (*paint).arrow_pressed = ArrowHit::Up;
                    // 捕获鼠标：按住箭头后移出滚动条也要能收到松开消息，
                    // 否则自动重复会一直持续到用户再次点击。
                    let _ = SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
                ArrowHit::Down => {
                    scroll_edit_by_lines(edit, 1);
                    start_arrow_repeat(paint, hwnd, 1);
                    (*paint).arrow_pressed = ArrowHit::Down;
                    let _ = SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
                ArrowHit::None => {
                    let Some(geom) = scroll_geometry(edit, client_h, min_thumb_h, arrow) else {
                        return DefSubclassProc(hwnd, msg, wp, lp);
                    };
                    if y >= geom.thumb_top && y < geom.thumb_top + geom.thumb_h {
                        // 按在滑块上：开始拖动，并记录手指相对滑块顶部的偏移。
                        (*paint).dragging = true;
                        (*paint).drag_offset = y - geom.thumb_top;
                        let _ = SetCapture(hwnd);
                        let _ = InvalidateRect(hwnd, None, false);
                    } else {
                        // 按在轨道空白处：翻一页。方向与系统滚动条一致。
                        // 分成两个分支写，避免读者怀疑运算符优先级：整个 if 表达式
                        // 在这里是一个值，乘法作用在它的结果上。
                        let delta = if y < geom.thumb_top {
                            -geom.state.page
                        } else {
                            geom.state.page
                        };
                        scroll_edit_by_lines(edit, delta);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // stop_repeat 会清掉按下态，必须用它的返回值判断是否需要重画。
            let was_pressed = (*paint).stop_repeat(hwnd);
            let was_dragging = (*paint).dragging;
            (*paint).dragging = false;
            if was_pressed || was_dragging {
                // 按下箭头时为了在移出窗口后仍能收到松开消息调用了
                // SetCapture。自定义窗口类不会替我们释放捕获，必须显式
                // 释放，否则鼠标会被滚动条一直“扣住”，其他控件失去响应。
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_CAPTURECHANGED => {
            // 失去捕获（Alt+Tab、窗口失焦）时结束拖动，否则会一直保持按下态。
            let was_pressed = (*paint).stop_repeat(hwnd);
            let was_dragging = (*paint).dragging;
            (*paint).dragging = false;
            if was_pressed || was_dragging {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == SCROLL_REPEAT_TIMER => {
            // 按住箭头按钮时自动重复逐行滚动，与系统滚动条一致。
            let dir = (*paint).repeat_dir;
            if dir == 0 {
                // 理论上 stop_repeat 会同时杀掉定时器，这里再兜一次：
                // 万一状态被清空而定时器仍在，空转的 WM_TIMER 会一直
                // 占用消息队列。
                let _ = KillTimer(hwnd, SCROLL_REPEAT_TIMER);
                LRESULT(0)
            } else {
                // 首次延迟已经过完，后续改成短间隔连续滚动。
                let _ = SetTimer(hwnd, SCROLL_REPEAT_TIMER, SCROLL_REPEAT_INTERVAL, None);
                scroll_edit_by_lines((*paint).edit, dir);
                LRESULT(0)
            }
        }
        WM_MOUSEWHEEL => {
            // 滚轮在滚动条上也要能滚动正文。系统默认会滚动滚动条控件自己，
            // 那个控件没有正文范围，滚了没有任何效果，因此接管。
            let delta = ((wp.0 as u32 >> 16) & 0xFFFF) as i16;
            // 高精度触控板会送来大量 1~119 的小 delta，整除后会变成 0，
            // 滚轮就完全没有反应。这里把不足一档的余量累计起来，满一档
            // 再真正滚动。
            //
            // 方向反转时丢弃上一方向残留的余量：否则反向滚动会先“回弹”
            // 一小段再真正翻页，手感发涩。
            let mut remainder = (*paint).wheel_remainder;
            if remainder != 0 && (remainder > 0) != (delta > 0) {
                remainder = 0;
            }
            let accumulated = remainder + delta as i32;
            // 向零截断的除法：余量保持在 (-120, 120) 且保留符号，
            // 这样下一次同方向滚动能接着累计。
            let notches = accumulated / WHEEL_DELTA as i32;
            (*paint).wheel_remainder = accumulated - notches * WHEEL_DELTA as i32;
            if notches != 0 {
                // 向上滚（delta > 0）要让内容向下走，也就是滚向更早的行。
                // EM_LINESCROLL 的 lParam 为正表示向下翻行，所以这里取负。
                let lines = -notches * wheel_lines_per_notch();
                scroll_edit_by_lines((*paint).edit, lines);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // 滚动条窗口即将销毁：结束拖动、释放捕获，并清空句柄，
            // 避免后续消息再通过失效的 HWND 去查询正文状态。
            // 自动重复定时器也必须停掉，否则窗口销毁后定时器仍在触发，
            // 句柄已被系统回收，消息会投递到无效窗口上。
            (*paint).stop_repeat(hwnd);
            if (*paint).dragging {
                (*paint).dragging = false;
                let _ = ReleaseCapture();
            }
            (*paint).hovered = false;
            // 只有当被销毁的正是当前记录的那个窗口时才清空状态，
            // 避免迟到的销毁消息把新窗口的 bar/edit 清成空句柄。
            if (*paint).bar == hwnd {
                (*paint).bar = HWND::default();
                (*paint).edit = HWND::default();
            }
            // ScrollBarPaint 的生命周期由 AppState 管理，不在这里释放，
            // 否则 AppState 之后释放时会二次释放。
            // WM_NCDESTROY 必须继续传给默认过程，否则窗口的销毁流程
            // 会被截断，滚动条窗口会残留在窗口资源里。
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

// 拖动滑块时把鼠标位置换算成滚动位置。滑块几何与 paint_scroll_bar 一致，
// 因此拖动过程中不会出现滑块位置和实际滚动位置脱节。
//
// hwnd 是滚动条控件本身，正文范围和滚动位置都向父 EDIT 查询。
//
// 按住箭头按钮时启动自动重复：系统滚动条会持续逐行滚动，直到松开鼠标。
// 定时器挂在滚动条子窗口上，与主窗口的三个定时器互不影响。
//
// 这里不传 TIMERPROC：带回调的定时器会绕过窗口过程直接调用函数，
// 子类过程就收不到 WM_TIMER，滚动条也拿不到自身状态。传 None 时系统
// 把 WM_TIMER 投递给窗口，消息会正常经过子类过程。
unsafe fn start_arrow_repeat(paint: *mut ScrollBarPaint, bar: HWND, dir: i32) {
    if paint.is_null() || bar.is_invalid() || dir == 0 {
        return;
    }
    (*paint).stop_repeat(bar);
    (*paint).repeat_dir = dir;
    // 首次延迟用较长的间隔，模拟系统滚动条“按住稍等才开始连发”的手感；
    // 触发之后由 WM_TIMER 分支改成短间隔。
    // 定时器创建失败时必须把方向清回去，否则 repeat_dir 仍非零，
    // 下一次 WM_MOUSEWHEEL 之类的路径会误以为“正在自动重复”。
    if SetTimer(bar, SCROLL_REPEAT_TIMER, SCROLL_REPEAT_FIRST_DELAY, None) == 0 {
        (*paint).repeat_dir = 0;
    }
}

unsafe fn set_scroll_by_drag(hwnd: HWND, edit: HWND, thumb_top: i32, min_thumb_h: i32, arrow: i32) {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_err() {
        return;
    }
    let client_h = client.bottom - client.top;
    // 复用绘制用的几何计算，保证拖动时的手感与画出来的滑块完全一致。
    let Some(geom) = scroll_geometry(edit, client_h, min_thumb_h, arrow) else {
        return;
    };
    if geom.travel <= 0 {
        return;
    }
    // thumb_top 是客户区坐标，必须先减去轨道顶部偏移，换算成滑块在轨道内
    // 的实际位置。不减这一段，滑块位置会整体偏移一个箭头的高度，
    // 表现为拖到顶也留着一段空白、拖到底也到不了底。
    let local_top = thumb_top - geom.track_top;
    let frac = (local_top as f64 / geom.travel as f64).clamp(0.0, 1.0);
    let new_pos = (frac * geom.state.max_pos as f64).round() as i32;
    set_edit_scroll_pos(edit, new_pos);
}

// 让正文滚动若干行（正数向下，负数向上）。
//
// EM_LINESCROLL 的参数容易记错：wParam 是水平滚动的字符数，lParam 才是
// 垂直滚动的行数。行数放在 wParam 里不会报错，但会变成水平滚动，表现为
// 拖动滑块时正文左右乱跳。
//
// 一次消息即可滚动多行，不会像逐行发送 WM_VSCROLL 那样产生 O(n) 次重绘，
// 长文本拖动滑块时不会再卡顿。
unsafe fn scroll_edit_by_lines(edit: HWND, lines: i32) {
    if edit.is_invalid() || lines == 0 {
        return;
    }
    let _ = send(edit, EM_LINESCROLL, WPARAM(0), LPARAM(lines as isize));
    invalidate_scroll_bar(edit);
}

// 编辑框上的普通滚轮（滚动正文）。
//
// 高精度触控板会送来大量 1~119 的 delta，先累计到一整档再滚动；
// 方向反转时丢弃上一方向的余量，避免先回弹一小段。
unsafe fn scroll_edit_by_wheel(edit: HWND, delta: i16) {
    if edit.is_invalid() || delta == 0 {
        return;
    }
    let parent = GetParent(edit).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return;
    }
    let delta = delta as i32;
    let mut remainder = (*p).text_wheel_remainder;
    if remainder != 0 && (remainder > 0) != (delta > 0) {
        remainder = 0;
    }
    let accumulated = remainder.saturating_add(delta);
    let notches = accumulated / WHEEL_DELTA as i32;
    (*p).text_wheel_remainder = accumulated - notches * WHEEL_DELTA as i32;
    if notches != 0 {
        // 滚轮向上（delta > 0）回到更早的行，EM_LINESCROLL 负数向上。
        scroll_edit_by_lines(edit, -notches * wheel_lines_per_notch());
    }
}

// 滚轮一档对应滚动多少行。跟随系统设置（SPI_GETWHEELSCROLLLINES），
// 用户把滚动行数改成 1 或 6 时应当同步生效，而不是永远固定 3 行。
// 系统设置为 0 表示“整屏滚动”，这里退回到常见的 3 行。
fn wheel_lines_per_notch() -> i32 {
    unsafe {
        let mut lines: u32 = 0;
        let ok = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut lines as *mut u32 as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok();
        if ok && lines > 0 {
            lines as i32
        } else {
            3
        }
    }
}

// 把正文滚动到指定的“第一可见行”。先用 EM_LINESCROLL 计算相对位移，
// 滚完再读回实际位置校验；换行方式不一致时实际落点可能与目标差一行，
// 再用剩余差值补一次即可收敛。
//
// 这里刻意不用 WM_VSCROLL + SB_THUMBPOSITION 做“精确落点”：该命令携带
// 的行号放在 HIWORD(wParam) 里只有 16 位，系统按有符号 short 解释。正文
// 上限是 1,000,000 个 UTF-16 单元，行数可能远超 32767，超范围时只能
// 截断，截断后的行号与目标毫无关系，表现为拖动滑块时正文突然跳到
// 另一个位置。宁可差一行，也不要跳到无关位置。
unsafe fn set_edit_scroll_pos(edit: HWND, pos: i32) {
    let Some(state) = edit_scroll_state(edit) else {
        return;
    };
    let target = pos.clamp(0, state.max_pos);
    let mut remaining = target - state.pos;
    // 最多补两次：EM_LINESCROLL 会被系统夹到合法范围，正常情况下一次就
    // 能落到目标；两次仍差一行，说明行的度量本身不一致，接受这个偏差。
    for _ in 0..2 {
        if remaining == 0 {
            break;
        }
        scroll_edit_by_lines(edit, remaining);
        let Some(after) = edit_scroll_state(edit) else {
            break;
        };
        remaining = target - after.pos;
    }
    invalidate_scroll_bar(edit);
}

// 通知父窗口的滚动条重画。滚动位置可能由任何路径改变（键盘、滚轮、
// 拖动、程序化设置），统一在这里刷新，避免滑块停留在旧位置。
unsafe fn invalidate_scroll_bar(edit: HWND) {
    let parent = GetParent(edit).unwrap_or_default();
    let p = state_of(parent);
    if p.is_null() {
        return;
    }
    let bar = (*p).scroll_bar;
    // 窗口销毁流程中句柄可能已经失效。先确认窗口仍然存在，
    // 避免对已回收的 HWND 调用 InvalidateRect。
    if !bar.is_invalid() && IsWindow(bar).as_bool() {
        let _ = InvalidateRect(bar, None, false);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != 0 && Some(msg) == TASKBAR_CREATED_MSG.get().copied() {
        let p = state_of(hwnd);
        if !p.is_null() {
            // TaskbarCreated 可能在 WM_CREATE 的图标初始化完成前到达。
            // 此时没有可加入托盘的图标；提前显示窗口或弹提示会让启动过程
            // 进入半初始化状态，等 WM_CREATE 完成后再处理即可。
            if (*p).tray_icon.is_invalid() {
                (*p).tray_added = false;
                return LRESULT(0);
            }
            (*p).tray_added = false;
            // 不持有 &mut AppState 调用 Shell_NotifyIconW：它可能同步派发消息并重入。
            let icon = (*p).tray_icon;
            let close_btn = (*p).close_btn;
            let icon_ok = add_tray_icon_raw(hwnd, icon);
            (*p).tray_added = icon_ok;
            set_close_accessible_name(close_btn, icon_ok);
            if !icon_ok && !IsWindowVisible(hwnd).as_bool() {
                // Explorer 重启后托盘暂时不可用时，不能让隐藏窗口失去
                // 唯一入口：恢复窗口并提示仍可关闭到托盘。
                // 走统一恢复路径，顺带处理最小化、屏幕外位置和正文焦点。
                show_window(hwnd);
                let _ = MessageBoxW(
                    hwnd,
                    w!("系统托盘暂不可用，窗口已恢复显示。稍后可再次尝试关闭到托盘。"),
                    w!("悬浮文本框"),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
        return LRESULT(0);
    }

    match msg {
        WM_NCCALCSIZE => {
            // 去掉系统非客户区边框。返回 0 表示“整个窗口都是客户区”，
            // 系统就不会再绘制 WS_THICKFRAME 带来的那条灰色/白色边框。
            //
            // wParam == 0 是旧版兼容路径（此时 lp 指向 RECT 而不是
            // NCCALCSIZE_PARAMS），同样不能回退到 DefWindowProcW：回退会
            // 让系统把边框重新画回来，表现为窗口偶尔出现一条突兀的白线。
            // 两条路径都返回 0：wParam != 0 是正常路径，wParam == 0 是旧版
            // 兼容路径（此时 lp 指向 RECT 而不是 NCCALCSIZE_PARAMS）。
            let _ = wp;
            LRESULT(0)
        }

        WM_NCPAINT => {
            // 非客户区不画任何东西。窗口没有标题栏和系统边框，任何一次
            // 非客户区重绘都只可能带来一条突兀的白线或灰边，直接跳过。
            LRESULT(0)
        }

        WM_NCACTIVATE => {
            // 官方契约（WM_NCACTIVATE，见 Winuser.h / MSDN）：
            //   * wParam == FALSE（失活）时，返回 TRUE 表示“系统继续走默认
            //     非客户区处理”，返回 FALSE 才是“阻止这次状态变化”；
            //   * wParam == TRUE（激活）时返回值被忽略，但官方要求返回
            //     TRUE 以让系统完成激活态切换。
            //
            // 之前这里固定返回 TRUE：注释说的“阻止默认绘制”与实际语义
            // 正好相反——失活时它恰恰是**允许**系统走默认非客户区处理，
            // 于是系统会在窗口外沿重新合成一圈白色或浅灰色描边，这正是
            // “切到别的程序后边框突然出现”的来源。
            //
            // 现在按契约实现：失活返回 FALSE 阻止这次非客户区状态变化，
            // 激活返回 TRUE 让系统正常完成切换。最小化状态下按官方要求
            // 交回 DefWindowProc，避免窗口恢复时状态不同步。
            if IsIconic(hwnd).as_bool() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if wp.0 == 0 {
                LRESULT(0)
            } else {
                LRESULT(1)
            }
        }

        WM_CREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            let hinst = cs.hInstance;

            let dpi = GetDpiForWindow(hwnd).max(96);

            // 画刷与画笔
            let bg_brush = CreateSolidBrush(COLORREF(C_BG));
            let bar_brush = CreateSolidBrush(COLORREF(C_BAR));
            let bar_dark_brush = CreateSolidBrush(COLORREF(C_BAR_DARK));
            let btn_hover_brush = CreateSolidBrush(COLORREF(C_BTN_HOVER));
            let btn_close_brush = CreateSolidBrush(COLORREF(C_BTN_CLOSE));
            let dark_pen = CreatePen(PS_SOLID, 1, COLORREF(C_BAR_DARK));
            let grip_pen = CreatePen(PS_SOLID, 1, COLORREF(C_TEXT_MUTED));

            // 字体
            let text_font = create_font(-13 * dpi as i32 / 96, false, w!("Microsoft YaHei UI"));
            let title_font = create_font(-12 * dpi as i32 / 96, true, w!("Microsoft YaHei UI"));
            let small_font = create_font(-10 * dpi as i32 / 96, false, w!("Microsoft YaHei UI"));
            let symbol_font = create_font(-11 * dpi as i32 / 96, true, w!("Microsoft YaHei UI"));

            let state_box = Box::new(AppState {
                hwnd,
                dpi,
                opacity_slider: HWND::default(),
                pin_btn: HWND::default(),
                min_btn: HWND::default(),
                close_btn: HWND::default(),
                text_edit: HWND::default(),
                scroll_bar: HWND::default(),
                scroll_subclassed: false,
                scroll_paint: Box::new(ScrollBarPaint::new()),
                count_label: HWND::default(),
                clear_btn: HWND::default(),
                bg_brush,
                bar_brush,
                bar_dark_brush,
                btn_hover_brush,
                btn_close_brush,
                dark_pen,
                grip_pen,
                text_font,
                title_font,
                small_font,
                symbol_font,
                app_icon: HICON::default(),
                tray_icon: HICON::default(),
                topmost: true,
                tray_added: false,
                first_hide: true,
                hovered_btn: HWND::default(),
                hover_tracking: HWND::default(),
                cleared_snapshot: None,
                edit_history: EditHistory::new(),
                count_scratch: Vec::new(),
                clear_notice_active: false,
                wheel_remainder: 0,
                text_wheel_remainder: 0,
                region_key: None,
                restore_visibility_fix: false,
                // 0 不是合法 alpha（范围约 76..=255），作为“尚未应用”的哨兵。
                retired_icons: Vec::new(),
                resizing: false,
                resize_start_mouse: POINT::default(),
                resize_start_rect: RECT::default(),
                last_alpha: 0,
                back_buffer: None,
            });
            let state_ptr = Box::into_raw(state_box);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            // 注意：以下创建子控件/发送消息都可能同步重入 wnd_proc。
            // 因此初始化阶段只通过裸指针逐字段写入，不跨调用持有 &mut AppState。

            // 创建子控件
            let base = (WS_CHILD | WS_VISIBLE).0;
            // 计数是单行文本；显式单行可避免极端字体度量下自动换行
            // 后被固定高度裁掉。
            let static_style = WINDOW_STYLE(base | SS_CENTERIMAGE | SS_SINGLELINE);

            let slider_style =
                WINDOW_STYLE(base | TBS_NOTICKS | TBS_TRANSPARENTBKGND | WS_TABSTOP.0);
            let opacity_slider = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                TRACKBAR_CLASSW,
                // 自绘标题栏不显示控件标题，但窗口文本仍是读屏器获取
                // 可访问名称的来源。
                w!("透明度"),
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
            (*state_ptr).opacity_slider = opacity_slider;
            let _ = send(
                opacity_slider,
                TBM_SETRANGE,
                WPARAM(1),
                LPARAM(makelong(30, 100) as isize),
            );
            let _ = send(opacity_slider, TBM_SETPOS, WPARAM(1), LPARAM(95));
            // 关闭可视主题，否则滑块会绘制浅色背景，与深色标题栏割裂。
            let _ = SetWindowTheme(opacity_slider, w!(""), w!(""));

            let btn_style = WINDOW_STYLE(base | BS_OWNERDRAW as u32 | WS_TABSTOP.0);
            let make_btn = |id: u16| -> HWND {
                let accessible_name = match id {
                    ID_PIN_BTN => w!("置顶"),
                    ID_MIN_BTN => w!("最小化"),
                    ID_CLOSE_BTN => w!("关闭到托盘"),
                    _ => w!("清空"),
                };
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    accessible_name,
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
            (*state_ptr).pin_btn = make_btn(ID_PIN_BTN);
            (*state_ptr).min_btn = make_btn(ID_MIN_BTN);
            (*state_ptr).close_btn = make_btn(ID_CLOSE_BTN);
            (*state_ptr).clear_btn = make_btn(ID_CLEAR_BTN);

            // 不添加 WS_VSCROLL：标准 EDIT 会自己绘制一条无法可靠着色、
            // 也无法子类化的系统滚动条。纵向滚动能力由 ES_AUTOVSCROLL
            // 和本程序的自绘滚动条共同提供。
            let edit_style = WINDOW_STYLE(
                base | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | ES_NOHIDESEL as u32
                    | WS_TABSTOP.0,
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
            (*state_ptr).text_edit = text_edit;
            let _ = send(
                text_edit,
                WM_SETFONT,
                WPARAM(text_font.0 as usize),
                LPARAM(1),
            );
            let _ = send(text_edit, EM_SETLIMITTEXT, WPARAM(TEXT_LIMIT), LPARAM(0));
            let margin = 8 * dpi as i32 / 96;
            let _ = send(
                text_edit,
                EM_SETMARGINS,
                WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize),
                LPARAM(makelong(margin as u16, margin as u16) as isize),
            );

            // 编辑区右侧的自绘滚动条。必须在 EDIT 创建之后建立，
            // ScrollBarPaint 才能拿到正文窗口句柄。
            if !create_scroll_bar(state_ptr) {
                // 滚动条窗口只负责视觉与鼠标交互；创建失败时编辑框仍可用
                // 键盘和滚轮滚动，不阻断启动。
                (*state_ptr).scroll_bar = HWND::default();
            }

            let count_label = CreateWindowExW(
                // 计数会变短，不能用透明扩展样式，否则底栏会残留旧字形。
                WINDOW_EX_STYLE::default(),
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
            (*state_ptr).count_label = count_label;
            let _ = send(
                count_label,
                WM_SETFONT,
                WPARAM(small_font.0 as usize),
                LPARAM(1),
            );
            // 子类化文本框（Shift+滚轮、Ctrl+Z、Tab）；按钮接管键盘导航。
            let _ = SetWindowSubclass(text_edit, Some(text_edit_subclass_proc), 1, 0);
            let _ = SetWindowSubclass(opacity_slider, Some(slider_subclass_proc), 1, 0);
            for button in [
                (*state_ptr).pin_btn,
                (*state_ptr).min_btn,
                (*state_ptr).close_btn,
                (*state_ptr).clear_btn,
            ] {
                let _ = SetWindowSubclass(button, Some(button_subclass_proc), 1, 0);
            }

            // 图标
            let app_icon = create_app_icon(64 * dpi as i32 / 96);
            let tray_icon = create_app_icon(32 * dpi as i32 / 96);
            (*state_ptr).app_icon = app_icon;
            (*state_ptr).tray_icon = tray_icon;
            let _ = send(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(app_icon.0 as isize),
            );
            let _ = send(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(tray_icon.0 as isize),
            );

            // 托盘
            let tray_added = add_tray_icon_raw(hwnd, tray_icon);
            (*state_ptr).tray_added = tray_added;
            // 关闭按钮的可见图标固定为“×”，但可访问名称应反映实际行为：
            // 托盘可用时是“关闭到托盘”，不可用时是直接退出。
            set_close_accessible_name((*state_ptr).close_btn, tray_added);
            // 窗口创建时默认置顶，按钮名称应描述“点击后会发生什么”。
            set_pin_accessible_name((*state_ptr).pin_btn, true);

            // 初始尺寸（光标所在显示器的工作区居中）
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let work = screen_work_area(cursor);
            let init_w = 280 * dpi as i32 / 96;
            let init_h = 200 * dpi as i32 / 96;
            let x = work.left + ((work.right - work.left) - init_w).max(0) / 2;
            let y = work.top + ((work.bottom - work.top) - init_h).max(0) / 2;
            if SetWindowPos(hwnd, HWND_TOPMOST, x, y, init_w, init_h, SWP_NOACTIVATE).is_err() {
                // 初始置顶同样可能失败；状态和按钮文案必须与窗口实际层级一致。
                (*state_ptr).topmost = false;
                set_pin_accessible_name((*state_ptr).pin_btn, false);
            }
            // 初始位置可能落在另一块不同 DPI 的显示器上。上面的 SetWindowPos
            // 会同步触发 WM_DPICHANGED 并改用系统建议尺寸，因此必须按实际
            // 客户区重建圆角区域，不能固定使用 init_w/h。
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_ok() {
                update_region_if_needed(hwnd, client.right, client.bottom);
            } else {
                let radius = 10 * dpi as i32 / 96;
                if apply_rounded_region(hwnd, init_w, init_h, radius) {
                    (*state_ptr).region_key = Some((init_w, init_h, radius));
                }
            }
            // WM_SIZE 通常会在 WM_CREATE 之后到达，但启动尺寸与创建尺寸
            // 相同时不一定再次触发；这里兜底布局一次，避免控件停在 0×0。
            layout_controls(&*state_ptr);
            apply_opacity(&*state_ptr);

            LRESULT(0)
        }

        WM_SIZE => {
            // 最小化时客户区尺寸为 0，不需要重排控件，保留上一次布局即可。
            // 记下这次最小化，等恢复时再夹取工作区：最小化期间窗口坐标是
            // 屏幕外占位值，此时夹取没有意义，而且显示器可能已经变化。
            if wp.0 as u32 == SIZE_MINIMIZED {
                let p = state_of(hwnd);
                if !p.is_null() {
                    // 最小化时子窗口不会被销毁，滚动条的自动重复定时器若
                    // 不停掉，会在窗口不可见时继续滚动正文。
                    stop_scroll_interaction(p);
                    // 最小化期间不会收到 WM_MOUSELEAVE；恢复时若保留旧
                    // hover 值，按钮会先画出错误的悬停底色。
                    (*p).hovered_btn = HWND::default();
                    (*p).hover_tracking = HWND::default();
                    // 最小化期间不会绘制客户区，释放离屏位图可减少长期占用；
                    // 恢复后的第一次 WM_PAINT 会按需重建。
                    if let Some(mut buffer) = (*p).back_buffer.take() {
                        buffer.release();
                    }
                    (*p).restore_visibility_fix = true;
                }
                return LRESULT(0);
            }
            let p = state_of(hwnd);
            if !p.is_null() {
                layout_controls(&*p);
                // 布局完成后同步滚动条的 DPI 和编辑框句柄，并按新尺寸重画。
                attach_scroll_bar(p);
                let w = loword(lp.0 as u32) as i32;
                let h = hiword(lp.0 as u32) as i32;
                update_region_if_needed(hwnd, w, h);
            }
            let needs_visibility_fix = if p.is_null() {
                false
            } else {
                let fix = (*p).restore_visibility_fix;
                (*p).restore_visibility_fix = false;
                fix
            };
            if needs_visibility_fix && wp.0 as u32 != SIZE_MAXIMIZED {
                ensure_window_visible(hwnd);
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let dpi = loword(wp.0 as u32) as u32;
            if lp.0 != 0 {
                let suggested = *(lp.0 as *const RECT);
                on_dpi_changed(hwnd, dpi, suggested);
            }
            LRESULT(0)
        }

        // 分辨率、任务栏位置变化或拖动结束时，把窗口拉回可见工作区。
        // WM_SETTINGCHANGE 覆盖大量无关设置（字体、主题、输入法），只在
        // SPI_SETWORKAREA 真正变化时夹取，避免其他应用改设置时无意义地移动窗口。
        WM_DISPLAYCHANGE | WM_EXITSIZEMOVE => {
            ensure_window_visible(hwnd);
            LRESULT(0)
        }

        WM_SETTINGCHANGE if wp.0 == SPI_SETWORKAREA.0 as usize => {
            ensure_window_visible(hwnd);
            LRESULT(0)
        }

        // 主题切换、强调色变化或用户设置改动后，DWM 可能重新合成出
        // 一圈系统描边（白色或浅灰色）。这类消息是边框“莫名其妙又出现”
        // 的主要来源，重新关闭一次边框并强制刷新非客户区。
        WM_THEMECHANGED | WM_SYSCOLORCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
            refresh_dwm_border(hwnd);
            LRESULT(0)
        }

        WM_ACTIVATE => {
            // 从任务栏/托盘恢复时，焦点可能停在主窗口本身。只在这种“无效焦点”
            // 状态下把焦点交给编辑框；用户主动点按钮或滑块时不要抢走焦点。
            let activation = loword(wp.0 as u32);
            if activation != WA_INACTIVE as u16 {
                // 窗口重新获得焦点时，DWM 会重新合成一次窗口边框，
                // DWMWA_BORDER_COLOR 属性可能被重置为系统默认色，于是
                // 出现“切回来之后突然多了一条白边”。这里重新声明一次
                // 无边框属性，代价只是一次 DwmSetWindowAttribute 调用。
                disable_dwm_border(hwnd);
                let p = state_of(hwnd);
                if !p.is_null() {
                    let state = &*p;
                    let focus = GetFocus();
                    if (focus.is_invalid() || focus == hwnd) && !state.text_edit.is_invalid() {
                        let _ = SetFocus(state.text_edit);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_SETFOCUS => {
            // 点击底栏空白等区域后焦点可能落到主窗口本身。把焦点交给编辑框，
            // 普通字符输入才不会丢失；Tab 分支仍可在焦点环为空时兜底。
            let p = state_of(hwnd);
            if !p.is_null() {
                let edit = (*p).text_edit;
                if !edit.is_invalid() && IsWindowVisible(edit).as_bool() {
                    let _ = SetFocus(edit);
                }
            }
            LRESULT(0)
        }

        WM_MOVING => {
            if lp.0 != 0 {
                let rect = &mut *(lp.0 as *mut RECT);
                let p = state_of(hwnd);
                let dpi = if p.is_null() { 96 } else { (*p).dpi };
                clamp_title_bar_to_virtual_screen(rect, dpi);
            }
            LRESULT(1)
        }

        WM_GETMINMAXINFO => {
            // 先让系统填入默认的最大化尺寸/位置等字段，再覆盖最小尺寸；
            // 只写 ptMinTrackSize 而不走默认处理会留下未初始化的其余字段。
            let _ = DefWindowProcW(hwnd, msg, wp, lp);
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

        WM_NCLBUTTONDBLCLK => {
            // 自绘标题栏双击默认会触发 SC_MAXIMIZE，但这个窗口没有最大化语义，
            // 会变成怪异的最大化/还原切换。吞掉双击，保留单击拖动。
            LRESULT(0)
        }

        WM_SYSCOMMAND if (wp.0 as u32 & 0xFFF0) == SC_MAXIMIZE => LRESULT(0),

        WM_NCHITTEST => {
            let screen_pt = POINT {
                x: get_x_lparam(lp) as i32,
                y: get_y_lparam(lp) as i32,
            };
            let mut client_pt = screen_pt;
            let mut rect = RECT::default();
            // 坐标转换或客户区查询失败时不能让默认的 0 矩形参与命中测试：
            // 那样几乎所有客户区都会被判成右下角手柄。交回默认过程。
            if !ScreenToClient(hwnd, &mut client_pt).as_bool()
                || GetClientRect(hwnd, &mut rect).is_err()
            {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }

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

            // 标题栏区域：除交互控件外均可拖动。标题文字由主窗口绘制，
            // 不是子窗口，因此这里只会命中按钮和透明度滑块。
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

        WM_NCLBUTTONDOWN if wp.0 as u32 == HTBOTTOMRIGHT => {
            // 窗口没有 WS_THICKFRAME，系统的 SC_SIZE 缩放不会生效，
            // 这里自己接管右下角手柄的拖动。
            if start_resize(hwnd) {
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_LBUTTONDOWN => {
            // 兜底：某些情况下系统把右下角当成客户区，命中测试返回
            // HTBOTTOMRIGHT 但派发的是客户区消息。这里用同一套坐标
            // 判定再确认一次，保证缩放始终能启动。
            let in_grip = {
                let p = state_of(hwnd);
                if p.is_null() {
                    false
                } else {
                    let mut client = RECT::default();
                    let pt = POINT {
                        x: get_x_lparam(lp) as i32,
                        y: get_y_lparam(lp) as i32,
                    };
                    if GetClientRect(hwnd, &mut client).is_err() {
                        false
                    } else {
                        let grip = (*p).scale(14);
                        pt.x >= client.right - grip && pt.y >= client.bottom - grip
                    }
                }
            };
            if in_grip && start_resize(hwnd) {
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_MOUSEMOVE | WM_NCMOUSEMOVE
            if {
                let p = state_of(hwnd);
                !p.is_null() && (*p).resizing
            } =>
        {
            apply_resize(hwnd);
            LRESULT(0)
        }

        // 拖动期间用定时器兜底。非客户区捕获时系统不一定持续投递
        // WM_NCMOUSEMOVE，定时器保证窗口尺寸始终跟随鼠标。
        WM_TIMER if wp.0 == TIMER_RESIZE => {
            let p = state_of(hwnd);
            if !p.is_null() && (*p).resizing {
                apply_resize(hwnd);
            } else {
                let _ = KillTimer(hwnd, TIMER_RESIZE);
            }
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_NCLBUTTONUP
            if {
                let p = state_of(hwnd);
                !p.is_null() && (*p).resizing
            } =>
        {
            let p = state_of(hwnd);
            // 最后一次鼠标移动可能没有对应的 WM_MOUSEMOVE（快速松开），
            // 松开前再按当前光标位置结算一次，避免尺寸停在上一帧。
            apply_resize(hwnd);
            (*p).resizing = false;
            let _ = KillTimer(hwnd, TIMER_RESIZE);
            if GetCapture() == hwnd {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }

        WM_CAPTURECHANGED
            if {
                let p = state_of(hwnd);
                !p.is_null() && (*p).resizing
            } =>
        {
            // 失去捕获（Alt+Tab、失焦）时结束缩放，避免一直保持按下态。
            let p = state_of(hwnd);
            (*p).resizing = false;
            let _ = KillTimer(hwnd, TIMER_RESIZE);
            LRESULT(0)
        }

        WM_KEYDOWN if wp.0 as u16 == VK_TAB.0 => {
            // 主窗口拥有焦点时，Tab/Shift+Tab 应进入子控件焦点环。
            let p = state_of(hwnd);
            if !p.is_null() {
                let backward = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let _ = focus_from_frame(&*p, backward);
            }
            LRESULT(0)
        }

        WM_KEYDOWN
            if wp.0 as u16 == VK_Z.0 && (plain_ctrl_z_down() || plain_ctrl_shift_z_down()) =>
        {
            // 少数情况下焦点会停在主窗口而非子控件（例如点击空白区域后
            // 由系统把焦点交给顶层窗口），这里提供与编辑框一致的入口。
            if let Ok(p) = state_ptr(hwnd) {
                let text_edit = (*p).text_edit;
                if !text_edit.is_invalid() {
                    if plain_ctrl_shift_z_down() {
                        let _ = edit_redo(text_edit);
                    } else if !undo_clear(text_edit) {
                        let _ = edit_undo(text_edit);
                    }
                }
            }
            LRESULT(0)
        }

        WM_MOUSEWHEEL => {
            // 标题栏/底栏空白处也允许 Shift+滚轮调节透明度；子控件各自处理。
            if adjust_opacity_with_wheel(hwnd, wp) {
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_SETCURSOR => {
            // WM_SETCURSOR 不只在鼠标位于本窗口时到达：右键托盘图标弹出
            // 菜单、窗口被激活等路径也会把它发给主窗口，此时光标可能远在
            // 窗口外的托盘上。旧实现只判断“客户区坐标是否大于右下边界减
            // grip”，对窗口右下方的任意坐标都成立，于是托盘菜单出现时鼠标
            // 会被错误地设成斜向缩放光标。
            //
            // 这里必须先把光标限制在客户区矩形 [0, right) × [0, bottom)
            // 内，再判断右下角手柄；不能只看“坐标大于右/下边界减 grip”。
            // 注意不要额外要求命中代码必须是 HTCLIENT：右下角区域由
            // WM_NCHITTEST 返回 HTBOTTOMRIGHT，命中代码并非 HTCLIENT，
            // 但那里同样需要显示缩放光标。
            let p = state_of(hwnd);
            if !p.is_null() {
                let state = &*p;
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let mut client_pt = cur;
                let mut rect = RECT::default();
                // 失败时退回默认光标处理，避免整个客户区都显示缩放光标。
                if !ScreenToClient(hwnd, &mut client_pt).as_bool()
                    || GetClientRect(hwnd, &mut rect).is_err()
                {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                let inside = client_pt.x >= 0
                    && client_pt.y >= 0
                    && client_pt.x < rect.right
                    && client_pt.y < rect.bottom;
                let grip = state.scale(14);
                if inside && client_pt.x >= rect.right - grip && client_pt.y >= rect.bottom - grip {
                    let _ = SetCursor(LoadCursorW(None, IDC_SIZENWSE).unwrap_or_default());
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_ERASEBKGND => {
            // 背景统一在 WM_PAINT 中用双缓冲绘制，避免这里先闪一次底色。
            LRESULT(1)
        }

        WM_PAINT => {
            let p = state_of(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let state = &mut *p;
            let mut ps = PAINTSTRUCT::default();
            let target = BeginPaint(hwnd, &mut ps);
            if target.is_invalid() {
                // BeginPaint 失败时没有取得绘制 DC，ps 也未初始化，不能
                // 调用 EndPaint 配对。直接返回，等待系统下次投递 WM_PAINT。
                return LRESULT(0);
            }

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right;
            let h = rect.bottom;

            // 双缓冲：把整窗背景、两条栏、边框和缩放手柄一次性画完。
            // 缓冲按客户区尺寸复用；创建失败时退回直接绘制。
            let hdc = ensure_back_buffer(&mut state.back_buffer, target, w, h);
            let buffered = hdc != target;
            FillRect(hdc, &rect, state.bg_brush);

            let title_h = state.scale(28);
            let bottom_h = state.scale(20);

            // 标题栏背景
            let title_rect = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: title_h,
            };
            FillRect(hdc, &title_rect, state.bar_brush);
            // 底栏背景
            let bottom_rect = RECT {
                left: 0,
                top: h - bottom_h,
                right: w,
                bottom: h,
            };
            FillRect(hdc, &bottom_rect, state.bar_brush);
            // 分割线。标题栏这条必须画在编辑框上沿之外：编辑框从
            // y = title_h 开始，画在 title_h 的行会被子控件完全盖住。
            let old_pen = SelectObject(hdc, state.dark_pen);
            let _ = MoveToEx(hdc, 0, title_h - 1, None);
            let _ = LineTo(hdc, w, title_h - 1);
            let _ = MoveToEx(hdc, 0, h - bottom_h, None);
            let _ = LineTo(hdc, w, h - bottom_h);
            SelectObject(hdc, old_pen);

            // 标题文字直接画进离屏缓冲，和背景在同一帧输出。之前它是
            // 独立子控件，缩放时子控件重绘与父窗口不同步，文字会消失。
            if let Some(mut title_text) = title_text_rect(state, w) {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(C_TEXT));
                let old_font = if state.title_font.is_invalid() {
                    HGDIOBJ::default()
                } else {
                    SelectObject(hdc, state.title_font)
                };
                let mut buf: Vec<u16> = "悬浮文本框".encode_utf16().collect();
                let _ = DrawTextW(
                    hdc,
                    &mut buf,
                    &mut title_text,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE,
                );
                if !old_font.is_invalid() {
                    SelectObject(hdc, old_font);
                }
            }

            // 右下角缩放手柄 (两条对角小线)
            let grip = state.scale(14);
            let old_gpen = SelectObject(hdc, state.grip_pen);
            let _ = MoveToEx(hdc, w - grip + state.scale(2), h - state.scale(3), None);
            let _ = LineTo(hdc, w - state.scale(3), h - grip + state.scale(2));
            let _ = MoveToEx(hdc, w - grip + state.scale(6), h - state.scale(3), None);
            let _ = LineTo(hdc, w - state.scale(3), h - grip + state.scale(6));
            SelectObject(hdc, old_gpen);

            if buffered {
                let _ = BitBlt(target, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
            }

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
            let hovered = state.hovered_btn == dis.hwndItem;

            let (bg, text_color, label, font) = match id {
                ID_PIN_BTN => (
                    if selected || hovered {
                        state.btn_hover_brush
                    } else {
                        state.bar_brush
                    },
                    if state.topmost {
                        COLORREF(C_ACCENT)
                    } else {
                        COLORREF(C_TEXT)
                    },
                    "顶",
                    state.symbol_font,
                ),
                ID_MIN_BTN => (
                    if selected || hovered {
                        state.btn_hover_brush
                    } else {
                        state.bar_brush
                    },
                    COLORREF(C_TEXT),
                    "—",
                    state.symbol_font,
                ),
                ID_CLOSE_BTN => (
                    if selected {
                        state.btn_close_brush
                    } else if hovered {
                        state.btn_hover_brush
                    } else {
                        state.bar_brush
                    },
                    if selected {
                        COLORREF(0x00FFFFFF)
                    } else {
                        COLORREF(C_TEXT)
                    },
                    "✕",
                    state.symbol_font,
                ),
                ID_CLEAR_BTN => (
                    if selected {
                        state.bar_dark_brush
                    } else if hovered {
                        state.btn_hover_brush
                    } else {
                        state.bar_brush
                    },
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
            let _ = DrawTextW(
                hdc,
                &mut buf,
                &mut rr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(hdc, old_font);
            // ODS_FOCUS 在部分 owner-draw 场景下不一定可靠；用实际焦点再兜底，
            // 保证键盘用户始终能看到焦点框。
            let focused = dis.itemState.0 & ODS_FOCUS.0 != 0 || GetFocus() == dis.hwndItem;
            if focused {
                let mut focus_rect = r;
                focus_rect.left += 2;
                focus_rect.top += 2;
                focus_rect.right -= 2;
                focus_rect.bottom -= 2;
                let _ = DrawFocusRect(hdc, &focus_rect);
            }
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
            if ctl == state.count_label {
                SetTextColor(hdc, COLORREF(C_TEXT_MUTED));
                // 计数文字会变短（例如 1000 -> 0），必须用不透明背景
                // 覆盖旧字形；透明绘制会留下残影。
                SetBkMode(hdc, OPAQUE);
                SetBkColor(hdc, COLORREF(C_BAR));
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
                let slider = (*p).opacity_slider;
                if !slider.is_invalid() && HWND(lp.0 as *mut _) == slider {
                    let pos = send(slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32;
                    // 缓存只由 apply_opacity_alpha 在 API 成功后更新。
                    apply_opacity_alpha(hwnd, pos);
                }
            }
            LRESULT(0)
        }

        WM_COMMAND => {
            let id = loword(wp.0 as u32);
            let notif = hiword(wp.0 as u32);
            match id {
                ID_PIN_BTN if notif == BN_CLICKED as u16 => {
                    toggle_topmost(hwnd);
                }
                ID_MIN_BTN if notif == BN_CLICKED as u16 => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                ID_CLOSE_BTN if notif == BN_CLICKED as u16 => close_or_hide(hwnd),
                ID_CLEAR_BTN if notif == BN_CLICKED as u16 => clear_text(hwnd),
                ID_TEXT_EDIT if notif == EN_CHANGE as u16 => {
                    let p = state_of(hwnd);
                    if !p.is_null() {
                        // 记录多级撤销/重做历史。清空、恢复清空以及撤销/重做
                        // 自身的程序化修改由 suppress 标记排除。
                        record_edit_change((*p).text_edit);
                        // 只查长度，不在每次按键时复制整段文本。清空后一旦重新输入，
                        // 旧快照立即失效；文本仍为空时保留快照等待 Ctrl+Z。
                        let length = GetWindowTextLengthW((*p).text_edit);
                        if (*p).cleared_snapshot.is_some() && length > 0 {
                            (*p).cleared_snapshot = None;
                        }
                        if length > 0 && (*p).clear_notice_active {
                            // 一旦开始重新输入，"Ctrl+Z 撤销"就不再成立。
                            let _ = KillTimer(hwnd, TIMER_CLEAR_NOTICE);
                            (*p).clear_notice_active = false;
                        }
                        // 正文变化会影响滑块位置和长度，刷新滚动条状态。
                        attach_scroll_bar(p);
                        // 文本长度变化会改变滑块的位置和长度，必须重画。
                        // 注意：必须放在 p 非空的分支里。原来的写法把
                        // (*p).text_edit 放在了判空之外，一旦 GWLP_USERDATA
                        // 已经被清零（WM_NCDESTROY 之后仍有消息到达的极端
                        // 情况），这里会直接解引用空指针而崩溃。
                        invalidate_scroll_bar((*p).text_edit);
                    }
                    if SetTimer(hwnd, TIMER_UPDATE_COUNT, 120, None) == 0 {
                        update_count(hwnd);
                    }
                }
                ID_TEXT_EDIT if notif == EN_VSCROLL as u16 => {
                    // 编辑框滚动时会通知父窗口。键盘翻页（PgUp/PgDn）、
                    // 方向键、Home/End 都会走这条路，此时正文长度没变，
                    // EN_CHANGE 不会触发，自绘滑块的位置就得不到更新。
                    // 不处理的话滑块会停在旧位置，与正文脱节。
                    let p = state_of(hwnd);
                    if !p.is_null() {
                        invalidate_scroll_bar((*p).text_edit);
                    }
                }
                IDM_TOGGLE_TOP => toggle_topmost(hwnd),
                IDM_CLEAR => clear_text(hwnd),
                IDM_SHOW => show_window(hwnd),
                IDM_HIDE => {
                    let _ = hide_to_tray(hwnd);
                }
                // 退出不弹确认框：正文不持久化是明确的取舍，用户主动退出
                // 即视为放弃当前正文。
                IDM_QUIT => quit_app(hwnd),
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            LRESULT(0)
        }

        WM_TRAYICON => {
            // v4 回调：LOWORD(lParam) 是通知事件，HIWORD(lParam) 是图标 ID。
            // v4 回调中 LOWORD(lParam) 才是通知事件；不能用整个 lParam
            // 去匹配鼠标消息，否则永远匹配不到。
            let event = loword(lp.0 as u32) as u32;
            match event {
                // 这里的 `|` 是 Rust 的“或模式”，不是按位或：NIN_SELECT(1024)
                // 是鼠标单击托盘图标，NIN_KEYSELECT(1025) 是键盘选择，
                // 两者都要走同一条显示/隐藏路径。若误写成按位或，
                // 结果会等于 1025，鼠标单击托盘图标将完全没有反应。
                NIN_SELECT | NIN_KEYSELECT => {
                    // 托盘单击按“当前可见性”切换。不能要求窗口已是前台：
                    // 窗口可见但位于其他窗口后面时，用户单击托盘图标的意图
                    // 仍是隐藏，而不是先提到前台。
                    if IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
                        let _ = hide_to_tray(hwnd);
                    } else {
                        show_window(hwnd);
                    }
                }
                WM_CONTEXTMENU => show_tray_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }

        WM_SHOW_AND_FOCUS => {
            show_window(hwnd);
            LRESULT(0)
        }

        WM_TIMER if wp.0 == TIMER_UPDATE_COUNT => {
            let _ = KillTimer(hwnd, TIMER_UPDATE_COUNT);
            // 清空提示存在时不能被延迟计数覆盖；提示定时器结束后会
            // 自己调用 update_count。
            let p = state_of(hwnd);
            let notice_visible = !p.is_null() && (*p).clear_notice_active;
            if !notice_visible {
                update_count(hwnd);
            }
            LRESULT(0)
        }

        WM_TIMER if wp.0 == TIMER_CLEAR_NOTICE => {
            let _ = KillTimer(hwnd, TIMER_CLEAR_NOTICE);
            let p = state_of(hwnd);
            if !p.is_null() {
                (*p).clear_notice_active = false;
            }
            // 没有正文却有快照时 update_count 会显示“字符: 0”，与本函数
            // 结束提示的语义相同；此时无需再读一次编辑框。
            update_count(hwnd);
            LRESULT(0)
        }

        WM_CLOSE => {
            // 托盘可用时关闭 = 隐藏；否则直接退出，避免留下不可见后台进程。
            close_or_hide(hwnd);
            LRESULT(0)
        }

        WM_DESTROY => {
            let p = state_of(hwnd);
            if !p.is_null() {
                // 不依赖 tray_added：NIM_ADD 成功但 NIM_SETVERSION 失败时，
                // 代码会尝试撤销图标；若那次撤销也失败，tray_added 已为 false，
                // 仅在 true 时删除会留下一个直到鼠标经过才消失的陈旧图标。
                // NIM_DELETE 对不存在的图标是安全的，退出时统一调用。
                remove_tray_icon(&*p);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_NCDESTROY => {
            let p = state_of(hwnd);
            if !p.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                // 先摘掉所有子类化过程，再释放 AppState 持有的资源。
                // 顺序反过来的话，子类回调可能在状态已释放后被调用，
                // 访问到已释放的 ScrollBarPaint。
                detach_scroll_bar(p);
                let mut state = Box::from_raw(p);
                if let Some(mut buffer) = state.back_buffer.take() {
                    buffer.release();
                }
                if !state.text_edit.is_invalid() {
                    let _ = RemoveWindowSubclass(state.text_edit, Some(text_edit_subclass_proc), 1);
                }
                if !state.opacity_slider.is_invalid() {
                    let _ =
                        RemoveWindowSubclass(state.opacity_slider, Some(slider_subclass_proc), 1);
                }
                for button in [
                    state.pin_btn,
                    state.min_btn,
                    state.close_btn,
                    state.clear_btn,
                ] {
                    if !button.is_invalid() {
                        let _ = RemoveWindowSubclass(button, Some(button_subclass_proc), 1);
                    }
                }
                for brush in [
                    state.bg_brush,
                    state.bar_brush,
                    state.bar_dark_brush,
                    state.btn_hover_brush,
                    state.btn_close_brush,
                ] {
                    if !brush.is_invalid() {
                        let _ = DeleteObject(brush);
                    }
                }
                // 自绘滚动条的画刷单独持有，需要在状态释放时清理。
                state.scroll_paint.release();
                for pen in [state.dark_pen, state.grip_pen] {
                    if !pen.is_invalid() {
                        let _ = DeleteObject(pen);
                    }
                }
                for font in [
                    state.text_font,
                    state.title_font,
                    state.small_font,
                    state.symbol_font,
                ] {
                    if !font.is_invalid() {
                        let _ = DeleteObject(font);
                    }
                }
                for icon in [state.app_icon, state.tray_icon] {
                    if !icon.is_invalid() {
                        let _ = DestroyIcon(icon);
                    }
                }
                for icon in state.retired_icons.drain(..) {
                    if !icon.is_invalid() {
                        let _ = DestroyIcon(icon);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---- 工具函数 (LOWORD/HIWORD/MAKELONG 等价实现) ----
fn loword(l: u32) -> u16 {
    (l & 0xFFFF) as u16
}
fn hiword(l: u32) -> u16 {
    ((l >> 16) & 0xFFFF) as u16
}
fn makelong(a: u16, b: u16) -> u32 {
    (a as u32) | ((b as u32) << 16)
}

// GET_X_LPARAM / GET_Y_LPARAM 等价实现
fn get_x_lparam(lp: LPARAM) -> i16 {
    (lp.0 as u32 as u16) as i16
}
fn get_y_lparam(lp: LPARAM) -> i16 {
    ((lp.0 as u32 >> 16) as u16) as i16
}

// 加速键表创建失败时的兜底。只处理本窗口（含其子控件）的按键，并过滤
// 自动重复，避免按住 Ctrl+Q 时反复投递退出命令。与加速键表的实际行为
// 一致：拒绝 Shift、Alt 组合（含 AltGr），只接受精确的 Ctrl+Q/L/T。
unsafe fn handle_accel_fallback(hwnd: HWND, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN || msg.wParam.0 == 0 {
        return false;
    }
    if GetAncestor(msg.hwnd, GA_ROOT) != hwnd {
        return false;
    }
    if (msg.lParam.0 & (1 << 30)) != 0 {
        return false;
    }
    if GetKeyState(VK_CONTROL.0 as i32) >= 0 || extra_accel_modifier_down() {
        return false;
    }
    let id = match msg.wParam.0 as u16 {
        vk if vk == b'Q' as u16 => IDM_QUIT,
        vk if vk == b'L' as u16 => IDM_CLEAR,
        vk if vk == b'T' as u16 => IDM_TOGGLE_TOP,
        _ => return false,
    };
    PostMessageW(hwnd, WM_COMMAND, WPARAM(id as usize), LPARAM(0)).is_ok()
}

// 加速键表路径也会收到按住不放产生的重复 WM_KEYDOWN。Ctrl+T 若不拦截，
// 会连续切换置顶状态；Ctrl+L 会反复重置清空提示。只吞掉精确的 Ctrl+Q/L/T
// 重复消息，首次按键仍由 TranslateAcceleratorW 正常派发。
unsafe fn is_shortcut_autorepeat(hwnd: HWND, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN || (msg.lParam.0 & (1 << 30)) == 0 {
        return false;
    }
    if GetAncestor(msg.hwnd, GA_ROOT) != hwnd {
        return false;
    }
    if GetKeyState(VK_CONTROL.0 as i32) >= 0 || extra_accel_modifier_down() {
        return false;
    }
    let key = msg.wParam.0 as u16;
    key == b'Q' as u16 || key == b'L' as u16 || key == b'T' as u16
}

// 加速键表只描述“必须按下哪些修饰键”，不同 Windows 实现对额外 Shift/Alt/Win
// 的处理并不一致。这里在翻译前统一拦下带额外修饰键的 Ctrl+Q/L/T，把消息交给
// 当前控件；否则 Ctrl+Shift+Q、AltGr+Q、Ctrl+Win+Q 等组合可能在加速键路径
// 误触发退出、清空或置顶，与 handle_accel_fallback 的精确过滤不一致。
// Alt 组合产生的是 WM_SYSKEYDOWN，也必须在这里检查，不能只过滤 WM_KEYDOWN。
unsafe fn is_accelerator_with_extra_modifier(hwnd: HWND, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    if GetAncestor(msg.hwnd, GA_ROOT) != hwnd {
        return false;
    }
    if GetKeyState(VK_CONTROL.0 as i32) >= 0 {
        return false;
    }
    if !extra_accel_modifier_down() {
        return false;
    }
    let key = msg.wParam.0 as u16;
    key == b'Q' as u16 || key == b'L' as u16 || key == b'T' as u16
}

// Shift/Alt 会让加速键产生额外语义；Win 组合是系统保留键，不能让窗口加速键
// 截获。AltGr 在部分布局下会同时置位 Ctrl+Alt，也被这个检查排除。
fn extra_accel_modifier_down() -> bool {
    unsafe {
        GetKeyState(VK_SHIFT.0 as i32) < 0
            || GetKeyState(VK_MENU.0 as i32) < 0
            || GetKeyState(VK_LWIN.0 as i32) < 0
            || GetKeyState(VK_RWIN.0 as i32) < 0
    }
}

// 自绘滚动条的窗口类过程。正常路径下所有绘制和交互都由安装在该窗口上的
// scroll_bar_subclass_proc 接管；这里只作为子类化安装失败时的兜底：
// 至少把整块区域涂成编辑区背景色，不留下白色或未初始化像素。
unsafe extern "system" fn scroll_bar_class_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_invalid() {
                let mut rect = RECT::default();
                if GetClientRect(hwnd, &mut rect).is_ok() {
                    let brush = CreateSolidBrush(COLORREF(C_SCROLL_TRACK));
                    if !brush.is_invalid() {
                        FillRect(hdc, &rect, brush);
                        let _ = DeleteObject(brush);
                    }
                }
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    // DPI 感知 (清单已声明 PerMonitor V2，此处兜底)
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let hinst = unsafe { GetModuleHandleW(None).unwrap_or_default() };

    let _ = TASKBAR_CREATED_MSG.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

    // 单实例：若同类窗口已存在，激活它后直接退出。
    unsafe {
        if let Ok(existing) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
            let _ = ShowWindowAsync(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            let _ = PostMessageW(existing, WM_SHOW_AND_FOCUS, WPARAM(0), LPARAM(0));
            return;
        }
    }

    // 注册窗口类
    let wc = WNDCLASSW {
        // 双击标题栏依赖 WM_NCLBUTTONDBLCLK；没有 CS_DBLCLKS 时系统
        // 不会生成该消息，双击只会被当作两次单击。
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        // WM_ERASEBKGND 已完全接管背景绘制；不使用类背景画刷，
        // 避免为进程生命周期创建一个永远不会被绘制的 GDI 对象。
        hbrBackground: HBRUSH::default(),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        unsafe {
            let _ = MessageBoxW(
                None,
                w!("注册窗口类失败，程序无法启动。"),
                w!("悬浮文本框"),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    }

    // 自绘滚动条使用独立窗口类，完全绕开系统 SCROLLBAR 类的主题绘制、
    // 默认命中测试和键盘行为。
    let scroll_wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(scroll_bar_class_proc),
        hInstance: hinst.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        lpszClassName: SCROLLBAR_CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&scroll_wc) } == 0 {
        unsafe {
            let _ = MessageBoxW(
                None,
                w!("注册滚动条窗口类失败，程序无法启动。"),
                w!("悬浮文本框"),
                MB_OK | MB_ICONERROR,
            );
        }
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

    // 创建主窗口 (无边框 + 分层；缩放由 WM_NCHITTEST 自定义处理)。
    // 初始尺寸必须先按系统 DPI 计算，否则 WM_CREATE 之前会先以 96 DPI 的
    // 280x200 出现一帧；高 DPI 下首次布局、圆角和命中测试都会短暂错位。
    let initial_dpi = unsafe { GetDpiForSystem() }.max(96);
    let initial_w = 280 * initial_dpi as i32 / 96;
    let initial_h = 200 * initial_dpi as i32 / 96;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_LAYERED,
            CLASS_NAME,
            w!("悬浮文本框"),
            // 不使用 WS_THICKFRAME：该样式会让 DWM 在窗口外侧合成一层系统
            // 边框，失焦或从托盘恢复时表现为一条突兀的白色/灰色描边。
            // 缩放能力由 WM_NCHITTEST 返回 HTBOTTOMRIGHT 自行实现，
            // 不依赖系统可调整边框。
            WS_POPUP | WS_CLIPCHILDREN | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            initial_w,
            initial_h,
            None,
            None,
            hinst,
            None,
        )
    };
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(_) => {
            unsafe {
                let _ = MessageBoxW(
                    None,
                    w!("创建主窗口失败，程序无法启动。"),
                    w!("悬浮文本框"),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
        }
    };

    unsafe {
        // 放在显示之前：让 DWM 在第一次合成前就知道不要画边框，避免
        // 出现“先画白边、再消失”的闪烁。顺序反了就会先显示一帧白边。
        disable_dwm_border(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Ok(p) = state_ptr(hwnd) {
            let state = &*p;
            if !state.text_edit.is_invalid() {
                let _ = SetFocus(state.text_edit);
            }
        }
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
        CreateAcceleratorTableW(&accels).ok()
    };

    // 消息循环
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if is_shortcut_autorepeat(hwnd, &msg) {
                continue;
            }
            if is_accelerator_with_extra_modifier(hwnd, &msg) {
                // 不让 TranslateAccelerator 匹配，但仍按普通按键派发，
                // 避免把组合键吞掉而破坏输入法或控件自身行为。
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                continue;
            }
            let translated = match haccel {
                Some(table) => TranslateAcceleratorW(hwnd, table, &msg) != 0,
                None => handle_accel_fallback(hwnd, &msg),
            };
            if !translated {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(table) = haccel {
            let _ = DestroyAcceleratorTable(table);
        }
    }
}
