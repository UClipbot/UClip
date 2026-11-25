use std::sync::atomic::Ordering;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Manager, Wry,
};
use uclip_core::{AppError, Theme, UpdateCapturePolicy, UpdateSettings};

use crate::{commands, desktop, AppState};

const THEMES: [(&str, Theme); 3] = [
    ("tray-theme-system", Theme::System),
    ("tray-theme-light", Theme::Light),
    ("tray-theme-dark", Theme::Dark),
];

struct TrayMenu {
    show: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    capture: CheckMenuItem<Wry>,
    theme: Submenu<Wry>,
    themes: [CheckMenuItem<Wry>; 3],
    quit: MenuItem<Wry>,
}

pub fn install(app: &AppHandle, tray: TrayIconBuilder<Wry>) -> tauri::Result<()> {
    // StatusNotifier hosts render this menu and send item events to Tauri.
    let action = |id| MenuItem::with_id(app, id, "", true, None::<&str>);
    let check = |id| CheckMenuItem::with_id(app, id, "", false, false, None::<&str>);
    let [system, light, dark] = THEMES.map(|(id, _)| check(id));
    let themes = [system?, light?, dark?];
    let theme = Submenu::with_items(app, "", false, &[&themes[0], &themes[1], &themes[2]])?;
    let items = TrayMenu {
        show: action("show")?,
        settings: action("tray-settings")?,
        capture: check("tray-capture")?,
        theme,
        themes,
        quit: action("quit")?,
    };
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &items.show,
            &items.capture,
            &items.theme,
            &items.settings,
            &separator,
            &items.quit,
        ],
    )?;
    app.manage(items);
    update_language(app)?;
    tray.menu(&menu).on_menu_event(on_menu_event).build(app)?;
    Ok(())
}

pub fn update_language(app: &AppHandle) -> tauri::Result<()> {
    let Some(items) = app.try_state::<TrayMenu>() else {
        return Ok(());
    };
    let chinese = app
        .state::<desktop::DesktopState>()
        .ui_language
        .load(Ordering::SeqCst)
        == 2;
    let (show, settings, capture, theme, themes, quit) = if chinese {
        (
            "显示 UClip",
            "设置",
            "剪贴板监听",
            "主题",
            ["跟随系统", "浅色", "深色"],
            "退出 UClip",
        )
    } else {
        (
            "Show UClip",
            "Settings",
            "Clipboard monitoring",
            "Theme",
            ["Follow system", "Light", "Dark"],
            "Quit UClip",
        )
    };
    items.show.set_text(show)?;
    items.settings.set_text(settings)?;
    items.capture.set_text(capture)?;
    items.theme.set_text(theme)?;
    for (item, text) in items.themes.iter().zip(themes) {
        item.set_text(text)?;
    }
    items.quit.set_text(quit)
}

pub async fn refresh(app: &AppHandle) {
    static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _refresh = REFRESH.lock().await;
    let Some(items) = app.try_state::<TrayMenu>() else {
        return;
    };
    let (appearance, clipboard) = tokio::join!(
        commands::get_settings(app.state::<AppState>()),
        commands::clipboard_status(app.clone(), app.state::<AppState>()),
    );
    let theme = appearance.map(|settings| settings.theme).ok();
    let _ = items.theme.set_enabled(theme.is_some());
    for (item, (_, value)) in items.themes.iter().zip(THEMES) {
        let _ = item.set_enabled(theme.is_some());
        let _ = item.set_checked(theme == Some(value));
    }
    if let Ok(status) = clipboard {
        let _ = items
            .capture
            .set_enabled(status.available || status.policy.enabled);
        let _ = items.capture.set_checked(status.policy.enabled);
    } else {
        let _ = items.capture.set_enabled(false);
    }
}

fn on_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "show" => desktop::show_from_tray(app),
        "quit" => crate::shutdown(app.clone()),
        "tray-settings" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::open_settings_window(app).await;
            });
        }
        "tray-capture" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if toggle_capture(&app).await.is_err() {
                    refresh(&app).await;
                }
            });
        }
        id => {
            if let Some((_, theme)) = THEMES.into_iter().find(|(known, _)| *known == id) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if set_theme(&app, theme).await.is_err() {
                        refresh(&app).await;
                    }
                });
            }
        }
    }
}

async fn toggle_capture(app: &AppHandle) -> Result<(), AppError> {
    let policy = commands::clipboard_status(app.clone(), app.state::<AppState>())
        .await?
        .policy;
    commands::update_capture_policy(
        app.clone(),
        app.state(),
        UpdateCapturePolicy {
            enabled: !policy.enabled,
            exclude_pattern: policy.exclude_pattern,
            max_items: policy.max_items,
            expected_revision: policy.revision,
        },
    )
    .await?;
    Ok(())
}

async fn set_theme(app: &AppHandle, theme: Theme) -> Result<(), AppError> {
    let settings = commands::get_settings(app.state::<AppState>()).await?;
    if settings.theme == theme {
        // GTK toggles the check mark even when the selected theme was clicked again.
        refresh(app).await;
        return Ok(());
    }
    commands::update_settings(
        app.clone(),
        app.state(),
        UpdateSettings {
            theme,
            language: None,
            show_search_by_default: None,
            expected_revision: settings.revision,
        },
    )
    .await?;
    Ok(())
}
