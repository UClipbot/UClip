use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use uclip_core::{AppError, ErrorCode};

pub use uclip_runtime::paths::DATA_PROFILE;

pub fn data_dir(app: &AppHandle) -> Result<PathBuf, AppError> {
    let root = app
        .path()
        .app_local_data_dir()
        .map_err(|_| AppError::new(ErrorCode::Storage))?;
    // The identifier and profile both differ from the previous production application.
    Ok(root.join(DATA_PROFILE))
}

pub fn prepare_directory(path: &std::path::Path) -> Result<(), AppError> {
    uclip_runtime::paths::prepare_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| AppError::new(ErrorCode::Storage))?;
    }
    Ok(())
}
