use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // 嵌入 Windows 清单 (Common Controls v6 + PerMonitor V2 DPI)
        embed_manifest(new_manifest("FloatTextBox")).expect("无法嵌入 Windows 清单文件");

        // 嵌入 exe 图标资源
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("FileDescription", "悬浮文本框");
        res.set("ProductName", "Float TextBox");
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=无法嵌入图标资源: {}", e);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app.ico");
}
