use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};
use uclip_core::{
    AppError, BootstrapSnapshot, SettingsSnapshot, UpdateSettings, CONTRACT_VERSION, SCHEMA_VERSION,
};

use crate::{paths::DATA_PROFILE, platform, AppState};

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<uclip_core::SyncStatus, AppError> {
    let sync = state.sync.as_ref().map_err(Clone::clone)?.clone();
    tauri::async_runtime::spawn_blocking(move || sync.status())
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
}

#[tauri::command]
pub async fn update_sync(
    state: State<'_, AppState>,
    request: uclip_core::UpdateSync,
) -> Result<uclip_core::SyncStatus, AppError> {
    let sync = state.sync.as_ref().map_err(Clone::clone)?.clone();
    tauri::async_runtime::spawn_blocking(move || sync.update(request))
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
}

#[tauri::command]
pub async fn sync_pairing_code(state: State<'_, AppState>) -> Result<String, AppError> {
    let sync = state.sync.as_ref().map_err(Clone::clone)?.clone();
    tauri::async_runtime::spawn_blocking(move || sync.pairing_code())
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
}

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
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: UpdateSettings,
) -> Result<SettingsSnapshot, AppError> {
    let core = state.core.as_ref().map_err(Clone::clone)?;
    let saved = core.update_settings(request).await?;
    let _ = app.emit("settings-changed", ());
    Ok(saved)
}

#[tauri::command]
pub async fn history_query(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    request: uclip_core::HistoryQuery,
    trash: Option<bool>,
    count: Option<u32>,
    search_sequence: Option<u64>,
    on_first_page: Option<tauri::ipc::JavaScriptChannelId>,
) -> Result<uclip_core::HistoryPage, AppError> {
    let core = state.core.as_ref().map_err(Clone::clone)?;
    let trash = trash.unwrap_or(false);
    if let Some(sequence) = search_sequence {
        let channel = on_first_page.map(|callback| callback.channel_on(webview));
        let latest = state.history_sequence.clone();
        latest.fetch_max(sequence, Ordering::Relaxed);
        let publish = channel.map(|channel| {
            move |page| {
                let _ = channel.send(page);
            }
        });
        core.search_history(request, trash, count, publish, move || {
            latest.load(Ordering::Relaxed) == sequence
        })
        .await
    } else {
        core.history_window(request, trash, count).await
    }
}
#[tauri::command]
pub async fn clip_detail(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    thumbnail: bool,
) -> Result<uclip_core::ClipDetail, AppError> {
    if window.label() == crate::hover_preview::LABEL
        && crate::hover_preview::current(window.app_handle())?.is_none_or(|value| value.id != id)
    {
        return Err(AppError::new(uclip_core::ErrorCode::NotFound));
    }
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
            let core = state.core.as_ref().map_err(Clone::clone)?;
            if request.enabled {
                let current = core.capture_policy().await?;
                if request.expected_revision != current.revision {
                    return Err(AppError::new(uclip_core::ErrorCode::Conflict));
                }
                if !current.enabled {
                    return Err(error.clone());
                }
            }
            core.update_capture_policy(request).await
        }
    }
}
#[tauri::command]
pub async fn copy_clip(
    state: State<'_, AppState>,
    id: String,
    ids: Option<Vec<String>>,
    history_revision: Option<String>,
    paste: bool,
    fallback_to_copy: Option<bool>,
) -> Result<uclip_core::CopyResult, AppError> {
    state
        .clipboard
        .as_ref()
        .map_err(Clone::clone)?
        .copy(
            ids.unwrap_or_else(|| vec![id]),
            paste,
            fallback_to_copy.unwrap_or(false),
            history_revision,
        )
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

#[tauri::command]
pub async fn purge_trash(
    state: State<'_, AppState>,
    request: uclip_core::PurgeTrash,
) -> Result<u32, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .purge_trash(request)
        .await
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
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        hardware_acceleration: Some(app.state::<crate::graphics::Preferences>().status()),
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        hardware_acceleration: None,
        error: enabled
            .err()
            .or_else(|| desktop.autostart_error.lock().ok()?.clone()),
    })
}

#[tauri::command]
pub async fn set_hardware_acceleration(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), AppError> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        use tauri::{Emitter, Manager};
        let writer = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            writer.state::<crate::graphics::Preferences>().save(enabled)
        })
        .await
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))??;
        let _ = app.emit("settings-changed", ());
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, enabled);
        Err(AppError::new(uclip_core::ErrorCode::Restricted))
    }
}

