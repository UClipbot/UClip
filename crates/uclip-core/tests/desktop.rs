use rusqlite::Connection;
use uclip_core::{ErrorCode, Storage, UpdateDesktopPreferences, WindowPlacement};

#[test]
fn desktop_preferences_and_geometry_survive_restart_with_conflict_and_failure_protection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let p = s.desktop_preferences().unwrap();
    assert!(!p.close_to_tray && !p.always_on_top && !p.auto_hide);
    assert!(p.native_titlebar);
    let mut update = UpdateDesktopPreferences {
        close_to_tray: true,
        always_on_top: true,
        auto_hide: false,
        native_titlebar: false,
        shortcut: "Control+Alt+K".into(),
        expected_revision: p.revision,
    };
    let saved = s.update_desktop_preferences(update.clone()).unwrap();
    assert!(!saved.native_titlebar);
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
    update.native_titlebar = true;
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

#[test]
fn v5_titlebar_migration_preserves_existing_preferences_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v5.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let id = s.create_text("migration fixture".into()).unwrap();
    let old = s.desktop_preferences().unwrap();
    let saved = s
        .update_desktop_preferences(UpdateDesktopPreferences {
            close_to_tray: true,
            always_on_top: true,
            auto_hide: false,
            native_titlebar: false,
            shortcut: "Control+Alt+K".into(),
            expected_revision: old.revision,
        })
        .unwrap();
    let revision = s.history_revision().unwrap();
    drop(s);
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "ALTER TABLE desktop_preferences DROP COLUMN native_titlebar; PRAGMA user_version=5;",
    )
    .unwrap();
    let s = Storage::open(&path).unwrap();
    assert_eq!(
        s.desktop_preferences().unwrap(),
        uclip_core::DesktopPreferences {
            native_titlebar: true,
            ..saved
        }
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    assert_eq!(
        s.read_clip(&id).unwrap(),
        uclip_core::ClipPayload::Text("migration fixture".into())
    );
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        uclip_core::SCHEMA_VERSION
    );
    drop(s);
    // A partially upgraded schema is rejected, preserving the stored preference and version.
    c.execute_batch("UPDATE desktop_preferences SET native_titlebar=0; PRAGMA user_version=5;")
        .unwrap();
    assert!(Storage::open(&path).is_err());
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        c.query_row("SELECT native_titlebar FROM desktop_preferences", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
