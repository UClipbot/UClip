//! One reusable, read-only preview window. Events carry hints, never clipboard content.
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, Window, WindowEvent};
use uclip_core::{AppError, ErrorCode, HoverPreviewRequest};

pub const LABEL: &str = "hover-preview";
#[derive(Default)]
pub struct PreviewState {
    inner: Mutex<Inner>,
}
#[derive(Default)]
struct Inner {
    request: Option<HoverPreviewRequest>,
    main_geometry: Option<MainGeometry>,
    hovered: bool,
    // Dismissal can arrive while a slower async creation is still queued.
    cancelled: VecDeque<String>,
}

#[derive(Clone, Copy, PartialEq)]
struct MainGeometry {
    origin: Option<PhysicalPosition<i32>>,
    size: Option<PhysicalSize<u32>>,
    scale: Option<f64>,
}

impl MainGeometry {
    fn read(window: &Window) -> Self {
        Self {
            origin: if crate::desktop::wayland() {
                None
            } else {
                window.inner_position().ok()
            },
            size: window.inner_size().ok(),
            scale: window.scale_factor().ok(),
        }
    }
}

impl Inner {
    fn remember_cancelled(&mut self, token: &str) {
        if !self.cancelled.iter().any(|value| value == token) {
            self.cancelled.push_back(token.into());
            if self.cancelled.len() > 64 {
                self.cancelled.pop_front();
            }
        }
    }
    fn accept(&mut self, request: HoverPreviewRequest) -> bool {
        if self.cancelled.contains(&request.token) {
            return false;
        }
        self.request = Some(request);
        self.main_geometry = None;
        self.hovered = false;
        true
    }
    fn dismiss(
        &mut self,
        token: Option<&str>,
        unless_hovered: bool,
        focused: bool,
    ) -> (bool, Option<String>) {
        let matches = token.is_none_or(|token| {
            self.request
                .as_ref()
                .is_some_and(|request| request.token == token)
        });
        if matches && unless_hovered && (self.hovered || focused) {
            return (false, None);
        }
        let closed = self.request.as_ref().map(|request| request.token.clone());
        if let Some(token) = token.or(closed.as_deref()) {
            self.remember_cancelled(token);
        }
        if matches {
            self.request = None;
            self.main_geometry = None;
            self.hovered = false;
        }
        (matches, closed)
    }
}

fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable)
}
pub fn current(app: &tauri::AppHandle) -> Result<Option<HoverPreviewRequest>, AppError> {
    app.state::<PreviewState>()
        .inner
        .lock()
        .map(|state| state.request.clone())
        .map_err(|_| unavailable())
}
fn validate(request: &HoverPreviewRequest) -> Result<(), AppError> {
    let anchor = &request.anchor;
    if uuid::Uuid::parse_str(&request.token).is_err()
        || uuid::Uuid::parse_str(&request.id).is_err()
        || request.source.len() > 512
        || request.size.as_ref().is_some_and(|size| size.len() > 128)
        || !request
            .saved_at
            .parse::<i64>()
            .is_ok_and(|time| (0..=8_640_000_000_000_000).contains(&time))
        || [anchor.x, anchor.y, anchor.width, anchor.height]
            .into_iter()
            .any(|value| !value.is_finite() || !(0.0..=16384.0).contains(&value))
        || anchor.width < 1.0
        || anchor.height < 1.0
    {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok(())
}

pub async fn show(app: tauri::AppHandle, request: HoverPreviewRequest) -> Result<(), AppError> {
    validate(&request)?;
    let token = request.token.clone();
    {
        let state = app.state::<PreviewState>();
        let mut state = state.inner.lock().map_err(|_| unavailable())?;
        if !state.accept(request.clone()) {
            return Ok(());
        }
    }
    // Construction remains async and serialized; no business/state lock spans a window API.
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _opening = OPENING.lock().await;
    if current(&app)?.is_none_or(|value| value.token != token) {
        return Ok(());
    }
    let main = app.get_webview_window("main").ok_or_else(unavailable)?;
    if !main.is_visible().unwrap_or(false) || main.is_minimized().unwrap_or(true) {
        return hide(&app, Some(&token), false);
    }
    let window = if let Some(window) = app.get_webview_window(LABEL) {
        window
    } else {
        tauri::WebviewWindowBuilder::new(
            &app,
            LABEL,
            tauri::WebviewUrl::App("index.html?window=hover-preview".into()),
        )
        .title(title(&app))
        .inner_size(560.0, 640.0)
        .min_inner_size(320.0, 240.0)
        .always_on_top(true)
        .skip_taskbar(true)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .on_navigation(crate::local_navigation)
        .build()
        .map_err(|_| unavailable())?
    };
    if current(&app)?.is_none_or(|value| value.token != token) {
        return Ok(());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let geometry = MainGeometry::read(&main.as_ref().window());
            {
                let state = handle.state::<PreviewState>();
                let mut state = state.inner.lock().map_err(|_| unavailable())?;
                if state
                    .request
                    .as_ref()
                    .is_none_or(|value| value.token != token)
                {
                    return Ok(());
                }
                state.main_geometry = Some(geometry);
            }
            // Wayland leaves absolute placement to the compositor.
            if !crate::desktop::wayland() {
                position(&main, &window, &request)?;
            }
            window
                .set_title(title(&handle))
                .map_err(|_| unavailable())?;
            window
                .emit("hover-preview-changed", ())
                .map_err(|_| unavailable())?;
            crate::desktop::show_without_focus(&window)
        })();
        let _ = tx.send(result);
    })
    .map_err(|_| unavailable())?;
    rx.await.map_err(|_| unavailable())?
}
pub fn title(app: &tauri::AppHandle) -> &'static str {
    if app
        .state::<crate::desktop::DesktopState>()
        .ui_language
        .load(std::sync::atomic::Ordering::SeqCst)
        == 2
    {
        "\u{5185}\u{5bb9}\u{9884}\u{89c8}"
    } else {
        "Content preview"
    }
}

