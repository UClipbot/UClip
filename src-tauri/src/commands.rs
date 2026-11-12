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
