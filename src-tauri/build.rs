fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["bootstrap", "get_settings", "update_settings"]),
    ))
    .expect("failed to build desktop configuration");
}
