//! Desktop lifetime stays native. UI only supplies settings and transient interaction guards.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Mutex,
};
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use uclip_core::{AppError, DesktopPreferences, ErrorCode, WindowPlacement};

#[derive(Default)]
pub struct DesktopState {
    /// 0 = not resolved yet, 1 = English, 2 = Simplified Chinese.
    pub ui_language: AtomicU8,
    pub preferences: Mutex<Option<DesktopPreferences>>,
    pub autostart_error: Mutex<Option<AppError>>,
    pub updating: AtomicBool,
    pub tray_available: AtomicBool,
    pub shortcut_available: AtomicBool,
    shortcut_recording: Mutex<Option<ShortcutRecording>>,
    pub protected: AtomicBool,
    pub geometry_epoch: AtomicU64,
    pub restoring: AtomicBool,
}

#[derive(Clone, Copy)]
struct ShortcutRecording {
    token: u32,
    #[cfg(target_os = "linux")]
    x11_window: Option<u32>,
}

impl ShortcutRecording {
    fn notify(self, app: &tauri::AppHandle, binding: Option<String>) {
        let _ = app.emit_to("settings", "global-shortcut-capture", (self.token, binding));
    }

    fn end(self, app: &tauri::AppHandle) {
        let state = app.state::<DesktopState>();
        let ended = state
            .shortcut_recording
            .lock()
            .ok()
            .and_then(|mut capture| capture.take_if(|active| active.token == self.token));
        if ended.is_some() {
            self.notify(app, None);
        }
    }
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
    fn recording(&self) -> Option<ShortcutRecording> {
        self.shortcut_recording
            .lock()
            .ok()
            .and_then(|active| *active)
    }
}
fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable)
}

pub async fn set_titlebar(window: &tauri::WebviewWindow, native: bool) -> Result<(), AppError> {
    #[cfg(not(target_os = "macos"))]
    {
        window.set_decorations(native).map_err(|_| unavailable())
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSWindow, NSWindowButton, NSWindowTitleVisibility};
        // Keep the titled NSWindow frame: borderless windows lose the system's
        // rounded corners. Overlay lets the custom chrome fill the titlebar area.
        window
            .set_title_bar_style(if native {
                tauri::TitleBarStyle::Visible
            } else {
                tauri::TitleBarStyle::Overlay
            })
            .map_err(|_| unavailable())?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        window
            .with_webview(move |webview| {
                // SAFETY: with_webview runs on the macOS main thread and supplies
                // the live NSWindow associated with this WebView. Do not retain it.
                let result = unsafe { webview.ns_window().cast::<NSWindow>().as_ref() }
                    .ok_or_else(unavailable)
                    .map(|window| {
                        window.setTitleVisibility(if native {
                            NSWindowTitleVisibility::Visible
                        } else {
                            NSWindowTitleVisibility::Hidden
                        });
                        for kind in [
                            NSWindowButton::CloseButton,
                            NSWindowButton::MiniaturizeButton,
                            NSWindowButton::ZoomButton,
                        ] {
                            if let Some(button) = window.standardWindowButton(kind) {
                                button.setHidden(!native);
                            }
                        }
                    });
                let _ = tx.send(result);
            })
            .map_err(|_| unavailable())?;
        rx.await.map_err(|_| unavailable())?
    }
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

