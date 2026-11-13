fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "bootstrap",
            "get_settings",
            "update_settings",
            "history_query",
            "clip_detail",
            "clipboard_status",
            "update_capture_policy",
            "copy_clip",
            "change_clip_deletion",
        ]),
    ))
    .expect("failed to build desktop configuration");
}
