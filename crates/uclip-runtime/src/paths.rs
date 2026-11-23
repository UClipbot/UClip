use std::{fs::DirBuilder, path::Path, path::PathBuf};

use uclip_core::{AppError, ErrorCode};

pub const DATA_PROFILE: &str = if cfg!(debug_assertions) {
    "development"
} else {
    "release"
};
pub const DATABASE: &str = "foundation.sqlite3";

pub fn data_dir(development: bool) -> Result<PathBuf, AppError> {
    let root = dirs::data_local_dir().ok_or_else(storage)?;
    let (identifier, profile) = if development {
        ("app.uclip.dev", "development")
    } else {
        ("app.uclip", DATA_PROFILE)
    };
    Ok(root.join(identifier).join(profile))
}

pub fn prepare_directory(path: &Path) -> Result<(), AppError> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| storage())?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| storage())?;
    if !metadata.is_dir() {
        return Err(storage());
    }
    Ok(())
}

fn storage() -> AppError {
    AppError::new(ErrorCode::Storage)
}