pub fn hide(
    app: &tauri::AppHandle,
    token: Option<&str>,
    unless_hovered: bool,
) -> Result<(), AppError> {
    let focused = app
        .get_webview_window(LABEL)
        .is_some_and(|window| window.is_focused().unwrap_or(false));
    let (should_hide, closed_token) = {
        let state = app.state::<PreviewState>();
        let mut state = state.inner.lock().map_err(|_| unavailable())?;
        state.dismiss(token, unless_hovered, focused)
    };
    if should_hide {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.hide().map_err(|_| unavailable())?;
            let _ = window.emit("hover-preview-changed", ());
        }
        if let Some(token) = closed_token {
            let _ = app.emit_to("main", "hover-preview-closed", token);
        }
    }
    Ok(())
}
pub fn presence(app: tauri::AppHandle, token: String, hovered: bool) -> Result<(), AppError> {
    {
        let state = app.state::<PreviewState>();
        let mut state = state.inner.lock().map_err(|_| unavailable())?;
        if state
            .request
            .as_ref()
            .is_none_or(|value| value.token != token)
        {
            return Ok(());
        }
        state.hovered = hovered;
    }
    if !hovered {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(450)).await;
            let _ = hide(&app, Some(&token), true);
        });
    }
    Ok(())
}
pub fn protects_main(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(LABEL).is_some_and(|window| {
        window.is_visible().unwrap_or(false)
            && (window.is_focused().unwrap_or(false)
                || app
                    .state::<PreviewState>()
                    .inner
                    .lock()
                    .is_ok_and(|state| state.hovered))
    })
}

pub fn main_geometry_changed(window: &Window) {
    let app = window.app_handle();
    let state = app.state::<PreviewState>();
    let anchor = state
        .inner
        .lock()
        .ok()
        .and_then(|state| Some((state.request.as_ref()?.token.clone(), state.main_geometry?)));
    // A window manager can repeat configure notifications when a preview is mapped.
    if let Some((token, geometry)) = anchor {
        if geometry != MainGeometry::read(window) {
            let _ = hide(app, Some(&token), false);
        }
    }
}

pub fn window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != LABEL {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = hide(window.app_handle(), None, false);
        }
        WindowEvent::Focused(false) => {
            let app = window.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !app
                    .get_webview_window("main")
                    .is_some_and(|main| main.is_focused().unwrap_or(false))
                    && !app
                        .get_webview_window(LABEL)
                        .is_some_and(|preview| preview.is_focused().unwrap_or(false))
                {
                    let _ = hide(&app, None, false);
                }
            });
        }
        _ => {}
    }
}

