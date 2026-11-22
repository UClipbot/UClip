//! Sizes are stored in logical pixels and are owned by the native windows.
use crate::AppState;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use tauri::WebviewWindow;
use tauri::{Manager, Monitor, Window, WindowEvent};
use uclip_core::{AppError, AuxiliaryWindow, AuxiliaryWindowSize, CoreService, ErrorCode};

#[derive(Default)]
pub struct SizeState {
    settings: TrackedSize,
    quick: TrackedSize,
}

#[derive(Default)]
struct TrackedSize {
    geometry: Mutex<Geometry>,
    epoch: AtomicU64,
    writer: tokio::sync::Mutex<()>,
    #[cfg(target_os = "linux")]
    x11_drag: AtomicU64,
}

#[derive(Default)]
struct Geometry {
    preferred: Option<AuxiliaryWindowSize>,
    persisted: Option<AuxiliaryWindowSize>,
    #[cfg(target_os = "linux")]
    drag_request: Option<(AuxiliaryWindowSize, std::time::Instant)>,
}

impl SizeState {
    fn tracked(&self, kind: AuxiliaryWindow) -> &TrackedSize {
        match kind {
            AuxiliaryWindow::Settings => &self.settings,
            AuxiliaryWindow::QuickPaste => &self.quick,
        }
    }

    pub async fn restore(&self, core: &CoreService) {
        for kind in [AuxiliaryWindow::Settings, AuxiliaryWindow::QuickPaste] {
            match core.auxiliary_window_size(kind).await {
                Ok(size) => {
                    if let Ok(mut geometry) = self.tracked(kind).geometry.lock() {
                        geometry.preferred = Some(size);
                        geometry.persisted = Some(size);
                    }
                }
                Err(error) => eprintln!(
                    "auxiliary_size_restore_failed: {:?}: {}",
                    kind, error.message
                ),
            }
        }
    }

    /// Fit a presentation to its monitor while retaining the user's preferred size.
    pub fn fit(&self, kind: AuxiliaryWindow, monitor: Option<&Monitor>) -> AuxiliaryWindowSize {
        #[cfg(target_os = "linux")]
        self.cancel_resize(kind);
        let Ok(preferred) = self
            .tracked(kind)
            .geometry
            .lock()
            .map(|geometry| geometry.preferred.unwrap_or_else(|| kind.default_size()))
        else {
            return kind.default_size();
        };
        if let Some(monitor) = monitor {
            let work = monitor.work_area().size;
            let scale = monitor.scale_factor();
            let minimum = kind.minimum_size();
            AuxiliaryWindowSize {
                width: preferred
                    .width
                    .min((f64::from(work.width) / scale).floor() as u32)
                    .max(minimum.width),
                height: preferred
                    .height
                    .min((f64::from(work.height) / scale).floor() as u32)
                    .max(minimum.height),
            }
        } else {
            preferred
        }
    }

    #[cfg(target_os = "linux")]
    fn cancel_resize(&self, kind: AuxiliaryWindow) {
        let tracked = self.tracked(kind);
        tracked.x11_drag.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut geometry) = tracked.geometry.lock() {
            geometry.drag_request = None;
        }
    }
}

fn kind_for(label: &str) -> Option<AuxiliaryWindow> {
    match label {
        "settings" => Some(AuxiliaryWindow::Settings),
        "quick-paste" => Some(AuxiliaryWindow::QuickPaste),
        _ => None,
    }
}

async fn persist(app: &tauri::AppHandle, kind: AuxiliaryWindow) {
    let Some(core) = app
        .try_state::<AppState>()
        .and_then(|state| state.core.as_ref().ok().cloned())
    else {
        return;
    };
    let sizes = app.state::<SizeState>();
    let tracked = sizes.tracked(kind);
    // Serialize writes so a delayed save cannot overwrite a newer resize.
    let _writer = tracked.writer.lock().await;
    let pending = tracked.geometry.lock().ok().and_then(|geometry| {
        geometry
            .preferred
            .filter(|size| Some(*size) != geometry.persisted)
    });
    if let Some(size) = pending {
        match core.save_auxiliary_window_size(kind, size).await {
            Ok(()) => {
                if let Ok(mut geometry) = tracked.geometry.lock() {
                    geometry.persisted = Some(size);
                }
            }
            Err(error) => eprintln!("auxiliary_size_save_failed: {:?}: {}", kind, error.message),
        }
    }
}

pub async fn save_pending(app: &tauri::AppHandle) {
    for kind in [AuxiliaryWindow::Settings, AuxiliaryWindow::QuickPaste] {
        persist(app, kind).await;
    }
}