#[tauri::command]
pub fn set_global_shortcut_recording(
    window: tauri::WebviewWindow,
    token: u32,
    recording: bool,
) -> Result<(), AppError> {
    if window.label() != "settings" {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    let state = window.state::<DesktopState>();
    if recording {
        if !window.is_focused().unwrap_or(false) {
            return Err(unavailable());
        }
        let active = ShortcutRecording {
            token,
            #[cfg(target_os = "linux")]
            x11_window: (!wayland())
                .then(|| crate::window_activation::window_id(&window))
                .transpose()?,
        };
        *state.shortcut_recording.lock().map_err(|_| unavailable())? = Some(active);
    } else {
        state
            .shortcut_recording
            .lock()
            .map_err(|_| unavailable())?
            .take_if(|active| active.token == token);
    }
    Ok(())
}

/// The OS may consume a registered combination before the settings WebView receives it.
pub fn forward_shortcut_to_recorder(app: &tauri::AppHandle, shortcut: &Shortcut) -> bool {
    let state = app.state::<DesktopState>();
    let Some(active) = state.recording() else {
        return false;
    };
    // On X11, the hotkey callback runs on the shortcut worker. A passive grab can
    // temporarily change keyboard focus without changing the foreground window.
    #[cfg(target_os = "linux")]
    if active
        .x11_window
        .is_some_and(|window| !crate::window_activation::is_active_window(window))
    {
        active.end(app);
        return false;
    }
    if state.recording().map(|recording| recording.token) != Some(active.token) {
        return false;
    }
    active.notify(app, Some(shortcut.to_string()));
    true
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
            if event.state == ShortcutState::Pressed && !forward_shortcut_to_recorder(app, pressed)
            {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if crate::quick_paste::show_main(app).await.is_err() {
                        eprintln!("desktop_focus_failed");
                    }
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
        crate::hover_preview::hide(app, None, false)?;
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
// Public platform APIs avoid Tauri show(), which activates windows on macOS/Windows.
pub fn show_without_focus(window: &tauri::WebviewWindow) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWindow;
        let pointer = window.ns_window().map_err(|_| unavailable())?;
        // SAFETY: called only in the main-thread callback, for the live titled window.
        let native = unsafe { pointer.cast::<NSWindow>().as_ref() }.ok_or_else(unavailable)?;
        native.orderFront(None);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE},
        };
        let handle = window.hwnd().map_err(|_| unavailable())?;
        // SAFETY: the live HWND is accessed on the UI thread, without activation.
        let _ = unsafe { ShowWindow(HWND(handle.0), SW_SHOWNOACTIVATE) };
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        window.show().map_err(|_| unavailable())
    }
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
pub fn settings_title(app: &tauri::AppHandle) -> &'static str {
    if app
        .state::<DesktopState>()
        .ui_language
        .load(Ordering::SeqCst)
        == 2
    {
        "UClip · 设置"
    } else {
        "UClip · Settings"
    }
}
pub fn update_ui_language(app: &tauri::AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window(crate::hover_preview::LABEL) {
        window
            .set_title(crate::hover_preview::title(app))
            .map_err(|_| unavailable())?;
    }
    if let Some(window) = app.get_webview_window("settings") {
        window
            .set_title(settings_title(app))
            .map_err(|_| unavailable())?;
    }
    #[cfg(target_os = "linux")]
    crate::native_tray_menu::update_language(app).map_err(|_| unavailable())?;
    Ok(())
}
pub async fn install_tray(app: &tauri::AppHandle) -> Result<(), AppError> {
    use tauri::tray::TrayIconBuilder;
    let state = app.state::<DesktopState>();
    if let Ok(core) = &app.state::<crate::AppState>().core {
        if let Ok(settings) = core.settings().await {
            let language = match settings.language {
                uclip_core::Language::ZhCn => 2,
                uclip_core::Language::En => 1,
                uclip_core::Language::System => 0,
            };
            // A WebView can resolve the locale while this settings read is pending.
            // A stale startup snapshot must not overwrite that newer presentation state.
            let _ =
                state
                    .ui_language
                    .compare_exchange(0, language, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
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
    #[cfg(target_os = "linux")]
    let custom_status_notifier = crate::status_notifier::install(app, pixels.clone()).await;
    {
        let tray = TrayIconBuilder::with_id("main-tray")
            .icon(tauri::image::Image::new_owned(pixels, 32, 32))
            .tooltip("UClip")
            .show_menu_on_left_click(false);
        #[cfg(target_os = "linux")]
        if !custom_status_notifier {
            crate::native_tray_menu::install(app, tray).map_err(|_| unavailable())?;
        }
        #[cfg(not(target_os = "linux"))]
        tray.on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                match button {
                    MouseButton::Left => show_from_tray(tray.app_handle()),
                    MouseButton::Right => crate::tray_menu::open_from_tray(tray),
                    MouseButton::Middle => {}
                }
            }
        })
        .build(app)
        .map_err(|_| unavailable())?;
    }
    update_ui_language(app)?;
    state.tray_available.store(true, Ordering::SeqCst);
    #[cfg(target_os = "linux")]
    crate::native_tray_menu::refresh(app).await;
    Ok(())
}

pub(crate) fn show_from_tray(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = show(&app, false);
    });
}
pub async fn restore(app: &tauri::AppHandle) -> Result<(), AppError> {
    let core = app
        .state::<crate::AppState>()
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let placement = core.window_placement().await?;
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
        window.set_always_on_top(true).map_err(|_| unavailable())?;
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
    crate::hover_preview::window_event(window, event);
    let app = window.app_handle();
    let state = app.state::<DesktopState>();
    if window.label() == "settings" {
        if let (WindowEvent::Focused(false) | WindowEvent::Destroyed, Some(active)) =
            (event, state.recording())
        {
            #[cfg(target_os = "linux")]
            if let (WindowEvent::Focused(false), Some(window)) = (event, active.x11_window) {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if !crate::window_activation::is_active_window(window) {
                        active.end(&app);
                    }
                });
                return;
            }
            active.end(app);
        }
        return;
    }
    if window.label() != "main" {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            let _ = crate::hover_preview::hide(app, None, false);
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
            crate::hover_preview::main_geometry_changed(window);
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
                let hide = state.preferences().is_some_and(|p| p.auto_hide)
                    && state.recoverable()
                    && !state.protected.load(Ordering::SeqCst)
                    && !crate::hover_preview::protects_main(&app)
                    && !app
                        .get_webview_window("settings")
                        .is_some_and(|w| w.is_visible().unwrap_or(true))
                    && state.geometry_epoch.load(Ordering::SeqCst) == epoch;
                if hide {
                    if let Some(w) = app.get_webview_window("main") {
                        if !w.is_focused().unwrap_or(true) {
                            let _ = crate::hover_preview::hide(&app, None, false);
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
