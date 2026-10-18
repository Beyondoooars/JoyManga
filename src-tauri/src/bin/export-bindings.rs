fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.parent().expect("workspace").join("src/bindings.ts");
    joymanga_lib::export_bindings(&target).expect("export TypeScript IPC bindings");
    println!("IPC bindings exported");
}