pub fn window_event(window: &Window, event: &WindowEvent) {
    let Some(kind) = kind_for(window.label()) else {
        return;
    };
    let app = window.app_handle();
    #[cfg(target_os = "linux")]
    if matches!(
        event,
        WindowEvent::ScaleFactorChanged { .. }
            | WindowEvent::Destroyed
            | WindowEvent::CloseRequested { .. }
    ) {
        app.state::<SizeState>().cancel_resize(kind);
    }
    if matches!(event, WindowEvent::CloseRequested { .. }) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { persist(&app, kind).await });
    } else {
        #[cfg(not(target_os = "windows"))]
        track_resize(window, event, kind);
    }
}

#[cfg(not(target_os = "windows"))]
fn track_resize(window: &Window, event: &WindowEvent, kind: AuxiliaryWindow) {
    let WindowEvent::Resized(_size) = event else {
        return;
    };
    #[cfg(target_os = "linux")]
    if !crate::desktop::wayland() {
        return;
    }
    if !window.is_visible().unwrap_or(false)
        || window.is_maximized().unwrap_or(true)
        || window.is_fullscreen().unwrap_or(true)
        || window.is_minimized().unwrap_or(true)
    {
        return;
    }
    #[cfg(target_os = "linux")]
    let size = {
        use gtk::prelude::*;
        let Ok(native) = window.gtk_window() else {
            return;
        };
        let (width, height) = native.size();
        let size = AuxiliaryWindowSize {
            width: width as u32,
            height: height as u32,
        };
        let sizes = window.state::<SizeState>();
        let acknowledged = sizes
            .tracked(kind)
            .geometry
            .lock()
            .is_ok_and(|mut geometry| {
                geometry
                    .drag_request
                    .take_if(|(expected, requested)| {
                        *expected == size && requested.elapsed() < Duration::from_secs(1)
                    })
                    .is_some()
            });
        if !acknowledged {
            return;
        }
        size
    };
    #[cfg(target_os = "macos")]
    let size = {
        if objc2::MainThreadMarker::new().is_none() {
            return;
        }
        let Ok(pointer) = window.ns_window() else {
            return;
        };
        // SAFETY: the resize callback supplies a live NSWindow on the main thread.
        let Some(native) = (unsafe { pointer.cast::<objc2_app_kit::NSWindow>().as_ref() }) else {
            return;
        };
        if !native.inLiveResize() {
            return;
        }
        let Ok(scale) = window.scale_factor() else {
            return;
        };
        logical_size(*_size, scale)
    };
    remember_size(window.app_handle(), kind, size);
}

fn logical_size(size: tauri::PhysicalSize<u32>, scale: f64) -> AuxiliaryWindowSize {
    let logical = size.to_logical::<f64>(scale);
    AuxiliaryWindowSize {
        width: logical.width.round() as u32,
        height: logical.height.round() as u32,
    }
}

fn remember_size(app: &tauri::AppHandle, kind: AuxiliaryWindow, size: AuxiliaryWindowSize) {
    if !kind.accepts(size) {
        return;
    }
    let sizes = app.state::<SizeState>();
    let tracked = sizes.tracked(kind);
    {
        let Ok(mut geometry) = tracked.geometry.lock() else {
            return;
        };
        if geometry.preferred == Some(size) {
            return;
        }
        geometry.preferred = Some(size);
    }
    let epoch = tracked
        .epoch
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if app
            .state::<SizeState>()
            .tracked(kind)
            .epoch
            .load(Ordering::Relaxed)
            == epoch
        {
            persist(&app, kind).await;
        }
    });
}

#[cfg(target_os = "linux")]
fn expect_gtk_size(
    app: &tauri::AppHandle,
    kind: AuxiliaryWindow,
    size: Option<AuxiliaryWindowSize>,
) {
    if let Ok(mut geometry) = app.state::<SizeState>().tracked(kind).geometry.lock() {
        geometry.drag_request = size.map(|size| (size, std::time::Instant::now()));
    }
}

