//! OS registration is the source of truth. Never accept a renderer-supplied path.
use uclip_core::{AppError, ErrorCode};
fn failure() -> AppError {
    AppError::new(ErrorCode::Restricted)
}

pub async fn initialize(
    app: &tauri::AppHandle,
    core: &uclip_core::CoreService,
) -> Result<(), AppError> {
    if core.take_autostart_default().await? {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<(), AppError> {
            if !enabled(&app)? {
                set(&app, true)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| AppError::new(ErrorCode::Unavailable))??;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn enabled(app: &tauri::AppHandle) -> Result<bool, AppError> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|_| failure())
}
#[cfg(not(target_os = "linux"))]
pub fn set(app: &tauri::AppHandle, enabled: bool) -> Result<bool, AppError> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    (if enabled {
        manager.enable()
    } else {
        manager.disable()
    })
    .map_err(|_| failure())?;
    let actual = manager.is_enabled().map_err(|_| failure())?;
    if actual != enabled {
        return Err(failure());
    }
    Ok(actual)
}

#[cfg(target_os = "linux")]
fn entry(app: &tauri::AppHandle) -> Result<(std::path::PathBuf, String), AppError> {
    use tauri::Manager;
    let file = app
        .path()
        .config_dir()
        .map_err(|_| failure())?
        .join("autostart")
        .join(format!("{}.desktop", app.config().identifier));
    // current_exe points into a temporary mount/extraction when running an AppImage.
    let executable = app
        .env()
        .appimage
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)
        .map_err(|_| failure())?;
    Ok((file, contents(executable.to_str().ok_or_else(failure)?)?))
}
#[cfg(target_os = "linux")]
fn contents(executable: &str) -> Result<String, AppError> {
    // Desktop Entry quoting has a string-escape pass before its argument-quoting pass.
    if !std::path::Path::new(executable).is_absolute()
        || executable.chars().any(|c| c.is_control() || c == '=')
    {
        return Err(failure());
    }
    let mut quoted = String::from("\"");
    for ch in executable.chars() {
        match ch {
            '\\' => quoted.push_str(r"\\\\"),
            '"' => quoted.push_str(r#"\\\""#),
            '$' => quoted.push_str(r"\\$"),
            '`' => quoted.push_str(r"\\`"),
            '%' => quoted.push_str("%%"),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    let name = if cfg!(debug_assertions) {
        "UClip Development"
    } else {
        "UClip"
    };
    Ok(format!("[Desktop Entry]\nType=Application\nVersion=1.0\nName={name}\nExec={quoted}\nTerminal=false\nX-UClip-Managed=true\n"))
}
#[cfg(target_os = "linux")]
fn read(file: &std::path::Path) -> Result<Option<String>, AppError> {
    match std::fs::symlink_metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Ok(m) if m.file_type().is_file() && m.len() <= 8192 => std::fs::read_to_string(file)
            .map(Some)
            .map_err(|_| failure()),
        _ => Err(failure()),
    }
}
#[cfg(target_os = "linux")]
pub fn enabled(app: &tauri::AppHandle) -> Result<bool, AppError> {
    let (file, expected) = entry(app)?;
    match read(&file)? {
        None => Ok(false),
        Some(actual) if actual == expected => Ok(true),
        Some(actual) if actual.lines().any(|l| l == "X-UClip-Managed=true") => Ok(false),
        Some(_) => Err(failure()),
    }
}
#[cfg(target_os = "linux")]
pub fn set(app: &tauri::AppHandle, enabled: bool) -> Result<bool, AppError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let (file, data) = entry(app)?;
    if read(&file)?.is_some_and(|value| !value.lines().any(|l| l == "X-UClip-Managed=true")) {
        return Err(failure());
    }
    if enabled {
        let parent = file.parent().ok_or_else(failure)?;
        std::fs::create_dir_all(parent).map_err(|_| failure())?;
        let temporary = parent.join(format!(".uclip-{}.tmp", uuid::Uuid::new_v4()));
        let write = (|| {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            f.write_all(data.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&temporary, &file)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if write.is_err() {
            let _ = std::fs::remove_file(temporary);
            return Err(failure());
        }
    } else {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failure()),
        }
    }
    self::enabled(app)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn desktop_entry_rejects_line_injection_and_quotes_reserved_characters() {
        for value in [
            "relative",
            "/tmp/program\nExec=bad",
            "/tmp/program=name",
            "/tmp/program\0",
        ] {
            assert!(contents(value).is_err());
        }
        let value = contents("/tmp/目录 space/$HOME`x`\\\"%p").unwrap();
        assert!(value
            .lines()
            .all(|line| !line.starts_with("Exec=") || line.ends_with('"')));
        assert_eq!(
            value
                .lines()
                .filter(|line| line.starts_with("Exec="))
                .count(),
            1
        );
        assert!(value.contains(r"\\$HOME"));
        assert!(value.contains("%%p"));
    }
}
