fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "bootstrap",
            "get_settings",
            "update_settings",
            "history_query",
            "create_text",
            "clip_detail",
            "clipboard_status",
            "update_capture_policy",
            "copy_clip",
            "change_clip_deletion",
            "desktop_status",
            "update_desktop_preferences",
            "update_key_bindings",
            "set_autostart",
            "set_desktop_interaction",
            "hide_main_window",
            "quit_app",
        ]),
    ))
    .expect("failed to build desktop configuration");
}
