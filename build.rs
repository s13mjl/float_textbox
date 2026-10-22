use embed_manifest::manifest::{DpiAwareness, MaxVersionTested, SupportedOS};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // 嵌入 Windows 清单。embed-manifest 1.5 的 new_manifest 默认依赖
        // Common Controls v6；DPI 显式写成 PerMonitorV2Only，避免依赖库的默认值。
        embed_manifest(
            new_manifest("FloatTextBox")
                .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
                .dpi_awareness(DpiAwareness::PerMonitorV2Only)
                .max_version_tested(MaxVersionTested::Windows11Version22H2),
        )
        .expect("无法嵌入 Windows 清单文件");

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
