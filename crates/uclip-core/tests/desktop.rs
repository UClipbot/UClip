use rusqlite::Connection;
use uclip_core::{ErrorCode, Storage, UpdateDesktopPreferences, WindowPlacement};

#[test]
fn desktop_preferences_and_geometry_survive_restart_with_conflict_and_failure_protection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let p = s.desktop_preferences().unwrap();
    assert!(!p.close_to_tray && !p.always_on_top && !p.auto_hide);
    let mut update = UpdateDesktopPreferences {
        close_to_tray: true,
        always_on_top: true,
        auto_hide: false,
        shortcut: "Control+Alt+K".into(),
        expected_revision: p.revision,
    };
    let saved = s.update_desktop_preferences(update.clone()).unwrap();
    assert_eq!(
        s.update_desktop_preferences(update.clone())
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    update.expected_revision = "01".into();
    assert_eq!(
        s.update_desktop_preferences(update.clone())
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    update.expected_revision = saved.revision.clone();
    update.auto_hide = true;
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_desktop BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        s.update_desktop_preferences(update).unwrap_err().code,
        ErrorCode::Storage
    );
    assert_eq!(s.desktop_preferences().unwrap(), saved);
    let placement = WindowPlacement {
        x: -1920,
        y: 10,
        width: 960,
        height: 700,
        maximized: false,
    };
    s.save_window_placement(placement).unwrap();
    assert_eq!(
        s.save_window_placement(WindowPlacement {
            width: 0,
            ..placement
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );
    drop(s);
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.desktop_preferences().unwrap(), saved);
    let restored = s.window_placement().unwrap().unwrap();
    assert_eq!(
        (
            restored.x,
            restored.y,
            restored.width,
            restored.height,
            restored.maximized
        ),
        (-1920, 10, 960, 700, false)
    );
}
