use tauri::State;
use uclip_core::{
    AppError, BootstrapSnapshot, SettingsSnapshot, UpdateSettings, CONTRACT_VERSION, SCHEMA_VERSION,
};

use crate::{paths::DATA_PROFILE, platform, AppState};

#[tauri::command]
pub async fn bootstrap(state: State<'_, AppState>) -> Result<BootstrapSnapshot, AppError> {
    let core = state.core.as_ref().map_err(Clone::clone)?;
    Ok(BootstrapSnapshot {
        contract_version: CONTRACT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        data_profile: DATA_PROFILE.into(),
        schema_version: SCHEMA_VERSION,
        settings: core.settings().await?,
        platform: platform::report(),
    })
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsSnapshot, AppError> {
    let core = state.core.as_ref().map_err(Clone::clone)?;
    core.settings().await
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    request: UpdateSettings,
) -> Result<SettingsSnapshot, AppError> {
    let core = state.core.as_ref().map_err(Clone::clone)?;
    core.update_settings(request).await
}

#[tauri::command]
pub async fn history_query(
    state: State<'_, AppState>,
    request: uclip_core::HistoryQuery,
) -> Result<uclip_core::HistoryPage, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .history(request)
        .await
}
#[tauri::command]
pub async fn clip_detail(
    state: State<'_, AppState>,
    id: String,
    thumbnail: bool,
) -> Result<uclip_core::ClipDetail, AppError> {
    static DETAIL_BUDGET: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _permit = DETAIL_BUDGET
        .try_acquire()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Busy))?;
    let payload = state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .read_clip(id.clone())
        .await?;
    tauri::async_runtime::spawn_blocking(move || crate::clipboard::detail(id, payload, thumbnail))
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
}
#[tauri::command]
pub async fn clipboard_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<uclip_core::ClipboardStatus, AppError> {
    match &state.clipboard {
        Ok(service) => {
            let mut status = service.status().await?;
            status.shortcut_available = state_desktop_shortcut(&app);
            Ok(status)
        }
        Err(error) => {
            let core = state.core.as_ref().map_err(Clone::clone)?;
            Ok(uclip_core::ClipboardStatus {
                policy: core.capture_policy().await?,
                history_revision: core.history_revision().await?,
                available: false,
                error: Some(error.clone()),
                paste_available: false,
                paste_target: false,
                shortcut_available: false,
            })
        }
    }
}
#[tauri::command]
pub async fn update_capture_policy(
    state: State<'_, AppState>,
    request: uclip_core::UpdateCapturePolicy,
) -> Result<uclip_core::CapturePolicy, AppError> {
    match &state.clipboard {
        Ok(service) => service.update_policy(request).await,
        Err(error) => {
            if !request.enabled {
                state
                    .core
                    .as_ref()
                    .map_err(Clone::clone)?
                    .update_capture_policy(request)
                    .await
            } else {
                Err(error.clone())
            }
        }
    }
}
#[tauri::command]
pub async fn copy_clip(
    state: State<'_, AppState>,
    id: String,
    paste: bool,
) -> Result<uclip_core::CopyResult, AppError> {
    state
        .clipboard
        .as_ref()
        .map_err(Clone::clone)?
        .copy(id, paste)
        .await
}

#[tauri::command]
pub async fn change_clip_deletion(
    state: State<'_, AppState>,
    request: uclip_core::ChangeClipDeletion,
) -> Result<uclip_core::ClipDeletionSnapshot, AppError> {
    match &state.clipboard {
        Ok(service) => service.change_deletion(request).await,
        Err(_) => {
            state
                .core
                .as_ref()
                .map_err(Clone::clone)?
                .change_clip_deletion(request)
                .await
        }
    }
}

fn state_desktop_shortcut(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;
    app.state::<crate::desktop::DesktopState>()
        .shortcut_available
        .load(std::sync::atomic::Ordering::SeqCst)
}

#[tauri::command]
pub async fn change_clip_pin(
    state: State<'_, AppState>,
    request: uclip_core::ChangeClipPin,
) -> Result<uclip_core::ClipPinSnapshot, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .change_clip_pin(request)
        .await
}

#[tauri::command]
pub async fn deduplicate_pinned(
    state: State<'_, AppState>,
    request: uclip_core::DeduplicatePinned,
) -> Result<uclip_core::DeduplicationSnapshot, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .deduplicate_pinned(request)
        .await
}

#[tauri::command]
pub async fn desktop_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<uclip_core::DesktopStatus, AppError> {
    use tauri::Manager;
    let preferences = state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .desktop_preferences()
        .await?;
    let autostart = app.clone();
    let enabled =
        tauri::async_runtime::spawn_blocking(move || crate::autostart::enabled(&autostart))
            .await
            .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?;
    let desktop = app.state::<crate::desktop::DesktopState>();
    Ok(uclip_core::DesktopStatus {
        keyboard: state
            .core
            .as_ref()
            .map_err(Clone::clone)?
            .keyboard_bindings()
            .await?,
        preferences,
        tray_available: desktop
            .tray_available
            .load(std::sync::atomic::Ordering::SeqCst),
        shortcut_available: state_desktop_shortcut(&app),
        autostart_enabled: enabled.as_ref().ok().copied(),
        error: enabled.err(),
    })
}

