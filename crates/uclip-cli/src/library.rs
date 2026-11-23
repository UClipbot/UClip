use std::{fs, path::PathBuf};

use uclip_core::{AppError, CoreService, ErrorCode};
use uclip_runtime::paths::{self, DATABASE, DATA_PROFILE};

pub struct Location {
    pub path: PathBuf,
}

impl Location {
    pub fn resolve(
        explicit: Option<PathBuf>,
        development: bool,
        portable: bool,
    ) -> Result<Self, AppError> {
        let path = if let Some(path) = explicit {
            path
        } else if development {
            paths::data_dir(true)?
        } else if portable {
            let executable = std::env::current_exe().map_err(|_| crate::storage())?;
            executable
                .parent()
                .ok_or_else(crate::storage)?
                .join("uclip-data")
                .join(DATA_PROFILE)
        } else if let Some(path) = std::env::var_os("UCLIP_DATA_DIR") {
            path.into()
        } else {
            paths::data_dir(false)?
        };
        if !path.is_absolute() {
            return Err(crate::invalid());
        }
        Ok(Self { path })
    }

    pub async fn open(&self, create: bool) -> Result<CoreService, AppError> {
        if create {
            paths::prepare_directory(&self.path)?;
        }
        let directory = fs::symlink_metadata(&self.path).map_err(file_error)?;
        if !directory.is_dir() {
            return Err(crate::storage());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if directory.permissions().mode() & 0o077 != 0 {
                return Err(AppError::new(ErrorCode::Restricted));
            }
        }
        let database = self.path.join(DATABASE);
        for filename in [
            DATABASE,
            "foundation.sqlite3-wal",
            "foundation.sqlite3-shm",
            "foundation.sqlite3-journal",
        ] {
            match fs::symlink_metadata(self.path.join(filename)) {
                Ok(metadata) if metadata.is_file() => {}
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && (create || filename != DATABASE) => {}
                Err(error) => return Err(file_error(error)),
                Ok(_) => return Err(crate::storage()),
            }
        }
        CoreService::open(database).await
    }
}

fn file_error(error: std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AppError::new(ErrorCode::NotFound)
    } else {
        crate::storage()
    }
}
