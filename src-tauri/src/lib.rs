pub mod clipboard;
mod clipboard_watch;
#[cfg(target_os = "linux")]
mod clipboard_x11;
mod commands;
mod paste;
mod paths;
mod platform;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use uclip_core::{AppError, CoreService};

static EXIT_STARTED: AtomicBool = AtomicBool::new(false);

pub(crate) struct AppState {
    core: Result<CoreService, AppError>,
    clipboard: Result<clipboard::ClipboardService, AppError>,
}

/// Navigation is restricted independently from CSP/ACL. Never load clipboard HTML as a document.
fn local_navigation(url: &url::Url) -> bool {
    let local = match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost") && url.port().is_none(),
        _ => false,
    };
    let development = cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("localhost")
        && url.port() == Some(1420);
    (local || development) && url.username().is_empty() && url.password().is_none()
}

pub fn run() {
    let app = tauri::Builder::default()
        // Upstream requires single-instance to be registered before every other plugin.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                if window
                    .show()
                    .and_then(|_| window.unminimize())
                    .and_then(|_| window.set_focus())
                    .is_err()
                {
                    eprintln!("desktop_focus_failed");
                }
            }
        }))
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::get_settings,
            commands::update_settings,
            commands::history_query,
            commands::clip_detail,
            commands::clipboard_status,
            commands::update_capture_policy,
            commands::copy_clip,
            commands::change_clip_deletion,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let directory = paths::data_dir(&handle);
            // All disk work and migrations happen off the event loop, before exposing the UI.
            tauri::async_runtime::spawn(async move {
                let startup = async {
                    let directory = directory?;
                    let prepared = directory.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        paths::prepare_directory(&prepared)
                    })
                    .await
                    .map_err(|_| uclip_core::AppError::new(uclip_core::ErrorCode::Unavailable))??;
                    let core = CoreService::open(directory.join("foundation.sqlite3")).await?;
                    Ok::<_, uclip_core::AppError>(core)
                }
                .await;
                if let Err(ref error) = startup {
                    eprintln!("startup_failed: {:?}: {}", error.code, error.message);
                }
                // Even a failed database startup gets a visible, safe error in the local UI.
                let clipboard = match &startup {
                    Ok(core) => {
                        let core = core.clone();
                        let app = handle.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            clipboard::ClipboardService::start(core, Some(app))
                        })
                        .await
                        .unwrap_or_else(|_| Err(AppError::new(uclip_core::ErrorCode::Unavailable)))
                    }
                    Err(error) => Err(error.clone()),
                };
                handle.manage(AppState {
                    core: startup,
                    clipboard,
                });
                // Creating a WebView in an async task avoids Windows synchronous-handler deadlocks.
                let result = WebviewWindowBuilder::new(
                    &handle,
                    "main",
                    WebviewUrl::App("index.html".into()),
                )
                .title("UClip")
                .inner_size(640.0, 650.0)
                .min_inner_size(480.0, 390.0)
                .on_navigation(local_navigation)
                .build();
                if result.is_err() {
                    eprintln!("desktop_window_creation_failed");
                    handle.exit(1);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to initialize desktop runtime");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // app.exit() is the explicit final exit; ordinary window closure drains storage first.
            if code.is_none() {
                api.prevent_exit();
                if !EXIT_STARTED.swap(true, Ordering::SeqCst) {
                    let app = handle.clone();
                    let core = handle
                        .try_state::<AppState>()
                        .and_then(|state| state.core.as_ref().ok().cloned());
                    tauri::async_runtime::spawn(async move {
                        let clipboard = app
                            .try_state::<AppState>()
                            .and_then(|state| state.clipboard.as_ref().ok().cloned());
                        let clip_failed = if let Some(clipboard) = clipboard {
                            clipboard.stop().await.is_err()
                        } else {
                            false
                        };
                        let core_failed = if let Some(core) = core {
                            core.shutdown().await.is_err()
                        } else {
                            false
                        };
                        let failed = clip_failed || core_failed;
                        if failed {
                            eprintln!("storage_shutdown_failed");
                        }
                        app.exit(if failed { 1 } else { 0 });
                    });
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_rejects_remote_origins_and_lookalike_hosts() {
        for value in [
            "https://tauri.evil.com",
            "https://tauri.localhost.evil.com",
            "file:///tmp/a.html",
            "javascript:alert(1)",
            "https://example.com",
            "http://localhost:1421",
        ] {
            assert!(!local_navigation(&value.parse().unwrap()), "{value}");
        }
        assert!(local_navigation(
            &"tauri://localhost/index.html".parse().unwrap()
        ));
        assert!(local_navigation(
            &"http://tauri.localhost/index.html".parse().unwrap()
        ));
    }
}
