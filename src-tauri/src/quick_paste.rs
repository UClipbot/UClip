//! Native session ownership for the focused, reusable Quick Paste window.
use crate::{desktop, paste::InvocationTarget, AppState};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use uclip_core::{
    AppError, AuxiliaryWindow, ClipKind, CopyResult, ErrorCode, QuickPastePreferences,
    QuickPasteSettings, QuickPasteSlot, QuickPasteSnapshot,
};

const LABEL: &str = "quick-paste";
const CHANGED: &str = "quick-paste-changed";
static TRANSITION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Default)]
pub struct QuickPasteState {
    settings: Mutex<Option<QuickPasteSettings>>,
    session: Mutex<Option<Session>>,
    pressed: AtomicBool,
}

struct Session {
    view: QuickPasteSnapshot,
    slots: Vec<QuickPasteSlot>,
    target: Option<InvocationTarget>,
    active: Arc<AtomicBool>,
    focused: bool,
    executing: bool,
    published: bool,
}

fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable)
}
fn target_lost() -> AppError {
    AppError::new(ErrorCode::TargetLost)
}

impl QuickPasteState {
    pub fn binding(&self) -> Option<Shortcut> {
        let settings = self.settings.lock().ok()?;
        desktop::parse_shortcut(settings.as_ref()?.preferences.shortcut.as_deref()?).ok()
    }

    fn settings(&self) -> Option<QuickPasteSettings> {
        self.settings.lock().ok()?.clone()
    }

    pub fn cache(
        &self,
        preferences: QuickPastePreferences,
        available: bool,
        error: Option<AppError>,
    ) {
        if let Ok(mut settings) = self.settings.lock() {
            *settings = Some(QuickPasteSettings {
                preferences,
                available,
                error,
            });
        }
    }
}

pub fn register(app: &tauri::AppHandle, shortcut: Shortcut) -> Result<(), AppError> {
    if desktop::wayland() {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    app.global_shortcut()
        .on_shortcut(shortcut, |app, shortcut, event| {
            let state = app.state::<QuickPasteState>();
            if state.binding() != Some(*shortcut) {
                return;
            }
            if event.state == ShortcutState::Released {
                state.pressed.store(false, Ordering::Release);
                return;
            }
            if state.pressed.swap(true, Ordering::AcqRel) {
                return;
            }
            if desktop::forward_shortcut_to_recorder(app, shortcut) {
                return;
            }
            let app = app.clone();
            let shortcut = *shortcut;
            tauri::async_runtime::spawn(async move {
                if let Err(error) = activate(&app, shortcut).await {
                    eprintln!("quick_paste_activation_failed: {:?}", error.code);
                    report(&app, &error);
                }
            });
        })
        .map_err(|_| AppError::new(ErrorCode::ShortcutConflict))
}

pub fn unregister(app: &tauri::AppHandle, shortcut: Shortcut) {
    if let Err(_error) = app.global_shortcut().unregister(shortcut) {
        eprintln!("quick_paste_shortcut_cleanup_failed");
    }
    app.state::<QuickPasteState>()
        .pressed
        .store(false, Ordering::Release);
}

pub fn prepare_window(app: &tauri::AppHandle) -> Result<WebviewWindow, AppError> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }
    let kind = AuxiliaryWindow::QuickPaste;
    let monitor = app.primary_monitor().ok().flatten();
    let size = app
        .state::<crate::auxiliary_window::SizeState>()
        .fit(kind, monitor.as_ref());
    let minimum = kind.minimum_size();
    let maximum = f64::from(AuxiliaryWindow::MAXIMUM_DIMENSION);
    let builder = crate::local_window(app, LABEL, "index.html?window=quick-paste")
        .title("UClip")
        .inner_size(f64::from(size.width), f64::from(size.height))
        .min_inner_size(f64::from(minimum.width), f64::from(minimum.height))
        .max_inner_size(maximum, maximum)
        .always_on_top(true)
        .skip_taskbar(true)
        .minimizable(false)
        .maximizable(false)
        .resizable(true)
        .decorations(cfg!(target_os = "macos"));
    #[cfg(target_os = "macos")]
    let builder = builder
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay);
    #[cfg(target_os = "linux")]
    let builder = builder.transparent(true);
    #[cfg(target_os = "windows")]
    let builder = builder.transparent(true).shadow(false);
    let window = builder.build().map_err(|_| unavailable())?;
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    crate::auxiliary_window::attach_resize_tracking(&window, kind)?;
    Ok(window)
}

