//! Desktop lifetime stays native. UI only supplies settings and transient interaction guards.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use uclip_core::{AppError, DesktopPreferences, ErrorCode, WindowPlacement};

#[derive(Default)]
pub struct DesktopState {
    pub preferences: Mutex<Option<DesktopPreferences>>,
    pub autostart_error: Mutex<Option<AppError>>,
    pub updating: AtomicBool,
    pub tray_available: AtomicBool,
    pub shortcut_available: AtomicBool,
    pub protected: AtomicBool,
    pub geometry_epoch: AtomicU64,
    pub restoring: AtomicBool,
}
impl DesktopState {
    pub fn preferences(&self) -> Option<DesktopPreferences> {
        self.preferences.lock().ok().and_then(|p| p.clone())
    }
    pub fn cache(&self, value: DesktopPreferences) {
        if let Ok(mut p) = self.preferences.lock() {
            *p = Some(value);
        }
    }
    pub fn recoverable(&self) -> bool {
        // Linux tray construction does not prove there is a visible tray host.
        self.shortcut_available.load(Ordering::SeqCst)
            || (!cfg!(target_os = "linux") && self.tray_available.load(Ordering::SeqCst))
    }
}
fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable)
}
pub fn wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some())
}
pub fn parse_shortcut(value: &str) -> Result<Shortcut, AppError> {
    let shortcut: Shortcut = value
        .parse()
        .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
    // Do not register unmodified typing keys or a modifier alone.
    if !shortcut.mods.intersects(
        tauri_plugin_global_shortcut::Modifiers::CONTROL
            | tauri_plugin_global_shortcut::Modifiers::ALT
            | tauri_plugin_global_shortcut::Modifiers::SUPER,
    ) {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok(shortcut)
}
pub fn register_shortcut(app: &tauri::AppHandle, value: &str) -> Result<(), AppError> {
    if wayland() {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    let shortcut = parse_shortcut(value)?;
    app.global_shortcut()
        .on_shortcut(shortcut, |app, pressed, event| {
            let current = app
                .state::<DesktopState>()
                .preferences()
                .and_then(|p| parse_shortcut(&p.shortcut).ok());
            if current != Some(*pressed) {
                return;
            }
            if event.state == ShortcutState::Pressed {
                let app = app.clone();
                // X11 round-trips never run on the UI event loop.
                tauri::async_runtime::spawn_blocking(move || {
                    if let Some(state) = app.try_state::<crate::AppState>() {
                        if let Ok(clipboard) = &state.clipboard {
                            clipboard.capture_target();
                        }
                    }
                    let _ = show(&app, true);
                });
            }
        })
        .map_err(|_| AppError::new(ErrorCode::ShortcutConflict))
}
pub fn show(app: &tauri::AppHandle, toggle: bool) -> Result<(), AppError> {
    let window = app.get_webview_window("main").ok_or_else(unavailable)?;
    if toggle
        && window.is_visible().unwrap_or(false)
        && window.is_focused().unwrap_or(false)
        && !app.state::<DesktopState>().protected.load(Ordering::SeqCst)
    {
        return window.hide().map_err(|_| unavailable());
    }
    #[cfg(target_os = "linux")]
    let activation = if wayland() {
        None
    } else {
        crate::window_activation::Activation::prepare().ok()
    };
    // Monitor changes can make a previously valid position unreachable.
    ensure_visible(&window)?;
    window
        .show()
        .and_then(|_| window.unminimize())
        .and_then(|_| window.set_focus())
        .map_err(|_| unavailable())?;
    #[cfg(target_os = "linux")]
    if let Some(activation) = activation {
        activation.request(&window)?;
    }
    Ok(())
}
pub fn ensure_visible(window: &tauri::WebviewWindow) -> Result<(), AppError> {
    let position = window.outer_position().map_err(|_| unavailable())?;
    let monitors = window.available_monitors().map_err(|_| unavailable())?;
    let visible = monitors.iter().any(|m| {
        let a = m.work_area();
        i64::from(position.x) >= i64::from(a.position.x)
            && i64::from(position.y) >= i64::from(a.position.y)
            && i64::from(position.x) + 100 < i64::from(a.position.x) + i64::from(a.size.width)
            && i64::from(position.y) + 40 < i64::from(a.position.y) + i64::from(a.size.height)
    });
    if !visible {
        window.center().map_err(|_| unavailable())?;
    }
    Ok(())
}
pub fn install_tray(app: &tauri::AppHandle) -> Result<(), AppError> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };
    let open = MenuItem::with_id(app, "show", "显示 UClip", true, None::<&str>)
        .map_err(|_| unavailable())?;
    let quit = MenuItem::with_id(app, "quit", "退出 UClip", true, None::<&str>)
        .map_err(|_| unavailable())?;
    let menu = Menu::with_items(app, &[&open, &quit]).map_err(|_| unavailable())?;
    // Existing local icon system: a simple clipboard silhouette, no remote assets.
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 4..29 {
        for x in 6..26 {
            if !(9..23).contains(&x)
                || !(7..26).contains(&y)
                || ((11..21).contains(&x) && (11..14).contains(&y))
                || ((11..21).contains(&x) && (18..21).contains(&y))
            {
                pixels[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4]
                    .copy_from_slice(&[104, 147, 121, 255]);
            }
        }
    }
    TrayIconBuilder::with_id("main-tray")
        .icon(tauri::image::Image::new_owned(pixels, 32, 32))
        .tooltip("UClip")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let _ = show(&app, false);
                });
            }
            "quit" => crate::shutdown(app.clone()),
            _ => {}
        })
        .build(app)
        .map_err(|_| unavailable())?;
    app.state::<DesktopState>()
        .tray_available
        .store(true, Ordering::SeqCst);
    Ok(())
}
pub async fn restore(app: &tauri::AppHandle) -> Result<(), AppError> {
    let core = app
        .state::<crate::AppState>()
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let placement = core.window_placement().await?;
    let preferences = core.desktop_preferences().await?;
    let window = app.get_webview_window("main").ok_or_else(unavailable)?;
    let desktop = app.state::<DesktopState>();
    desktop.restoring.store(true, Ordering::SeqCst);
    let result = (|| {
        if let Some(p) = placement {
            let monitor = window.current_monitor().map_err(|_| unavailable())?;
            if let Some(m) = monitor {
                let size = m.work_area().size;
                window
                    .set_size(tauri::PhysicalSize::new(
                        p.width.min(size.width),
                        p.height.min(size.height),
                    ))
                    .map_err(|_| unavailable())?;
            }
            window
                .set_position(tauri::PhysicalPosition::new(p.x, p.y))
                .map_err(|_| unavailable())?;
            ensure_visible(&window)?;
            if p.maximized {
                window.maximize().map_err(|_| unavailable())?;
            }
        }
        window
            .set_always_on_top(preferences.always_on_top)
            .map_err(|_| unavailable())?;
        Ok(())
    })();
    desktop.restoring.store(false, Ordering::SeqCst);
    result
}
pub async fn save_placement(app: &tauri::AppHandle) -> Result<(), AppError> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    if window.is_minimized().unwrap_or(true) {
        return Ok(());
    }
    let maximized = window.is_maximized().map_err(|_| unavailable())?;
    let core = app
        .state::<crate::AppState>()
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let placement = if maximized {
        let Some(mut p) = core.window_placement().await? else {
            return Ok(());
        };
        p.maximized = true;
        p
    } else {
        let p = window.outer_position().map_err(|_| unavailable())?;
        let size = window.inner_size().map_err(|_| unavailable())?;
        WindowPlacement {
            x: p.x,
            y: p.y,
            width: size.width,
            height: size.height,
            maximized: false,
        }
    };
    core.save_window_placement(placement).await
}
pub fn window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<DesktopState>();
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            if !crate::EXIT_STARTED.load(Ordering::SeqCst) {
                // Save before hiding, as hiding may no longer generate geometry events.
                api.prevent_close();
                let hide =
                    state.preferences().is_some_and(|p| p.close_to_tray) && state.recoverable();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = save_placement(&app).await;
                    if hide {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    } else {
                        crate::shutdown(app);
                    }
                });
            }
        }
        WindowEvent::Moved(_) | WindowEvent::Resized(_)
            if !state.restoring.load(Ordering::SeqCst) =>
        {
            let epoch = state.geometry_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                if app
                    .state::<DesktopState>()
                    .geometry_epoch
                    .load(Ordering::SeqCst)
                    == epoch
                {
                    let _ = save_placement(&app).await;
                }
            });
        }
        WindowEvent::Focused(false) => {
            let epoch = state.geometry_epoch.load(Ordering::SeqCst);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let state = app.state::<DesktopState>();
                let hide = state
                    .preferences()
                    .is_some_and(|p| p.auto_hide && !p.always_on_top)
                    && state.recoverable()
                    && !state.protected.load(Ordering::SeqCst)
                    && state.geometry_epoch.load(Ordering::SeqCst) == epoch;
                if hide {
                    if let Some(w) = app.get_webview_window("main") {
                        if !w.is_focused().unwrap_or(true) {
                            let _ = w.hide();
                        }
                    }
                }
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_require_a_real_modifier_and_parse_the_platform_default() {
        assert!(parse_shortcut("CommandOrControl+Shift+V").is_ok());
        assert!(parse_shortcut("Control+Alt+K").is_ok());
        for value in ["V", "Shift+V", "Control+", "bogus", "Control+Alt+K+V"] {
            assert!(parse_shortcut(value).is_err(), "{value}");
        }
    }
}
