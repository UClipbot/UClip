mod common;

use rusqlite::Connection;
use uclip_core::{ErrorCode, Storage, UpdateDesktopPreferences, WindowPlacement};

#[test]
fn desktop_preferences_and_geometry_survive_restart_with_conflict_and_failure_protection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let p = s.desktop_preferences().unwrap();
    assert!(p.close_to_tray && !p.auto_hide);
    assert!(!p.native_titlebar);
    let mut update = UpdateDesktopPreferences {
        close_to_tray: false,
        auto_hide: false,
        native_titlebar: true,
        shortcut: "Control+Alt+K".into(),
        expected_revision: p.revision,
    };
    let saved = s.update_desktop_preferences(update.clone()).unwrap();
    assert!(saved.native_titlebar);
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
    update.native_titlebar = false;
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
            auto_hide: false,
            native_titlebar: false,
            shortcut: "Control+Alt+K".into(),
            expected_revision: old.revision,
        })
        .unwrap();
    let revision = s.history_revision().unwrap();
    drop(s);
    let c = Connection::open(&path).unwrap();
    common::remove_replica_schema(&c);
    c.execute_batch(
        "ALTER TABLE desktop_preferences DROP COLUMN native_titlebar; ALTER TABLE desktop_preferences DROP COLUMN autostart_initialized; ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=5;",
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
    common::remove_replica_schema(&c);
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

#[test]
fn fresh_autostart_default_is_consumed_once_and_storage_failure_does_not_consume_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let preferences = s.desktop_preferences().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_default BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        s.take_autostart_default().unwrap_err().code,
        ErrorCode::Storage
    );
    c.execute_batch("DROP TRIGGER fail_default;").unwrap();
    assert!(s.take_autostart_default().unwrap());
    assert!(!s.take_autostart_default().unwrap());
    assert_eq!(s.desktop_preferences().unwrap(), preferences);
    drop(s);
    let mut s = Storage::open(&path).unwrap();
    assert!(!s.take_autostart_default().unwrap());
    assert_eq!(s.desktop_preferences().unwrap(), preferences);
}

#[test]
fn v6_upgrade_preserves_disabled_preferences_and_never_requests_autostart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v6.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let id = s.create_text("v6 fixture".into()).unwrap();
    drop(s);
    let c = Connection::open(&path).unwrap();
    common::remove_replica_schema(&c);
    c.execute_batch("UPDATE desktop_preferences SET close_to_tray=0,always_on_top=0,native_titlebar=1; ALTER TABLE desktop_preferences DROP COLUMN autostart_initialized; ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=6;").unwrap();
    let mut s = Storage::open(&path).unwrap();
    let p = s.desktop_preferences().unwrap();
    assert!(!p.close_to_tray && !p.auto_hide && p.native_titlebar);
    assert_eq!(p.revision, "0");
    assert!(!s.take_autostart_default().unwrap());
    assert_eq!(
        s.read_clip(&id).unwrap(),
        uclip_core::ClipPayload::Text("v6 fixture".into())
    );
    drop(s);
    assert_eq!(
        Storage::open(&path).unwrap().desktop_preferences().unwrap(),
        p
    );
    c.execute_batch("PRAGMA user_version=6;").unwrap();
    assert!(Storage::open(&path).is_err());
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[test]
fn v9_migration_preserves_blur_behavior_for_every_legacy_pin_combination() {
    for legacy_topmost in [false, true] {
        for legacy_auto_hide in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("v9.sqlite3");
            let mut storage = Storage::open(&path).unwrap();
            let id = storage.create_text("pin migration fixture".into()).unwrap();
            drop(storage);
            let connection = Connection::open(&path).unwrap();
            connection
                .execute(
                    "UPDATE desktop_preferences SET always_on_top=?1,auto_hide=?2,revision=17",
                    (legacy_topmost, legacy_auto_hide),
                )
                .unwrap();
            common::remove_replica_schema(&connection);
            connection.execute_batch("ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; PRAGMA user_version=9;").unwrap();
            let storage = Storage::open(&path).unwrap();
            let preferences = storage.desktop_preferences().unwrap();
            assert_eq!(preferences.auto_hide, legacy_auto_hide && !legacy_topmost);
            assert_eq!(preferences.revision, "17");
            assert_eq!(
                storage.read_clip(&id).unwrap(),
                uclip_core::ClipPayload::Text("pin migration fixture".into())
            );
            drop(storage);
            assert_eq!(
                Storage::open(&path).unwrap().desktop_preferences().unwrap(),
                preferences
            );
        }
    }
}

#[test]
fn failed_pin_migration_preserves_schema_and_legacy_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failed-v9.sqlite3");
    drop(Storage::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    common::remove_replica_schema(&connection);
    connection.execute_batch("UPDATE desktop_preferences SET always_on_top=1,auto_hide=1; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; PRAGMA user_version=9; CREATE TRIGGER fail_pin_migration BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(Storage::open(&path).err().unwrap().code, ErrorCode::Storage);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        9
    );
    assert!(connection
        .query_row("SELECT auto_hide FROM desktop_preferences", [], |row| row
            .get::<_, bool>(
            0
        ))
        .unwrap());
    connection
        .execute_batch("DROP TRIGGER fail_pin_migration")
        .unwrap();
    assert!(
        !Storage::open(&path)
            .unwrap()
            .desktop_preferences()
            .unwrap()
            .auto_hide
    );
}
