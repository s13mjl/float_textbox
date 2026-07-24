use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WebviewWindow, WindowEvent,
};

/// 切换置顶状态，返回新状态
#[tauri::command]
fn toggle_topmost(window: WebviewWindow) -> Result<bool, String> {
    let current = window.is_always_on_top().map_err(|e| e.to_string())?;
    window.set_always_on_top(!current).map_err(|e| e.to_string())?;
    Ok(!current)
}

/// 触发清空文本（通过事件通知前端）
#[tauri::command]
fn clear_text(window: WebviewWindow) -> Result<(), String> {
    window.emit("clear-text", ()).map_err(|e| e.to_string())
}

/// 退出应用
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// 显示窗口并聚焦
#[tauri::command]
fn show_window(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

/// 隐藏窗口
#[tauri::command]
fn hide_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let _window =
                app.get_webview_window("main").expect("main window missing");

            // ----- 托盘菜单 -----
            let show_item = MenuItem::with_id(app, "show", "显示", true, None::<&str>)?;
            let hide_item = MenuItem::with_id(app, "hide", "隐藏", true, None::<&str>)?;
            let pin_item = MenuItem::with_id(app, "pin", "切换置顶", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&show_item, &hide_item, &sep1, &pin_item, &sep2, &quit_item],
            )?;

            // ----- 托盘图标 -----
            let icon = app
                .default_window_icon()
                .cloned()
                .expect("default window icon missing");
            TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .tooltip("悬浮文本框")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "pin" => {
                        if let Some(w) = app.get_webview_window("main") {
                            if let Ok(cur) = w.is_always_on_top() {
                                let _ = w.set_always_on_top(!cur);
                                let _ = w.emit("topmost-changed", !cur);
                            }
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 拦截关闭，改为隐藏到托盘
                api.prevent_close();
                let _ = window.hide();
                let _ = window.emit("hidden-to-tray", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            toggle_topmost,
            clear_text,
            quit_app,
            show_window,
            hide_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
