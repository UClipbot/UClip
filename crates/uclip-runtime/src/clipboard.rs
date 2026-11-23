//! Shared native clipboard formats and publication.
use std::path::Path;
use uclip_core::{AppError, ClipPayload, ErrorCode};

#[cfg(target_os = "windows")]
pub use crate::clipboard_native::ClipboardWrite;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use crate::clipboard_native::Context;
#[cfg(target_os = "linux")]
use clipboard_rs::{Clipboard, ClipboardContent};
#[cfg(target_os = "linux")]
use x11rb::connection::RequestConnection;

#[cfg(target_os = "linux")]
pub struct Context {
    clipboard: clipboard_rs::ClipboardContext,
    max_property_bytes: usize,
}

#[cfg(target_os = "linux")]
impl Context {
    pub fn new() -> Result<Self, AppError> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        let (connection, _) = x11rb::connect(None).map_err(|_| error())?;
        // ChangeProperty has a 24-byte header and BIG-REQUESTS can add four bytes.
        // The server limit and the remaining capacity are both four-byte aligned.
        let max_property_bytes = connection.maximum_request_bytes().saturating_sub(28);
        let clipboard = clipboard_rs::ClipboardContext::new().map_err(|_| error())?;
        Ok(Self {
            clipboard,
            max_property_bytes,
        })
    }

    fn check_property_size(&self, length: usize) -> Result<(), AppError> {
        if length > self.max_property_bytes {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub const MARKER: &str = "com.uclip.internal";
#[cfg(not(target_os = "macos"))]
pub const MARKER: &str = "application/x-uclip-internal";
#[cfg(target_os = "macos")]
pub const CLI_MARKER: &str = "com.uclip.cli-transient";
#[cfg(not(target_os = "macos"))]
pub const CLI_MARKER: &str = "application/x-uclip-cli-transient";

#[derive(Clone)]
pub struct Captured {
    pub payload: ClipPayload,
    pub source_app: Option<String>,
}

fn error() -> AppError {
    AppError::new(ErrorCode::Clipboard)
}

pub fn sensitive(
    formats: &[String],
    get_buffer: impl Fn(&str) -> Result<Vec<u8>, AppError>,
) -> bool {
    formats.iter().any(|format| {
        let lower = format.to_ascii_lowercase();
        if lower.contains("concealed")
            || lower.contains("transient")
            || lower.contains("passwordmanager")
            || lower.contains("keepass")
            || lower == "excludeclipboardcontentfrommonitorprocessing"
        {
            return true;
        }
        if lower == "canincludeinclipboardhistory" || lower == "canuploadtocloudclipboard" {
            return get_buffer(format).map_or(true, |v| v.iter().all(|b| *b == 0));
        }
        false
    })
}
pub fn normalize_files(files: Vec<String>) -> Result<Vec<String>, AppError> {
    files
        .into_iter()
        .map(|path| {
            if path.starts_with("file:") {
                let uri = url::Url::parse(&path).map_err(|_| error())?;
                if uri
                    .host_str()
                    .is_some_and(|h| !h.is_empty() && h != "localhost")
                    || uri.query().is_some()
                    || uri.fragment().is_some()
                {
                    return Err(AppError::new(ErrorCode::InvalidInput));
                }
                uri.to_file_path()
                    .map_err(|_| error())?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| error())
            } else if Path::new(&path).is_absolute() {
                Ok(path)
            } else {
                Err(AppError::new(ErrorCode::InvalidInput))
            }
        })
        .collect()
}
#[cfg(target_os = "linux")]
pub fn write(
    context: &Context,
    payload: ClipPayload,
    format: &str,
    marker: &[u8],
) -> Result<(), AppError> {
    validate_write(&payload)?;
    context.check_property_size(marker.len())?;
    let content = match payload {
        ClipPayload::Text(text) => {
            context.check_property_size(text.len())?;
            ClipboardContent::Text(text)
        }
        ClipPayload::Image {
            width,
            height,
            rgba,
        } => {
            let bytes = crate::clipboard_native::png(width, height, rgba)?;
            context.check_property_size(bytes.len())?;
            ClipboardContent::Other("image/png".into(), bytes)
        }
        ClipPayload::Files(paths) => {
            let paths = paths
                .iter()
                .map(|p| {
                    url::Url::from_file_path(p)
                        .map(|u| u.to_string())
                        .map_err(|_| error())
                })
                .collect::<Result<Vec<_>, _>>()?;
            // The largest clipboard-rs file targets are the CRLF URI list and
            // the GNOME/Nautilus URI lists with a "copy\n" prefix.
            let uri_bytes: usize = paths.iter().map(String::len).sum();
            let separators = paths.len() - 1;
            context.check_property_size(uri_bytes + (2 * separators).max(5 + separators))?;
            ClipboardContent::Files(paths)
        }
    };
    context
        .clipboard
        .set(vec![
            content,
            ClipboardContent::Other(format.into(), marker.to_vec()),
        ])
        .map_err(|_| error())
}
#[cfg(target_os = "windows")]
pub fn write(
    context: &Context,
    payload: ClipPayload,
    format: &str,
    marker: &[u8],
) -> Result<ClipboardWrite, AppError> {
    validate_write(&payload)?;
    context.write(payload, format, marker)
}
#[cfg(target_os = "macos")]
pub fn write(
    context: &Context,
    payload: ClipPayload,
    format: &str,
    marker: &[u8],
) -> Result<(), AppError> {
    validate_write(&payload)?;
    context.write(payload, format, marker)
}
fn validate_write(payload: &ClipPayload) -> Result<(), AppError> {
    payload.validate()?;
    if let ClipPayload::Files(paths) = payload {
        if paths
            .iter()
            .any(|p| !Path::new(p).try_exists().unwrap_or(false))
        {
            return Err(AppError::new(ErrorCode::MissingFile));
        }
    }
    Ok(())
}