struct DesktopMutation<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for DesktopMutation<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tauri::command]
pub async fn update_desktop_preferences(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: uclip_core::UpdateDesktopPreferences,
) -> Result<uclip_core::DesktopPreferences, AppError> {
    use uclip_core::ErrorCode;
    use {
        std::sync::atomic::Ordering, tauri::Manager,
        tauri_plugin_global_shortcut::GlobalShortcutExt,
    };
    let desktop = app.state::<crate::desktop::DesktopState>();
    if desktop
        .updating
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::new(ErrorCode::Busy));
    }
    let _reservation = DesktopMutation(&desktop.updating);
    let core = state.core.as_ref().map_err(Clone::clone)?;
    let previous = core.desktop_preferences().await?;
    if previous.revision != request.expected_revision {
        return Err(AppError::new(ErrorCode::Conflict));
    }
    let next_shortcut = crate::desktop::parse_shortcut(&request.shortcut)?;
    let old_shortcut = crate::desktop::parse_shortcut(&previous.shortcut)?;
    let was_registered = desktop.shortcut_available.load(Ordering::SeqCst);
    let change_shortcut = next_shortcut != old_shortcut || !was_registered;
    let mut newly_registered = false;
    if change_shortcut {
        let handle = app.clone();
        let value = request.shortcut.clone();
        let registered = tauri::async_runtime::spawn_blocking(move || {
            crate::desktop::register_shortcut(&handle, &value)
        })
        .await
        .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        match registered {
            Ok(()) => newly_registered = true,
            Err(error) if next_shortcut != old_shortcut => return Err(error),
            Err(_) => {}
        }
    }
    if (request.close_to_tray || request.auto_hide) && !desktop.recoverable() && !newly_registered {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::new(ErrorCode::Unavailable))?;
    let operation = async {
        if request.native_titlebar != previous.native_titlebar {
            window
                .set_decorations(request.native_titlebar)
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        }
        window
            .set_always_on_top(request.always_on_top)
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        core.update_desktop_preferences(request).await
    }
    .await;
    match operation {
        Ok(saved) => {
            desktop.cache(saved.clone());
            desktop
                .shortcut_available
                .store(was_registered || newly_registered, Ordering::SeqCst);
            if change_shortcut && was_registered && next_shortcut != old_shortcut {
                let handle = app.clone();
                let removed = tauri::async_runtime::spawn_blocking(move || {
                    handle.global_shortcut().unregister(old_shortcut)
                })
                .await;
                // A failed unregister must be visible; disable its callback by checking the current preference below.
                if !removed.is_ok_and(|r| r.is_ok()) {
                    eprintln!("desktop_shortcut_cleanup_failed");
                }
            }
            Ok(saved)
        }
        Err(error) => {
            let _ = window.set_decorations(previous.native_titlebar);
            let _ = window.set_always_on_top(previous.always_on_top);
            if newly_registered {
                let handle = app.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    handle.global_shortcut().unregister(next_shortcut)
                })
                .await;
            }
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, AppError> {
    tauri::async_runtime::spawn_blocking(move || crate::autostart::set(&app, enabled))
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
}

#[tauri::command]
pub fn set_desktop_interaction(app: tauri::AppHandle, active: bool) {
    use tauri::Manager;
    app.state::<crate::desktop::DesktopState>()
        .protected
        .store(active, std::sync::atomic::Ordering::SeqCst);
}
#[tauri::command]
pub async fn hide_main_window(app: tauri::AppHandle) -> Result<(), AppError> {
    use tauri::Manager;
    if !app.state::<crate::desktop::DesktopState>().recoverable() {
        return Err(AppError::new(uclip_core::ErrorCode::Restricted));
    }
    app.get_webview_window("main")
        .ok_or_else(|| AppError::new(uclip_core::ErrorCode::Unavailable))?
        .hide()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))
}

#[tauri::command]
pub async fn minimize_main_window(window: tauri::WebviewWindow) -> Result<(), AppError> {
    window
        .minimize()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))
}

#[tauri::command]
pub async fn toggle_main_window_maximized(window: tauri::WebviewWindow) -> Result<(), AppError> {
    if window
        .is_maximized()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
    {
        window.unmaximize()
    } else {
        window.maximize()
    }
    .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))
}

#[tauri::command]
pub async fn start_main_window_drag(window: tauri::WebviewWindow) -> Result<(), AppError> {
    window
        .start_dragging()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))
}

#[tauri::command]
pub async fn close_main_window(window: tauri::WebviewWindow) -> Result<(), AppError> {
    // CloseRequested decides whether to hide or drain storage and exit.
    window
        .close()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))
}
#[tauri::command]
pub async fn quit_app(app: tauri::AppHandle) {
    crate::shutdown(app);
}

#[tauri::command]
pub async fn create_text(
    state: State<'_, AppState>,
    text: String,
    collection_id: Option<String>,
) -> Result<String, AppError> {
    uclip_core::ClipPayload::Text(text.clone()).validate()?;
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .create_text_in_collection(text, collection_id)
        .await
}

#[tauri::command]
pub async fn collections_list(
    state: State<'_, AppState>,
) -> Result<uclip_core::CollectionsSnapshot, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .collections()
        .await
}

#[tauri::command]
pub async fn create_collection(
    state: State<'_, AppState>,
    request: uclip_core::CreateCollection,
) -> Result<uclip_core::CollectionItem, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .create_collection(request)
        .await
}

#[tauri::command]
pub async fn rename_collection(
    state: State<'_, AppState>,
    request: uclip_core::RenameCollection,
) -> Result<uclip_core::CollectionItem, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .rename_collection(request)
        .await
}

#[tauri::command]
pub async fn transfer_clip(
    state: State<'_, AppState>,
    request: uclip_core::TransferClip,
) -> Result<uclip_core::ClipTransferSnapshot, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .transfer_clip(request)
        .await
}

#[tauri::command]
pub async fn update_key_bindings(
    state: State<'_, AppState>,
    request: uclip_core::UpdateKeyBindings,
) -> Result<uclip_core::KeyboardSnapshot, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .update_key_bindings(request)
        .await
}
