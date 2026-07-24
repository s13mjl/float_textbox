// 始终使用 Windows GUI 子系统，避免弹出黑色控制台窗口
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    float_textbox_lib::run()
}
