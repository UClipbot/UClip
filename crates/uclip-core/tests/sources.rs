use rusqlite::Connection;
use uclip_core::*;

fn query() -> HistoryQuery {
    HistoryQuery {
        query: String::new(),
        regex: false,
        kind: None,
        pinned_only: false,
        collection_id: None,
        offset: 0,
        expected_revision: None,
    }
}
fn enable(storage: &mut Storage) {
    let policy = storage.capture_policy().unwrap();
    storage
        .update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: String::new(),
            max_items: 500,
            expected_revision: policy.revision,
        })
        .unwrap();
}

#[test]
fn source_is_durable_and_recapture_replaces_including_unknown_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    enable(&mut storage);
    let payload = ClipPayload::Text("https://example.com/source-test".into());
    let id = storage
        .capture_with_source(payload.clone(), Some("Google-chrome".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        storage.history(query()).unwrap().items[0]
            .source_app
            .as_deref(),
        Some("Google-chrome")
    );
    drop(storage);
    let mut storage = Storage::open(&path).unwrap();
    assert_eq!(
        storage.history(query()).unwrap().items[0]
            .source_app
            .as_deref(),
        Some("Google-chrome")
    );
    let revision = storage.history_revision().unwrap();
    assert_eq!(
        storage
            .capture_with_source(payload.clone(), Some("Firefox".into()))
            .unwrap(),
        Some(id.clone())
    );
    assert_ne!(storage.history_revision().unwrap(), revision);
    assert_eq!(
        storage.history(query()).unwrap().items[0]
            .source_app
            .as_deref(),
        Some("Firefox")
    );
    assert_eq!(
        storage.capture_with_source(payload, None).unwrap(),
        Some(id)
    );
    assert!(storage.history(query()).unwrap().items[0]
        .source_app
        .is_none());
    let manual = storage.create_text("manual source".into()).unwrap();
    let page = storage.history(query()).unwrap();
    let item = page.items.iter().find(|item| item.id == manual).unwrap();
    assert!(item.manual && item.source_app.is_none());
}

#[test]
fn invalid_metadata_and_failed_transactions_do_not_advance_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failure.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    enable(&mut storage);
    let revision = storage.history_revision().unwrap();
    for invalid in ["".to_owned(), "\nsecret".to_owned(), "x".repeat(257)] {
        assert_eq!(
            storage
                .capture_with_source(ClipPayload::Text("not saved".into()), Some(invalid))
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fixture_source_failure BEFORE INSERT ON clips BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(storage
        .capture_with_source(
            ClipPayload::Text("not saved".into()),
            Some("Google-chrome".into())
        )
        .is_err());
    assert_eq!(storage.history_revision().unwrap(), revision);
    assert_eq!(storage.history(query()).unwrap().total, 0);
}

#[test]
fn v7_migration_preserves_records_and_partial_migration_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("migration.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    let id = storage.create_text("old record".into()).unwrap();
    let preferences = storage.desktop_preferences().unwrap();
    let revision = storage.history_revision().unwrap();
    drop(storage);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=7;")
        .unwrap();
    let storage = Storage::open(&path).unwrap();
    assert_eq!(storage.desktop_preferences().unwrap(), preferences);
    assert_eq!(storage.history_revision().unwrap(), revision);
    let item = &storage.history(query()).unwrap().items[0];
    assert_eq!(item.id, id);
    assert!(item.source_app.is_none());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    drop(storage);
    connection.pragma_update(None, "user_version", 7).unwrap();
    assert!(Storage::open(&path).is_err());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row("SELECT text FROM clips WHERE id=?1", [&id], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "old record"
    );
}