struct DesktopMutation<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for DesktopMutation<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tauri::command]
pub fn quick_paste_settings(
    app: tauri::AppHandle,
) -> Result<uclip_core::QuickPasteSettings, AppError> {
    crate::quick_paste::settings(&app)
}

#[tauri::command]
pub async fn update_quick_paste_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: uclip_core::UpdateQuickPastePreferences,
) -> Result<uclip_core::QuickPasteSettings, AppError> {
    use crate::{desktop, quick_paste};
    use std::sync::atomic::Ordering;
    use uclip_core::ErrorCode;

    let desktop = app.state::<desktop::DesktopState>();
    if desktop
        .updating
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::new(ErrorCode::Busy));
    }
    let _reservation = DesktopMutation(&desktop.updating);
    let core = state.core.as_ref().map_err(Clone::clone)?;
    let existing = quick_paste::settings(&app)?;
    if existing.preferences.revision != request.expected_revision {
        return Err(AppError::new(ErrorCode::Conflict));
    }
    let quick = app.state::<quick_paste::QuickPasteState>();
    let include_pinned_only = request.shortcut == existing.preferences.shortcut
        && request.include_pinned != existing.preferences.include_pinned;
    if include_pinned_only {
        let saved = core.update_quick_paste_preferences(request).await?;
        quick.cache(saved, existing.available, existing.error);
        let _ = app.emit("settings-changed", ());
        return quick_paste::settings(&app);
    }
    let next = request
        .shortcut
        .as_deref()
        .map(desktop::parse_shortcut)
        .transpose()?;
    if let Some(next) = next {
        let main = desktop
            .preferences()
            .ok_or_else(|| AppError::new(ErrorCode::Unavailable))?
            .shortcut;
        if next == desktop::parse_shortcut(&main)? {
            return Err(AppError::new(ErrorCode::ShortcutConflict));
        }
    }
    let old = quick.binding();
    let registered =
        if let Some(next) = next.filter(|next| Some(*next) != old || !existing.available) {
            if desktop::wayland() {
                return Err(AppError::new(ErrorCode::Restricted));
            }
            quick_paste::prepare_window(&app)?;
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || quick_paste::register(&app, next))
                .await
                .map_err(|_| AppError::new(ErrorCode::Unavailable))??;
            Some(next)
        } else {
            None
        };
    let result = core.update_quick_paste_preferences(request).await;
    let unregister = match &result {
        Ok(saved) => {
            quick.cache(saved.clone(), next.is_some(), None);
            old.filter(|_| existing.available && old != next)
        }
        Err(_) => registered,
    };
    if let Some(shortcut) = unregister {
        let app = app.clone();
        let _ =
            tauri::async_runtime::spawn_blocking(move || quick_paste::unregister(&app, shortcut))
                .await;
    }
    result?;
    let _ = app.emit("settings-changed", ());
    quick_paste::settings(&app)
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
    if app.state::<crate::quick_paste::QuickPasteState>().binding() == Some(next_shortcut) {
        return Err(AppError::new(ErrorCode::ShortcutConflict));
    }
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
            crate::desktop::set_titlebar(&window, request.native_titlebar).await?;
        }
        window
            .set_always_on_top(true)
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
            let _ = app.emit("settings-changed", ());
            Ok(saved)
        }
        Err(error) => {
            let _ = crate::desktop::set_titlebar(&window, previous.native_titlebar).await;
            let _ = window.set_always_on_top(true);
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
    use tauri::Manager;
    let registration = app.clone();
    let actual =
        tauri::async_runtime::spawn_blocking(move || crate::autostart::set(&registration, enabled))
            .await
            .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))??;
    if let Ok(mut error) = app
        .state::<crate::desktop::DesktopState>()
        .autostart_error
        .lock()
    {
        *error = None;
    }
    Ok(actual)
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
    crate::hover_preview::hide(&app, None, false)?;
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
    crate::hover_preview::hide(window.app_handle(), None, false)?;
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
pub async fn open_settings_window(app: tauri::AppHandle) -> Result<(), AppError> {
    use tauri::Manager;
    // Serialize this window's construction, including simultaneous menu/shortcut requests.
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _opening = OPENING.lock().await;
    if let Some(window) = app.get_webview_window("settings") {
        crate::desktop::ensure_visible(&window)?;
        window
            .show()
            .and_then(|_| window.unminimize())
            .and_then(|_| window.set_focus())
            .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?;
        return Ok(());
    }
    let parent = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::new(uclip_core::ErrorCode::Unavailable))?;
    let kind = uclip_core::AuxiliaryWindow::Settings;
    let monitor = parent.current_monitor().ok().flatten();
    let size = app
        .state::<crate::auxiliary_window::SizeState>()
        .fit(kind, monitor.as_ref());
    let minimum = kind.minimum_size();
    let maximum = f64::from(uclip_core::AuxiliaryWindow::MAXIMUM_DIMENSION);
    let window = crate::local_window(&app, "settings", "index.html?window=settings")
        .title(crate::desktop::settings_title(&app))
        .inner_size(f64::from(size.width), f64::from(size.height))
        .min_inner_size(f64::from(minimum.width), f64::from(minimum.height))
        .max_inner_size(maximum, maximum)
        .resizable(true)
        .parent(&parent)
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?
        .build()
        .map_err(|_| AppError::new(uclip_core::ErrorCode::Unavailable))?;
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    crate::auxiliary_window::attach_resize_tracking(&window, kind)?;
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = window;
    Ok(())
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
pub async fn delete_collection(
    state: State<'_, AppState>,
    request: uclip_core::DeleteCollection,
) -> Result<u32, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .delete_collection(request)
        .await
}