fn notify(app: &tauri::AppHandle) {
    let _ = app.emit_to(LABEL, CHANGED, ());
}

fn report(app: &tauri::AppHandle, error: &AppError) {
    for window in ["main", "settings"] {
        let _ = app.emit_to(window, "quick-paste-unavailable", error.code);
    }
}

fn code(shortcut: Shortcut) -> String {
    format!("{:?}", shortcut.key)
}

fn modifiers(shortcut: Shortcut) -> u8 {
    // Shared with the panel's KeyboardEvent matcher.
    u8::from(shortcut.mods.contains(Modifiers::CONTROL))
        | (u8::from(shortcut.mods.contains(Modifiers::SHIFT)) << 1)
        | (u8::from(shortcut.mods.contains(Modifiers::ALT)) << 2)
        | (u8::from(shortcut.mods.contains(Modifiers::SUPER)) << 3)
}

fn digit_key(code: &str) -> bool {
    code.strip_prefix("Digit")
        .or_else(|| code.strip_prefix("Numpad"))
        .is_some_and(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_digit())
}

fn place(
    window: &WebviewWindow,
    pointer: PhysicalPosition<f64>,
    monitor: Option<tauri::Monitor>,
) -> Result<(), AppError> {
    let Some(monitor) = monitor else {
        return Ok(());
    };
    let work = monitor.work_area();
    let desired = window
        .app_handle()
        .state::<crate::auxiliary_window::SizeState>()
        .fit(AuxiliaryWindow::QuickPaste, Some(&monitor));
    let scale = window.scale_factor().map_err(|_| unavailable())?;
    let inner = window
        .inner_size()
        .map_err(|_| unavailable())?
        .to_logical::<f64>(scale);
    let outer = window
        .outer_size()
        .map_err(|_| unavailable())?
        .to_logical::<f64>(scale);
    let target_scale = monitor.scale_factor();
    if inner.width.round() as u32 != desired.width || inner.height.round() as u32 != desired.height
    {
        window
            .set_size(LogicalSize::new(desired.width, desired.height))
            .map_err(|_| unavailable())?;
    }
    let width = f64::from(desired.width) + (outer.width - inner.width).max(0.0);
    let height = f64::from(desired.height) + (outer.height - inner.height).max(0.0);
    let offset = (12.0 * target_scale).round() as i64;
    let x = panel_axis(
        pointer.x as i64,
        i64::from(work.position.x),
        i64::from(work.size.width),
        (width * target_scale).ceil() as i64,
        offset,
    );
    let y = panel_axis(
        pointer.y as i64,
        i64::from(work.position.y),
        i64::from(work.size.height),
        (height * target_scale).ceil() as i64,
        offset,
    );
    window
        .set_position(PhysicalPosition::new(x as i32, y as i32))
        .map_err(|_| unavailable())
}

fn panel_axis(pointer: i64, origin: i64, extent: i64, size: i64, offset: i64) -> i64 {
    let last = origin + (extent - size).max(0);
    let desired = if pointer + offset <= last {
        pointer + offset
    } else {
        pointer - size - offset
    };
    desired.clamp(origin, last)
}

