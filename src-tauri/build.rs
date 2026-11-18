fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=test-manifest.xml");
        // Tauri 的默认资源只链接应用二进制；库测试链入 Wry 时也需要同一清单。
        embed_resource::compile_for_everything("test-manifest.rc", embed_resource::NONE)
            .manifest_required()
            .expect("嵌入 Windows Common Controls v6 清单");
    }
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attributes).expect("tauri-build 失败");
}
