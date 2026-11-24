use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use uclip_core::{AppError, ErrorCode, HardwareAcceleration};

pub struct Preferences {
    file: Result<PathBuf, AppError>,
    enabled: AtomicBool,
    enabled_at_startup: bool,
    writer: Mutex<()>,
}

impl Preferences {
    /// Read the preference before any webview is created.
    pub fn initialize(identifier: &str) -> Self {
        #[cfg(target_os = "linux")]
        let filename = "opengl";
        #[cfg(target_os = "windows")]
        let filename = "hardware-acceleration";
        let file = uclip_runtime::paths::data_dir(identifier == "app.uclip.dev")
            .map(|directory| directory.join(filename));
        let saved = file.as_ref().ok().and_then(|path| {
            let metadata = fs::symlink_metadata(path).ok()?;
            if !metadata.is_file() || metadata.len() > 8 {
                return None;
            }
            fs::read_to_string(path).ok()?.trim().parse::<bool>().ok()
        });
        #[cfg(target_os = "linux")]
        let enabled = configure_webkit(saved);
        #[cfg(target_os = "windows")]
        let enabled = saved.unwrap_or(true);
        Self {
            file,
            enabled: AtomicBool::new(enabled),
            enabled_at_startup: enabled,
            writer: Mutex::new(()),
        }
    }

    pub fn status(&self) -> HardwareAcceleration {
        HardwareAcceleration {
            enabled: self.enabled.load(Ordering::SeqCst),
            enabled_at_startup: self.enabled_at_startup,
        }
    }

    pub fn save(&self, enabled: bool) -> Result<(), AppError> {
        let _writer = self.writer.lock().map_err(|_| storage())?;
        let file = self.file.as_ref().map_err(Clone::clone)?;
        let parent = file.parent().ok_or_else(storage)?;
        uclip_runtime::paths::prepare_directory(parent)?;
        let temporary = parent.join(format!(
            ".hardware-acceleration-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        let write = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut config = options.open(&temporary)?;
            writeln!(config, "{enabled}")?;
            config.sync_all()?;
            drop(config);
            fs::rename(&temporary, file)?;
            Ok::<_, std::io::Error>(())
        })();
        if write.is_err() {
            let _ = fs::remove_file(temporary);
            return Err(storage());
        }
        self.enabled.store(enabled, Ordering::SeqCst);
        #[cfg(target_os = "linux")]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| storage())?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub fn webview2_browser_args(&self) -> Option<&'static str> {
        // All windows share the startup policy. Preserve Wry's current defaults
        // when supplying AdditionalBrowserArguments for software rendering.
        (!self.enabled_at_startup).then_some(concat!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ",
            "--autoplay-policy=no-user-gesture-required --disable-gpu",
        ))
    }
}

#[cfg(target_os = "linux")]
fn configure_webkit(saved: Option<bool>) -> bool {
    use std::{env, path::Path};

    const DISABLE_COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
    const CPU_PAINTING: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
    let enabled = saved.unwrap_or_else(|| {
        if let Some(value) = env::var_os(DISABLE_COMPOSITING) {
            return value == "0";
        }
        // NVIDIA GPU painting can leak fence descriptors until the renderer stalls.
        let nvidia_x11 = !crate::desktop::wayland()
            && env::var_os("DISPLAY").is_some_and(|display| !display.is_empty())
            && Path::new("/sys/module/nvidia").is_dir();
        !nvidia_x11
    });
    if !enabled {
        env::remove_var("WEBKIT_FORCE_COMPOSITING_MODE");
        env::set_var(DISABLE_COMPOSITING, "1");
        env::set_var(CPU_PAINTING, "1");
    } else if saved == Some(true) {
        env::remove_var(DISABLE_COMPOSITING);
        env::remove_var(CPU_PAINTING);
    }
    enabled
}

fn storage() -> AppError {
    AppError::new(ErrorCode::Storage)
}