async fn activate(app: &tauri::AppHandle, shortcut: Shortcut) -> Result<(), AppError> {
    let transition = TRANSITION.lock().await;
    let state = app.state::<QuickPasteState>();
    if state.binding() != Some(shortcut) {
        return Ok(());
    }
    let current = snapshot(app)?;
    if let Some(current) = current {
        if current.feedback_only {
            close_locked(app, &current.token)?;
        } else if current.final_key.is_some() {
            return expand_locked(app, Some(&current.token)).await.map(|_| ());
        } else {
            return Ok(());
        }
    }
    let desktop = app.state::<desktop::DesktopState>();
    if desktop.protected.load(Ordering::Acquire)
        || app
            .get_webview_window("settings")
            .is_some_and(|w| w.is_visible().unwrap_or(true))
    {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    let main = desktop.preferences().ok_or_else(unavailable)?.shortcut;
    let main_binding = desktop::parse_shortcut(&main)?;
    let preferences = state.settings().ok_or_else(unavailable)?.preferences;
    let core = app
        .state::<AppState>()
        .core
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let clipboard = app
        .state::<AppState>()
        .clipboard
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let pointer = app.cursor_position().map_err(|_| unavailable())?;
    let monitor = app
        .monitor_from_point(pointer.x, pointer.y)
        .map_err(|_| unavailable())?
        .or_else(|| app.primary_monitor().ok().flatten());
    let target = tauri::async_runtime::spawn_blocking(move || clipboard.invocation_target())
        .await
        .map_err(|_| unavailable())?;
    let window = prepare_window(app)?;
    if let Err(error) = crate::window_frame::wait_for_theme(&window).await {
        let _ = window.destroy();
        return Err(error);
    }
    if state.binding() != Some(shortcut) {
        return Ok(());
    }
    let token = uuid::Uuid::new_v4().to_string();
    let final_key = code(shortcut);
    *state.session.lock().map_err(|_| unavailable())? = Some(Session {
        view: QuickPasteSnapshot {
            token: token.clone(),
            items: vec![],
            loading: true,
            copy_only: target.is_none(),
            feedback_only: false,
            final_key: (!digit_key(&final_key) && final_key != "Escape").then(|| final_key.clone()),
            main_shortcut: main,
            main_code: code(main_binding),
            quick_code: final_key,
            main_modifiers: modifiers(main_binding),
            quick_modifiers: modifiers(shortcut),
            error: None,
            result: None,
        },
        slots: vec![],
        target,
        active: Arc::new(AtomicBool::new(true)),
        focused: false,
        executing: false,
        published: false,
    });
    let shown = async {
        crate::hover_preview::hide(app, None, false)?;
        place(&window, pointer, monitor)?;
        let focused_window = window.clone();
        tauri::async_runtime::spawn_blocking(move || {
            #[cfg(target_os = "linux")]
            let activation = crate::window_activation::Activation::prepare().ok();
            focused_window
                .set_focusable(true)
                .and_then(|_| focused_window.show())
                .and_then(|_| focused_window.set_focus())
                .map_err(|_| unavailable())?;
            #[cfg(target_os = "linux")]
            if let Some(activation) = activation {
                activation.request(&focused_window)?;
            }
            Ok::<_, AppError>(())
        })
        .await
        .map_err(|_| unavailable())??;
        let mut unfocused_after_release = 0;
        for _ in 0..150 {
            if window.is_focused().unwrap_or(false) {
                if let Some(main) = app.get_webview_window("main") {
                    if main.is_visible().unwrap_or(false) {
                        main.hide().map_err(|_| unavailable())?;
                    }
                }
                return Ok(());
            }
            // X11 may defer focus events while the opening key owns a passive grab.
            if !state.pressed.load(Ordering::Acquire) {
                unfocused_after_release += 1;
                if unfocused_after_release >= 30 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(12)).await;
        }
        Err(unavailable())
    }
    .await;
    if let Err(error) = shown {
        close_locked(app, &token)?;
        return Err(error);
    }
    notify(app);
    drop(transition);

    // Metadata becomes interactive without waiting for image decoding.
    let slots = core.quick_paste_slots(preferences.include_pinned).await;
    let thumbs = {
        let mut sessions = state.session.lock().map_err(|_| unavailable())?;
        let Some(session) = sessions.as_mut().filter(|s| s.view.token == token) else {
            return Ok(());
        };
        session.view.loading = false;
        match slots {
            Ok(slots) => {
                session.view.items = slots.iter().map(|slot| slot.item.clone()).collect();
                session.slots = slots;
            }
            Err(error) => session.view.error = Some(error),
        }
        session
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.item.kind == ClipKind::Image)
            .map(|(index, slot)| (index, slot.id.clone(), slot.revision.clone()))
            .collect::<Vec<_>>()
    };
    notify(app);
    for (index, id, revision) in thumbs {
        if snapshot(app)?.is_none_or(|view| view.token != token) {
            break;
        }
        let Ok(payload) = core.read_quick_paste_slot(id.clone(), revision).await else {
            continue;
        };
        let Ok(Ok(detail)) = tauri::async_runtime::spawn_blocking(move || {
            crate::clipboard::detail(id, payload, true)
        })
        .await
        else {
            continue;
        };
        {
            let mut sessions = state.session.lock().map_err(|_| unavailable())?;
            let Some(session) = sessions.as_mut().filter(|s| s.view.token == token) else {
                break;
            };
            session.view.items[index].thumbnail = detail.image_data_url;
        }
        notify(app);
    }
    Ok(())
}

