use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

pub(crate) const LABEL: &str = "tray-menu";
const WIDTH: f64 = 272.0;
const HEIGHT: f64 = 248.0;
const GAP: f64 = 6.0;

#[cfg(not(target_os = "macos"))]
type Anchor = Option<tauri::PhysicalPosition<f64>>;
#[cfg(target_os = "macos")]
type Anchor = objc2_foundation::NSPoint;

#[cfg(not(target_os = "linux"))]
pub fn open_from_tray(tray: &tauri::tray::TrayIcon<tauri::Wry>) {
    let app = tray.app_handle();
    #[cfg(target_os = "windows")]
    let cursor = app.cursor_position().ok();
    #[cfg(target_os = "macos")]
    let cursor = {
        // tray-icon clears its highlight on left release only.
        let _ = tray.with_inner_tray_icon(|inner| {
            if let (Some(main), Some(item)) =
                (objc2::MainThreadMarker::new(), inner.ns_status_item())
            {
                if let Some(button) = item.button(main) {
                    button.highlight(false);
                }
            }
        });
        objc2_app_kit::NSEvent::mouseLocation()
    };
    open(app, cursor);
}

pub fn open(app: &AppHandle, cursor: Anchor) {
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _opening = OPENING.lock().await;
        #[cfg(target_os = "linux")]
        let activation = crate::window_activation::Activation::prepare().ok();
        let window = if let Some(window) = app.get_webview_window(LABEL) {
            window
        } else {
            let window =
                crate::window_frame::builder(&app, LABEL, "index.html?window=tray-menu", false)
                    .title("UClip Menu")
                    .inner_size(WIDTH, HEIGHT)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .visible(false)
                    .build();
            let window = match window {
                Ok(window) => window,
                Err(error) => {
                    eprintln!("tray_menu_window_failed: {error}");
                    return;
                }
            };
            if let Err(error) = crate::window_frame::prepare(&window, false).await {
                let _ = window.destroy();
                eprintln!("tray_menu_window_failed: {error}");
                return;
            }
            window
        };
        #[cfg(target_os = "linux")]
        let focus_window = window.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            position(&window, cursor);
            if let Err(error) = window.show().and_then(|_| window.set_focus()) {
                let _ = window.hide();
                eprintln!("tray_menu_show_failed: {error}");
            }
        }) {
            eprintln!("tray_menu_dispatch_failed: {error}");
        }
        #[cfg(target_os = "linux")]
        if let Some(activation) = activation {
            // X11 window managers can ignore activation before a newly shown window is mapped.
            tokio::time::sleep(std::time::Duration::from_millis(70)).await;
            if focus_window.is_visible().unwrap_or(false)
                && !focus_window.is_focused().unwrap_or(false)
            {
                let _ = activation.request(&focus_window);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn position(window: &WebviewWindow, cursor: Anchor) {
    let Some((cursor, monitor)) = cursor.and_then(|cursor| {
        window
            .monitor_from_point(cursor.x, cursor.y)
            .ok()
            .flatten()
            .map(|monitor| (cursor, monitor))
    }) else {
        let _ = window.center();
        return;
    };
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let x = popup_axis(
        cursor.x,
        work.position.x.into(),
        work.size.width.into(),
        WIDTH * scale,
        GAP * scale,
    );
    let y = popup_axis(
        cursor.y,
        work.position.y.into(),
        work.size.height.into(),
        HEIGHT * scale,
        GAP * scale,
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ));
}

#[cfg(target_os = "macos")]
fn position(window: &WebviewWindow, cursor: Anchor) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScreen, NSWindow};
    use objc2_foundation::NSPoint;

    // AppKit coordinates keep the cursor, work area and window in the same units on mixed-DPI displays.
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: this main-thread callback owns the live Tauri window for the duration of the borrow.
    let Some(native) = (unsafe { pointer.cast::<NSWindow>().as_ref() }) else {
        return;
    };
    for screen in NSScreen::screens(main).iter() {
        let frame = screen.frame();
        if cursor.x >= frame.origin.x
            && cursor.x < frame.origin.x + frame.size.width
            && cursor.y >= frame.origin.y
            && cursor.y < frame.origin.y + frame.size.height
        {
            let work = screen.visibleFrame();
            let x = popup_axis(cursor.x, work.origin.x, work.size.width, WIDTH, GAP);
            let y = -popup_axis(
                -cursor.y,
                -(work.origin.y + work.size.height),
                work.size.height,
                HEIGHT,
                GAP,
            );
            native.setFrameTopLeftPoint(NSPoint::new(x, y));
            return;
        }
    }
    let _ = window.center();
}

fn popup_axis(cursor: f64, start: f64, extent: f64, size: f64, gap: f64) -> f64 {
    let end = start + (extent - size).max(0.0);
    let after = cursor + gap;
    let preferred = if after <= end {
        after
    } else {
        cursor - size - gap
    };
    preferred.clamp(start, end)
}

pub fn window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != LABEL {
        return;
    }
    match event {
        WindowEvent::Focused(false) if !window.is_focused().unwrap_or(false) => {
            let _ = window.hide();
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::popup_axis;

    #[test]
    fn tray_popup_flips_and_stays_inside_the_work_area() {
        assert_eq!(popup_axis(1900.0, 0.0, 1920.0, 272.0, 6.0), 1622.0);
        assert_eq!(popup_axis(1080.0, 0.0, 1040.0, 248.0, 6.0), 792.0);
        assert_eq!(popup_axis(-1900.0, -1920.0, 1920.0, 272.0, 6.0), -1894.0);
        assert_eq!(popup_axis(3.0, 30.0, 1080.0, 248.0, 6.0), 30.0);
    }
}
