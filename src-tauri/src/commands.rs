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
    state: State<'_, AppState>,
) -> Result<uclip_core::ClipboardStatus, AppError> {
    match &state.clipboard {
        Ok(service) => service.status().await,
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