#[tauri::command]
pub async fn transfer_clips(
    state: State<'_, AppState>,
    request: uclip_core::TransferClips,
) -> Result<uclip_core::ClipTransferBatch, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .transfer_clips(request)
        .await
}

#[tauri::command]
pub async fn editable_content(
    state: State<'_, AppState>,
    request: uclip_core::ClipVersion,
) -> Result<uclip_core::EditableContent, AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .editable_content(request)
        .await
}

#[tauri::command]
pub async fn edit_clip(
    state: State<'_, AppState>,
    request: uclip_core::EditClip,
) -> Result<(), AppError> {
    state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .edit_clip(request)
        .await
}

#[tauri::command]
pub async fn update_key_bindings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: uclip_core::UpdateKeyBindings,
) -> Result<uclip_core::KeyboardSnapshot, AppError> {
    let saved = state
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .update_key_bindings(request)
        .await?;
    let _ = app.emit("settings-changed", ());
    Ok(saved)
}

/// A resolved presentation locale. This does not mutate persisted settings.
#[tauri::command]
pub fn set_ui_language(
    app: tauri::AppHandle,
    language: uclip_core::Language,
) -> Result<(), AppError> {
    let resolved = match language {
        uclip_core::Language::ZhCn => 2,
        uclip_core::Language::En => 1,
        uclip_core::Language::System => {
            return Err(AppError::new(uclip_core::ErrorCode::InvalidInput));
        }
    };
    app.state::<crate::desktop::DesktopState>()
        .ui_language
        .store(resolved, std::sync::atomic::Ordering::SeqCst);
    crate::desktop::update_ui_language(&app)
}

#[tauri::command]
pub async fn show_hover_preview(
    app: tauri::AppHandle,
    request: uclip_core::HoverPreviewRequest,
    previous_token: Option<String>,
) -> Result<bool, AppError> {
    let token = request.token.clone();
    let result = crate::hover_preview::show(app.clone(), request, previous_token).await;
    if result.is_err() {
        let _ = crate::hover_preview::hide(&app, Some(&token), false);
    }
    result
}
#[tauri::command]
pub fn hide_hover_preview(
    app: tauri::AppHandle,
    token: String,
    unless_hovered: bool,
) -> Result<(), AppError> {
    crate::hover_preview::hide(&app, Some(&token), unless_hovered)
}
#[tauri::command]
pub fn get_hover_preview(
    app: tauri::AppHandle,
) -> Result<Option<uclip_core::HoverPreviewRequest>, AppError> {
    crate::hover_preview::current(&app)
}
#[tauri::command]
pub fn hover_preview_presence(
    app: tauri::AppHandle,
    token: String,
    hovered: bool,
) -> Result<(), AppError> {
    crate::hover_preview::presence(app, token, hovered)
}