fn snapshot(app: &tauri::AppHandle) -> Result<Option<QuickPasteSnapshot>, AppError> {
    Ok(app
        .state::<QuickPasteState>()
        .session
        .lock()
        .map_err(|_| unavailable())?
        .as_ref()
        .map(|session| session.view.clone()))
}

fn close_locked(app: &tauri::AppHandle, token: &str) -> Result<(), AppError> {
    {
        let state = app.state::<QuickPasteState>();
        let mut sessions = state.session.lock().map_err(|_| unavailable())?;
        let Some(session) = sessions
            .as_ref()
            .filter(|session| session.view.token == token)
        else {
            return Ok(());
        };
        session.active.store(false, Ordering::Release);
        *sessions = None;
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        window.hide().map_err(|_| unavailable())?;
    }
    notify(app);
    Ok(())
}

async fn expand_locked(app: &tauri::AppHandle, token: Option<&str>) -> Result<bool, AppError> {
    let state = app.state::<QuickPasteState>();
    let (id, active) = {
        let sessions = state.session.lock().map_err(|_| unavailable())?;
        let Some(session) = sessions
            .as_ref()
            .filter(|s| token.is_none_or(|token| s.view.token == token))
        else {
            return Ok(false);
        };
        if session.executing {
            return Ok(true);
        }
        (
            session.view.token.clone(),
            session.active.load(Ordering::Acquire),
        )
    };
    if !active {
        close_locked(app, &id)?;
        return Ok(false);
    }
    // Focus main first: an intermediate external focus transition can retarget paste.
    let window_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || desktop::show(&window_app, false))
        .await
        .map_err(|_| unavailable())??;
    close_locked(app, &id)?;
    Ok(true)
}

pub async fn show_main(app: tauri::AppHandle) -> Result<(), AppError> {
    let _transition = TRANSITION.lock().await;
    if expand_locked(&app, None).await? {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(clipboard) = &state.clipboard {
                clipboard.capture_target();
            }
        }
        desktop::show(&app, true)
    })
    .await
    .map_err(|_| unavailable())?
}

pub fn window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != LABEL {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<QuickPasteState>();
    let token = {
        let Ok(mut sessions) = state.session.lock() else {
            return;
        };
        let Some(session) = sessions.as_mut() else {
            return;
        };
        match event {
            WindowEvent::Focused(true) => {
                session.focused = true;
                return;
            }
            WindowEvent::CloseRequested { api, .. } => api.prevent_close(),
            WindowEvent::Focused(false) if session.focused && !session.view.feedback_only => {}
            WindowEvent::Destroyed => {}
            _ => return,
        }
        session.active.store(false, Ordering::Release);
        if session.executing {
            return;
        }
        session.view.token.clone()
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _transition = TRANSITION.lock().await;
        let _ = close_locked(&app, &token);
    });
}

pub fn settings(app: &tauri::AppHandle) -> Result<QuickPasteSettings, AppError> {
    app.state::<QuickPasteState>()
        .settings()
        .ok_or_else(unavailable)
}

#[tauri::command]
pub fn quick_paste_snapshot(app: tauri::AppHandle) -> Result<Option<QuickPasteSnapshot>, AppError> {
    snapshot(&app)
}

#[tauri::command]
pub async fn expand_quick_paste(window: WebviewWindow, token: String) -> Result<(), AppError> {
    let _transition = TRANSITION.lock().await;
    if !window.is_focused().unwrap_or(false)
        || snapshot(window.app_handle())?.is_none_or(|s| s.token != token || s.final_key.is_none())
    {
        return Err(target_lost());
    }
    expand_locked(window.app_handle(), Some(&token)).await?;
    Ok(())
}

