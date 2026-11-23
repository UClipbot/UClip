mod common;

use rusqlite::Connection;
use tempfile::tempdir;
use uclip_core::{CoreService, ErrorCode, Storage, Theme, UpdateSettings};

fn update(theme: Theme, revision: &str) -> UpdateSettings {
    UpdateSettings {
        theme,
        language: None,
        show_search_by_default: None,
        expected_revision: revision.into(),
    }
}

#[test]
fn committed_settings_survive_restart_and_stale_writes_are_rejected() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("test.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    assert_eq!(storage.settings().unwrap().revision, "0");
    assert_eq!(
        storage
            .update_settings(update(Theme::Dark, "0"))
            .unwrap()
            .revision,
        "1"
    );
    assert_eq!(
        storage
            .update_settings(update(Theme::Light, "0"))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    drop(storage);
    let reopened = Storage::open(&path).unwrap();
    assert_eq!(reopened.settings().unwrap().theme, Theme::Dark);
    assert_eq!(reopened.settings().unwrap().revision, "1");
}

#[test]
fn foreign_database_is_not_migrated_or_tagged() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("old.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE old_clips(content TEXT); INSERT INTO old_clips VALUES('fixture');",
        )
        .unwrap();
    assert_eq!(
        Storage::open(&path).err().unwrap().code,
        ErrorCode::ForeignDatabase
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT content FROM old_clips", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "fixture"
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn newer_schema_is_rejected_without_downgrade() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("future.sqlite3");
    drop(Storage::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    assert_eq!(
        Storage::open(&path).err().unwrap().code,
        ErrorCode::UnsupportedSchema
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        999
    );
}

#[test]
fn failed_transaction_does_not_advance_revision_or_change_setting() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("failure.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_write BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
    assert_eq!(
        storage
            .update_settings(update(Theme::Dark, "0"))
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    assert_eq!(storage.settings().unwrap().theme, Theme::System);
    assert_eq!(storage.settings().unwrap().revision, "0");
    connection
        .execute_batch("DROP TRIGGER reject_write")
        .unwrap();
    assert_eq!(
        storage
            .update_settings(update(Theme::Dark, "0"))
            .unwrap()
            .revision,
        "1"
    );
}

#[test]
fn invalid_revision_cannot_bypass_compare_and_swap() {
    let directory = tempdir().unwrap();
    let mut storage = Storage::open(&directory.path().join("test.sqlite3")).unwrap();
    for value in [
        "-1",
        "00",
        "+0",
        "nope",
        "9223372036854775807",
        "18446744073709551616",
    ] {
        assert_eq!(
            storage
                .update_settings(update(Theme::Dark, value))
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }
    assert_eq!(storage.settings().unwrap().revision, "0");
}

#[test]
fn startup_failure_is_safe_and_does_not_fall_back_to_empty_database() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("broken.sqlite3");
    let bytes = b"not a database; private fixture /tmp/sensitive-path";
    std::fs::write(&path, bytes).unwrap();
    let error = Storage::open(&path).err().unwrap();
    assert_eq!(error.code, ErrorCode::Storage);
    let serialized = serde_json::to_string(&error).unwrap();
    assert!(!serialized.contains("sensitive-path"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn migration_is_atomic_on_table_creation_failure() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("migration.sqlite3");
    drop(Storage::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 0).unwrap();
    // Simulates an unexpected schema that would fail migration at CREATE TABLE.
    assert_eq!(Storage::open(&path).err().unwrap().code, ErrorCode::Storage);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT theme FROM settings", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "system"
    );
}

#[tokio::test]
async fn concurrent_writers_cannot_silently_overwrite_and_shutdown_closes_admission() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("service.sqlite3");
    let service = CoreService::open(path.clone()).await.unwrap();
    let other = service.clone();
    let (first, second) = tokio::join!(
        service.update_settings(update(Theme::Dark, "0")),
        other.update_settings(update(Theme::Light, "0")),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        first.err().or(second.err()).unwrap().code,
        ErrorCode::Conflict
    );
    let saved = service.settings().await.unwrap();
    service.shutdown().await.unwrap();
    assert_eq!(
        service.settings().await.unwrap_err().code,
        ErrorCode::Unavailable
    );
    let reopened = CoreService::open(path).await.unwrap();
    assert_eq!(reopened.settings().await.unwrap(), saved);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_directory_is_reported_without_creating_an_alternate_store() {
    let directory = tempdir().unwrap();
    let result = CoreService::open(directory.path().join("missing/test.sqlite3")).await;
    assert_eq!(result.err().unwrap().code, ErrorCode::Storage);
}

#[test]
fn unknown_settings_fields_and_invalid_themes_are_rejected() {
    assert!(serde_json::from_str::<UpdateSettings>(
        r#"{"theme":"dark","expectedRevision":"0","token":"secret"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<UpdateSettings>(
        r#"{"theme":"invalid","expectedRevision":"0"}"#
    )
    .is_err());
}

#[test]
fn language_is_durable_and_theme_only_updates_preserve_it() {
    use uclip_core::Language;
    let directory = tempdir().unwrap();
    let path = directory.path().join("language.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    assert_eq!(storage.settings().unwrap().language, Language::System);
    let mut request = update(Theme::Dark, "0");
    request.language = Some(Language::En);
    let saved = storage.update_settings(request).unwrap();
    assert_eq!(saved.language, Language::En);
    let mut stale = update(Theme::Light, "0");
    stale.language = Some(Language::ZhCn);
    assert_eq!(
        storage.update_settings(stale).unwrap_err().code,
        ErrorCode::Conflict
    );
    storage.update_settings(update(Theme::Light, "1")).unwrap();
    drop(storage);
    let mut reopened = Storage::open(&path).unwrap();
    assert_eq!(reopened.settings().unwrap().language, Language::En);
    assert_eq!(reopened.settings().unwrap().theme, Theme::Light);
    for (revision, language) in [("2", Language::ZhCn), ("3", Language::System)] {
        let mut request = update(Theme::Light, revision);
        request.language = Some(language);
        assert_eq!(
            reopened.update_settings(request).unwrap().language,
            language
        );
    }
}

#[test]
fn v8_migration_preserves_settings_and_defaults_to_system_language() {
    use uclip_core::{Language, SCHEMA_VERSION};
    let directory = tempdir().unwrap();
    let path = directory.path().join("v8.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    storage.update_settings(update(Theme::Dark, "0")).unwrap();
    drop(storage);
    let connection = Connection::open(&path).unwrap();
    common::remove_replica_schema(&connection);
    connection
        .execute_batch("ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=8;")
        .unwrap();
    drop(connection);
    let storage = Storage::open(&path).unwrap();
    assert_eq!(storage.settings().unwrap().language, Language::System);
    assert_eq!(storage.settings().unwrap().theme, Theme::Dark);
    assert_eq!(storage.settings().unwrap().revision, "1");
    drop(storage);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn language_validation_rejects_unknown_values_and_accepts_older_requests() {
    let old: UpdateSettings =
        serde_json::from_str(r#"{"theme":"dark","expectedRevision":"0"}"#).unwrap();
    assert!(old.language.is_none());
    for language in ["fr", "zh", "EN", "", "../secret"] {
        let input =
            serde_json::json!({"theme":"dark", "language":language, "expectedRevision":"0"});
        assert!(serde_json::from_value::<UpdateSettings>(input).is_err());
    }
}

#[test]
fn search_toolbar_defaults_to_visible_and_keeps_a_saved_opt_out() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("v12.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    assert!(storage.settings().unwrap().show_search_by_default);
    storage.update_settings(update(Theme::Dark, "0")).unwrap();
    drop(storage);

    let connection = Connection::open(&path).unwrap();
    common::remove_replica_schema(&connection);
    connection
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; PRAGMA user_version=12;",
        )
        .unwrap();
    drop(connection);
    let mut storage = Storage::open(&path).unwrap();
    let migrated = storage.settings().unwrap();
    assert!(migrated.show_search_by_default);
    assert_eq!(migrated.theme, Theme::Dark);
    assert_eq!(migrated.revision, "1");
    let saved = storage
        .update_settings(UpdateSettings {
            show_search_by_default: Some(false),
            ..update(Theme::Dark, &migrated.revision)
        })
        .unwrap();
    assert!(!saved.show_search_by_default);
    assert!(
        !storage
            .update_settings(update(Theme::Light, &saved.revision))
            .unwrap()
            .show_search_by_default
    );
    drop(storage);
    assert!(
        !Storage::open(&path)
            .unwrap()
            .settings()
            .unwrap()
            .show_search_by_default
    );
}