// All rectangles are physical screen coordinates, including negative monitor origins.
fn placement(main: [f64; 4], area: [f64; 4], size: [f64; 2], y: f64, gap: f64) -> [f64; 2] {
    let right = main[0] + main[2] + gap;
    let left = main[0] - size[0] - gap;
    let x = if right + size[0] <= area[0] + area[2] - gap {
        right
    } else if left >= area[0] + gap {
        left
    } else if area[0] + area[2] - right >= main[0] - area[0] {
        right
    } else {
        left
    };
    [
        x.clamp(
            area[0] + gap,
            (area[0] + area[2] - size[0] - gap).max(area[0] + gap),
        ),
        y.clamp(
            area[1] + gap,
            (area[1] + area[3] - size[1] - gap).max(area[1] + gap),
        ),
    ]
}
fn position(
    main: &tauri::WebviewWindow,
    window: &tauri::WebviewWindow,
    request: &HoverPreviewRequest,
) -> Result<(), AppError> {
    let origin = main.outer_position().map_err(|_| unavailable())?;
    let main_size = main.outer_size().map_err(|_| unavailable())?;
    let scale = main.scale_factor().map_err(|_| unavailable())?;
    let inner = main.inner_position().map_err(|_| unavailable())?;
    let monitor = main
        .current_monitor()
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    let area = monitor.work_area();
    let gap = 8.0 * scale;
    let current_size = window.outer_size().map_err(|_| unavailable())?;
    let size = [
        f64::from(current_size.width)
            .min(f64::from(area.size.width) - gap * 2.0)
            .max(1.0),
        f64::from(current_size.height)
            .min(f64::from(area.size.height) - gap * 2.0)
            .max(1.0),
    ];
    if size[0] < f64::from(current_size.width) || size[1] < f64::from(current_size.height) {
        let inner_size = window.inner_size().map_err(|_| unavailable())?;
        let frame_width = current_size.width.saturating_sub(inner_size.width);
        let frame_height = current_size.height.saturating_sub(inner_size.height);
        window
            .set_size(tauri::PhysicalSize::new(
                (size[0] as u32).saturating_sub(frame_width).max(1),
                (size[1] as u32).saturating_sub(frame_height).max(1),
            ))
            .map_err(|_| unavailable())?;
    }
    let point = placement(
        [
            f64::from(origin.x),
            f64::from(origin.y),
            f64::from(main_size.width),
            f64::from(main_size.height),
        ],
        [
            f64::from(area.position.x),
            f64::from(area.position.y),
            f64::from(area.size.width),
            f64::from(area.size.height),
        ],
        size,
        f64::from(inner.y) + request.anchor.y * scale,
        gap,
    );
    window
        .set_position(tauri::PhysicalPosition::new(
            point[0] as i32,
            point[1] as i32,
        ))
        .map_err(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_prefers_adjacent_space_and_clamps_to_negative_hidpi_work_areas() {
        assert_eq!(
            placement(
                [100.0, 100.0, 640.0, 650.0],
                [0.0, 0.0, 1920.0, 1080.0],
                [560.0, 640.0],
                150.0,
                8.0
            ),
            [748.0, 150.0]
        );
        assert_eq!(
            placement(
                [1100.0, 100.0, 640.0, 650.0],
                [0.0, 0.0, 1920.0, 1080.0],
                [560.0, 640.0],
                800.0,
                8.0
            ),
            [532.0, 432.0]
        );
        assert_eq!(
            placement(
                [-1280.0, 0.0, 1280.0, 1300.0],
                [-2880.0, -200.0, 2880.0, 1800.0],
                [1120.0, 1280.0],
                -400.0,
                16.0
            ),
            [-2416.0, -184.0]
        );
    }
    #[test]
    fn late_opens_and_old_closes_cannot_replace_the_current_reading_session() {
        let request = |token: &str| HoverPreviewRequest {
            token: token.into(),
            id: "fixture".into(),
            kind: uclip_core::ClipKind::Text,
            source: "Fixture".into(),
            saved_at: "1".into(),
            size: None,
            anchor: uclip_core::PreviewAnchor {
                x: 0.0,
                y: 0.0,
                width: 600.0,
                height: 56.0,
            },
        };
        let mut state = Inner::default();
        state.dismiss(Some("old"), false, false);
        assert!(!state.accept(request("old")));
        assert!(state.accept(request("current")));
        assert!(!state.dismiss(Some("old"), false, false).0);
        assert_eq!(state.request.as_ref().unwrap().token, "current");
        state.hovered = true;
        assert!(!state.dismiss(Some("current"), true, false).0);
        state.hovered = false;
        assert!(!state.dismiss(Some("current"), true, true).0);
        assert!(state.dismiss(Some("current"), false, true).0);
        assert!(state.request.is_none());
        assert!(!state.accept(request("current")));
    }
    #[test]
    fn preview_rejects_unbounded_metadata_and_invalid_geometry() {
        let mut request = HoverPreviewRequest {
            token: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            kind: uclip_core::ClipKind::Text,
            source: "Fixture".into(),
            saved_at: "1234".into(),
            size: None,
            anchor: uclip_core::PreviewAnchor {
                x: 0.0,
                y: 20.0,
                width: 600.0,
                height: 56.0,
            },
        };
        assert!(validate(&request).is_ok());
        request.anchor.x = f64::NAN;
        assert!(validate(&request).is_err());
        request.anchor.x = 0.0;
        request.source = "x".repeat(513);
        assert!(validate(&request).is_err());
    }
}
