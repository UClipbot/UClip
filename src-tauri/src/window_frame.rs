use crate::local_window;
use tauri::{AppHandle, WebviewWindow, WebviewWindowBuilder, Wry};
use uclip_core::AppError;
#[cfg(target_os = "linux")]
use {tauri::Manager, uclip_core::ErrorCode};

pub fn builder<'a>(
    app: &'a AppHandle,
    label: &str,
    page: &str,
    native_titlebar: bool,
) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    let builder = local_window(app, label, page);
    let native_titlebar = crate::desktop::native_titlebar_enabled(native_titlebar);
    let builder = builder.decorations(cfg!(target_os = "macos") || native_titlebar);

    #[cfg(target_os = "windows")]
    let (builder, mode) = (builder.transparent(true).shadow(false), "inset");
    #[cfg(target_os = "linux")]
    let (builder, mode) = if app.state::<LinuxFrame>().composited {
        (builder.transparent(true), "inset")
    } else {
        (builder, "flush")
    };
    #[cfg(target_os = "macos")]
    let (builder, mode) = (
        builder
            .visible(false)
            .hidden_title(!native_titlebar)
            .title_bar_style(crate::desktop::title_bar_style(native_titlebar)),
        "system",
    );

    builder.initialization_script(format!(
        "window.__UCLIP_WINDOW_FRAME__ = {{ mode: '{mode}', nativeTitlebar: {native_titlebar}, canUseNativeTitlebar: {} }};",
        crate::desktop::native_titlebar_enabled(true)
    ))
}

pub async fn prepare(window: &WebviewWindow, native_titlebar: bool) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    crate::desktop::set_titlebar(window, native_titlebar).await?;
    #[cfg(not(target_os = "macos"))]
    let _ = native_titlebar;

    #[cfg(target_os = "linux")]
    if !window.app_handle().state::<LinuxFrame>().composited {
        window
            .with_webview(|webview| {
                use gtk::prelude::*;
                let Some(window) = webview
                    .inner()
                    .toplevel()
                    .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                else {
                    return;
                };
                window.connect_size_allocate(|window, _| update_shape(window));
                window.connect_map(update_shape);
                window.connect_notify_local(Some("decorated"), |window, _| update_shape(window));
                window.connect_window_state_event(|window, _| {
                    update_shape(window);
                    gtk::glib::Propagation::Proceed
                });
                update_shape(&window);
            })
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
    }

    #[cfg(target_os = "windows")]
    let _ = window;
    Ok(())
}

#[cfg(target_os = "linux")]
pub struct LinuxFrame {
    composited: bool,
}

#[cfg(target_os = "linux")]
impl LinuxFrame {
    pub fn detect() -> Self {
        Self {
            composited: crate::desktop::wayland()
                || gtk::gdk::Screen::default().is_some_and(|screen| screen.is_composited()),
        }
    }
}

#[cfg(target_os = "linux")]
fn update_shape(window: &gtk::Window) {
    use gtk::{cairo, gdk::WindowState, prelude::*};

    let Some(native) = window.window() else {
        return;
    };
    if !native.display().supports_shapes() {
        return;
    }
    if window.is_decorated()
        || native
            .state()
            .intersects(WindowState::MAXIMIZED | WindowState::FULLSCREEN)
    {
        native.shape_combine_region(None, 0, 0);
        return;
    }

    // X11 without compositing uses a window shape for the same radius as the CSS.
    let width = native.width();
    let height = native.height();
    let radius = 13.min(width / 2).min(height / 2);
    let mut rows = Vec::with_capacity((2 * radius + 1) as usize);
    rows.push(cairo::RectangleInt::new(
        0,
        radius,
        width,
        height - 2 * radius,
    ));
    for y in 0..radius {
        let dy = f64::from(radius - y) - 0.5;
        let dx = f64::from(radius) - (f64::from(radius * radius) - dy * dy).sqrt();
        let inset = dx.round() as i32;
        rows.push(cairo::RectangleInt::new(inset, y, width - 2 * inset, 1));
        rows.push(cairo::RectangleInt::new(
            inset,
            height - 1 - y,
            width - 2 * inset,
            1,
        ));
    }
    native.shape_combine_region(Some(&cairo::Region::create_rectangles(&rows)), 0, 0);
}