#[cfg(target_os = "linux")]
pub fn attach_resize_tracking(
    window: &WebviewWindow,
    kind: AuxiliaryWindow,
) -> Result<(), AppError> {
    use gtk::{gdk, glib::Propagation, prelude::*};
    use std::{cell::Cell, rc::Rc};

    if !crate::desktop::wayland() {
        return Ok(());
    }
    let app = window.app_handle().clone();
    window
        .with_webview(move |platform| {
            let view = platform.inner();
            view.add_events(
                gdk::EventMask::BUTTON_PRESS_MASK
                    | gdk::EventMask::BUTTON_RELEASE_MASK
                    | gdk::EventMask::BUTTON1_MOTION_MASK,
            );
            let native = view
                .toplevel()
                .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                .map(|window| window.downgrade());
            let Some(native) = native else {
                return;
            };
            let drag = Rc::new(Cell::new(None));
            let press = drag.clone();
            let toplevel = native.clone();
            let press_app = app.clone();
            view.connect_button_press_event(move |view, event| {
                let (x, y) = event.position();
                // Keep this hit area aligned with .window-resize-grip.
                let right = f64::from(view.allocated_width());
                let bottom = f64::from(view.allocated_height());
                if event.button() != 1
                    || !(right - 24.0..right).contains(&x)
                    || !(bottom - 24.0..bottom).contains(&y)
                {
                    return Propagation::Proceed;
                }
                let Some(window) = toplevel.upgrade() else {
                    return Propagation::Proceed;
                };
                if window.window().is_none_or(|native| {
                    native
                        .state()
                        .intersects(gdk::WindowState::MAXIMIZED | gdk::WindowState::FULLSCREEN)
                }) {
                    return Propagation::Proceed;
                }
                expect_gtk_size(&press_app, kind, None);
                press.set(Some((event.root(), window.size(), window.scale_factor())));
                Propagation::Stop
            });
            let motion = drag.clone();
            view.connect_motion_notify_event(move |_, event| {
                let Some(((start_x, start_y), (width, height), scale)) = motion.get() else {
                    return Propagation::Proceed;
                };
                if !event.state().contains(gdk::ModifierType::BUTTON1_MASK) {
                    motion.set(None);
                    return Propagation::Proceed;
                }
                if let Some(window) = native
                    .upgrade()
                    .filter(|window| window.scale_factor() == scale)
                {
                    let (x, y) = event.root();
                    let minimum = kind.minimum_size();
                    let maximum = AuxiliaryWindow::MAXIMUM_DIMENSION as i32;
                    let width =
                        (width + (x - start_x).round() as i32).clamp(minimum.width as i32, maximum);
                    let height = (height + (y - start_y).round() as i32)
                        .clamp(minimum.height as i32, maximum);
                    expect_gtk_size(
                        &app,
                        kind,
                        Some(AuxiliaryWindowSize {
                            width: width as u32,
                            height: height as u32,
                        }),
                    );
                    window.resize(width, height);
                } else {
                    motion.set(None);
                    expect_gtk_size(&app, kind, None);
                }
                Propagation::Stop
            });
            let release = drag.clone();
            view.connect_button_release_event(move |_, event| {
                if event.button() == 1 && release.take().is_some() {
                    return Propagation::Stop;
                }
                Propagation::Proceed
            });
            view.connect_grab_broken_event(move |_, _| {
                drag.set(None);
                Propagation::Proceed
            });
        })
        .map_err(|_| AppError::new(ErrorCode::Unavailable))
}

#[cfg(target_os = "linux")]
fn resize_x11(window: Window, kind: AuxiliaryWindow) -> Result<(), AppError> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{ConnectionExt, KeyButMask},
    };

    let unavailable = || AppError::new(ErrorCode::Unavailable);
    let sizes = window.state::<SizeState>();
    let drag = &sizes.tracked(kind).x11_drag;
    let generation = drag.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let (connection, screen) = x11rb::connect(None).map_err(|_| unavailable())?;
    let root = connection.setup().roots[screen].root;
    let pointer = || {
        connection
            .query_pointer(root)
            .map_err(|_| unavailable())?
            .reply()
            .map_err(|_| unavailable())
    };
    let start = pointer()?;
    if !start.mask.contains(KeyButMask::BUTTON1) {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    let initial = window.inner_size().map_err(|_| unavailable())?;
    let scale = window.scale_factor().map_err(|_| unavailable())?;
    window
        .start_resize_dragging(tauri_runtime::ResizeDirection::SouthEast)
        .map_err(|_| unavailable())?;
    let minimum = kind.minimum_size();
    let maximum = AuxiliaryWindow::MAXIMUM_DIMENSION;
    let requested = |initial: u32, delta: i32, minimum: u32| {
        let lower = (f64::from(minimum) * scale).round() as u32;
        let upper = (f64::from(maximum) * scale).round() as u32;
        initial.saturating_add_signed(delta).clamp(lower, upper)
    };
    let mut confirmed = initial;
    loop {
        if drag.load(Ordering::Relaxed) != generation
            || !window.is_visible().unwrap_or(false)
            || window.scale_factor().ok() != Some(scale)
        {
            return Ok(());
        }
        let next = pointer()?;
        let size = window.inner_size().map_err(|_| unavailable())?;
        let width = requested(
            initial.width,
            i32::from(next.root_x) - i32::from(start.root_x),
            minimum.width,
        );
        let height = requested(
            initial.height,
            i32::from(next.root_y) - i32::from(start.root_y),
            minimum.height,
        );
        // Match the user's pointer displacement before accepting the WM's configure.
        if size != confirmed
            && size.width.abs_diff(width) <= 2
            && size.height.abs_diff(height) <= 2
            && drag.load(Ordering::Relaxed) == generation
            && !window.is_maximized().unwrap_or(true)
            && !window.is_fullscreen().unwrap_or(true)
        {
            confirmed = size;
            remember_size(window.app_handle(), kind, logical_size(size, scale));
        }
        if !next.mask.contains(KeyButMask::BUTTON1) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "windows")]
