use rusqlite::Connection;
use uclip_core::{ErrorCode, KeyBindings, Storage, UpdateKeyBindings};

#[test]
fn keyboard_map_is_durable_revision_guarded_and_rejects_conflicts_and_reserved_editing_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let old = s.keyboard_bindings().unwrap();
    assert_eq!(old.bindings, KeyBindings::default());
    let mut bindings = old.bindings.clone();
    bindings.copy.primary = true;
    let saved = s
        .update_key_bindings(UpdateKeyBindings {
            bindings: bindings.clone(),
            expected_revision: old.revision.clone(),
        })
        .unwrap();
    assert_eq!(
        s.update_key_bindings(UpdateKeyBindings {
            bindings: bindings.clone(),
            expected_revision: old.revision
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    let mut duplicate = bindings.clone();
    duplicate.search = duplicate.copy.clone();
    assert_eq!(
        s.update_key_bindings(UpdateKeyBindings {
            bindings: duplicate,
            expected_revision: saved.revision.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::ShortcutConflict
    );
    let mut reserved = bindings.clone();
    reserved.copy.code = "KeyC".into();
    assert_eq!(
        s.update_key_bindings(UpdateKeyBindings {
            bindings: reserved,
            expected_revision: saved.revision.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::ShortcutConflict
    );
    let mut unmodified = bindings.clone();
    unmodified.settings = unmodified.preview.clone();
    unmodified.preview.code = "F8".into();
    assert_eq!(
        s.update_key_bindings(UpdateKeyBindings {
            bindings: unmodified,
            expected_revision: saved.revision.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::ShortcutConflict
    );
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_keys BEFORE UPDATE ON keyboard_bindings BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        s.update_key_bindings(UpdateKeyBindings {
            bindings: KeyBindings::default(),
            expected_revision: saved.revision.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(s.keyboard_bindings().unwrap(), saved);
    drop(s);
    assert_eq!(
        Storage::open(&path).unwrap().keyboard_bindings().unwrap(),
        saved
    );
}

#[test]
fn v3_keyboard_migration_is_atomic_and_corrupt_maps_fail_without_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v3.sqlite3");
    drop(Storage::open(&path).unwrap());
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TABLE keyboard_bindings; DROP TABLE collections; DROP TABLE collection_changes; DROP INDEX clips_collection_order; ALTER TABLE desktop_preferences DROP COLUMN native_titlebar; ALTER TABLE desktop_preferences DROP COLUMN autostart_initialized; ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=3; CREATE TABLE keyboard_bindings(fixture TEXT);").unwrap();
    drop(c);
    assert!(Storage::open(&path).is_err());
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        3
    );
    c.execute_batch("DROP TABLE keyboard_bindings;").unwrap();
    drop(c);
    let s = Storage::open(&path).unwrap();
    assert_eq!(
        s.keyboard_bindings().unwrap().bindings,
        KeyBindings::default()
    );
    drop(s);
    let c = Connection::open(&path).unwrap();
    c.execute("UPDATE keyboard_bindings SET value='{}'", [])
        .unwrap();
    drop(c);
    assert!(matches!(
        Storage::open(&path),
        Err(uclip_core::AppError {
            code: ErrorCode::Storage,
            ..
        })
    ));
}

#[test]
fn v11_search_shortcut_migrates_only_the_untouched_default() {
    let dir = tempfile::tempdir().unwrap();
    for revision in [0, 3] {
        let path = dir.path().join(format!("keys-{revision}.sqlite3"));
        drop(Storage::open(&path).unwrap());
        let mut legacy = KeyBindings::default();
        legacy.search.code = "KeyK".into();
        if revision > 0 {
            legacy.copy = KeyBindings::default().search;
        }
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; PRAGMA user_version=11;",
        )
        .unwrap();
        c.execute(
            "UPDATE keyboard_bindings SET value=?1,revision=?2",
            rusqlite::params![serde_json::to_string(&legacy).unwrap(), revision],
        )
        .unwrap();
        drop(c);

        let expected = if revision == 0 {
            KeyBindings::default()
        } else {
            legacy
        };
        for _ in 0..2 {
            let snapshot = Storage::open(&path).unwrap().keyboard_bindings().unwrap();
            assert_eq!(snapshot.bindings, expected);
            assert_eq!(snapshot.revision, revision.max(1).to_string());
        }
    }
}