#[tauri::command]
pub async fn cancel_quick_paste(app: tauri::AppHandle, token: String) -> Result<(), AppError> {
    let _transition = TRANSITION.lock().await;
    // Hiding a focused popup leaves focus restoration to the window manager.
    close_locked(&app, &token)
}

async fn show_feedback(window: &WebviewWindow, token: &str) -> Result<(), AppError> {
    let app = window.app_handle();
    let (send, completion) = tokio::sync::oneshot::channel();
    let window = window.clone();
    app.run_on_main_thread(move || {
        let result = window
            .set_focusable(false)
            .map_err(|_| unavailable())
            .and_then(|_| crate::desktop::show_without_focus(&window));
        let _ = send.send(result);
    })
    .map_err(|_| unavailable())?;
    completion.await.map_err(|_| unavailable())??;
    let app = app.clone();
    let token = token.to_owned();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        let _transition = TRANSITION.lock().await;
        let _ = close_locked(&app, &token);
    });
    Ok(())
}

#[tauri::command]
pub async fn select_quick_paste(
    window: WebviewWindow,
    token: String,
    slot: usize,
) -> Result<CopyResult, AppError> {
    let app = window.app_handle();
    let clipboard = app
        .state::<AppState>()
        .clipboard
        .as_ref()
        .map_err(Clone::clone)?
        .clone();
    let (id, revision, target, active) = {
        let _transition = TRANSITION.lock().await;
        if !window.is_focused().unwrap_or(false) {
            return Err(target_lost());
        }
        let state = app.state::<QuickPasteState>();
        let mut sessions = state.session.lock().map_err(|_| unavailable())?;
        let session = sessions
            .as_mut()
            .filter(|s| s.view.token == token && s.active.load(Ordering::Acquire))
            .ok_or_else(target_lost)?;
        if session.executing || session.view.loading || session.published {
            return Err(AppError::new(ErrorCode::Busy));
        }
        let item = session
            .slots
            .get(slot)
            .ok_or_else(|| AppError::new(ErrorCode::InvalidInput))?;
        session.executing = true;
        (
            item.id.clone(),
            item.revision.clone(),
            session.target,
            session.active.clone(),
        )
    };
    let result = clipboard
        .quick_paste(id, revision, target, active.clone())
        .await;
    let _transition = TRANSITION.lock().await;
    let unfocused = !active.load(Ordering::Acquire) || !window.is_focused().unwrap_or(false);
    let copied_with_warning = result
        .as_ref()
        .ok()
        .filter(|result| result.copied)
        .and_then(|result| result.warning.as_ref());
    let feedback_only = unfocused && copied_with_warning.is_some();
    if !feedback_only
        && (unfocused
            || result
                .as_ref()
                .is_ok_and(|result| result.paste_requested && result.warning.is_none()))
    {
        close_locked(app, &token)?;
        return result;
    }
    {
        let state = app.state::<QuickPasteState>();
        let mut sessions = state.session.lock().map_err(|_| unavailable())?;
        let Some(session) = sessions.as_mut().filter(|s| s.view.token == token) else {
            return result;
        };
        session.executing = false;
        session.published = result.is_ok();
        session.view.result = result.as_ref().ok().cloned();
        session.view.error = result.as_ref().err().cloned();
        if feedback_only {
            session.active.store(false, Ordering::Release);
            session.target = None;
            session.slots.clear();
            session.view.items.clear();
            session.view.feedback_only = true;
        }
    }
    notify(app);
    if let Some(warning) = copied_with_warning.filter(|_| feedback_only) {
        if show_feedback(&window, &token).await.is_err() {
            let _ = close_locked(app, &token);
            report(app, warning);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_at_monitor_edges_including_negative_desktop_coordinates() {
        assert_eq!(panel_axis(-25, -1920, 1920, 360, 12), -397);
        assert_eq!(panel_axis(-1900, -1920, 1920, 360, 12), -1888);
        assert_eq!(panel_axis(99, 0, 100, 360, 12), 0);
    }

    #[test]
    fn digits_never_get_local_expansion() {
        for key in ["Digit0", "Digit9", "Numpad1"] {
            assert!(digit_key(key));
        }
        for key in ["KeyV", "F1", "NumpadEnter"] {
            assert!(!digit_key(key));
        }
    }
}