pub use windows_resize::attach_resize_tracking;

#[cfg(target_os = "windows")]
mod windows_resize {
    use super::*;
    use std::cell::Cell;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SIZING,
            },
        },
    };

    struct Drag {
        window: WebviewWindow,
        kind: AuxiliaryWindow,
        sizing: Cell<bool>,
    }

    pub fn attach_resize_tracking(
        window: &WebviewWindow,
        kind: AuxiliaryWindow,
    ) -> Result<(), AppError> {
        let handle = window.clone();
        window
            .run_on_main_thread(move || {
                let Ok(hwnd) = handle.hwnd() else {
                    return;
                };
                let drag = Box::into_raw(Box::new(Drag {
                    window: handle,
                    kind,
                    sizing: Cell::new(false),
                }));
                // SAFETY: the subclass is installed on the owner thread. Its context is freed on destruction.
                if !unsafe { SetWindowSubclass(HWND(hwnd.0), Some(resize_event), 0, drag as usize) }
                    .as_bool()
                {
                    // SAFETY: installation failed, so Windows will not use the context.
                    drop(unsafe { Box::from_raw(drag) });
                    eprintln!("auxiliary_resize_tracking_failed");
                }
            })
            .map_err(|_| AppError::new(ErrorCode::Unavailable))
    }

    unsafe extern "system" fn resize_event(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        id: usize,
        data: usize,
    ) -> LRESULT {
        if message == WM_NCDESTROY {
            // SAFETY: the subclass owns the box and receives the last message on the owner thread.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(resize_event), id);
                drop(Box::from_raw(data as *mut Drag));
                return DefSubclassProc(hwnd, message, wparam, lparam);
            }
        }
        // SAFETY: data belongs to this subclass and remains live until WM_NCDESTROY.
        let drag = unsafe { &*(data as *const Drag) };
        match message {
            WM_ENTERSIZEMOVE | WM_DPICHANGED => drag.sizing.set(false),
            WM_SIZING => drag.sizing.set(true),
            WM_EXITSIZEMOVE if drag.sizing.replace(false) => {
                let window = &drag.window;
                if !window.is_maximized().unwrap_or(true)
                    && !window.is_fullscreen().unwrap_or(true)
                    && !window.is_minimized().unwrap_or(true)
                {
                    if let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) {
                        remember_size(window.app_handle(), drag.kind, logical_size(size, scale));
                    }
                }
            }
            _ => {}
        }
        // SAFETY: forward the current message to the remaining subclasses and original procedure.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}

/// Only the two local auxiliary windows can start a native drag on themselves.
#[tauri::command]
pub async fn resize_auxiliary_corner(window: Window) -> Result<(), AppError> {
    let Some(_kind) = kind_for(window.label()) else {
        return Err(AppError::new(ErrorCode::Restricted));
    };
    if !window.is_visible().unwrap_or(false)
        || window.is_maximized().unwrap_or(true)
        || window.is_fullscreen().unwrap_or(true)
    {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    #[cfg(target_os = "macos")]
    return Err(AppError::new(ErrorCode::Restricted));

    #[cfg(target_os = "linux")]
    {
        if crate::desktop::wayland() {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        tauri::async_runtime::spawn_blocking(move || resize_x11(window, _kind))
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
    }

    #[cfg(target_os = "windows")]
    window
        .start_resize_dragging(tauri_runtime::ResizeDirection::SouthEast)
        .map_err(|_| AppError::new(ErrorCode::Unavailable))
}
