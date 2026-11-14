use rusqlite::Connection;
use uclip_core::*;
fn query(text: &str) -> HistoryQuery {
    HistoryQuery {
        query: text.into(),
        regex: false,
        kind: None,
        pinned_only: false,
        collection_id: None,
        offset: 0,
        expected_revision: None,
    }
}
fn enabled(storage: &mut Storage, max: u32) {
    let p = storage.capture_policy().unwrap();
    storage
        .update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: String::new(),
            max_items: max,
            expected_revision: p.revision,
        })
        .unwrap();
}
#[test]
fn migrates_settings_v1_atomically_and_rejects_partial_v2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA application_id=1129074264; PRAGMA user_version=1; CREATE TABLE settings(singleton INTEGER PRIMARY KEY, theme TEXT, revision INTEGER); INSERT INTO settings VALUES(1,'dark',7);").unwrap();
    drop(conn);
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.settings().unwrap().theme, Theme::Dark);
    assert_eq!(s.settings().unwrap().revision, "7");
    assert!(s.capture_policy().unwrap().enabled);
    drop(s);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        uclip_core::SCHEMA_VERSION
    );
    drop(conn);
    let bad = dir.path().join("bad.sqlite3");
    let c = Connection::open(&bad).unwrap();
    c.execute_batch("PRAGMA application_id=1129074264; PRAGMA user_version=1; CREATE TABLE settings(singleton INTEGER PRIMARY KEY, theme TEXT, revision INTEGER); INSERT INTO settings VALUES(1,'dark',7); CREATE TABLE capture_policy(x);").unwrap();
    drop(c);
    assert!(Storage::open(&bad).is_err());
    let c = Connection::open(&bad).unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='clips'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[test]
fn three_types_survive_restart_and_images_are_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 500);
    let values = vec![
        ClipPayload::Text("  中文\n\nemoji 🦀  ".into()),
        ClipPayload::Image {
            width: 2,
            height: 1,
            rgba: vec![12, 34, 56, 0, 255, 0, 99, 128],
        },
        ClipPayload::Files(vec![
            dir.path().join("目录 你好#%.txt").to_str().unwrap().into(),
            dir.path().to_str().unwrap().into(),
        ]),
    ];
    let ids = values
        .iter()
        .map(|v| s.capture(v.clone()).unwrap().unwrap())
        .collect::<Vec<_>>();
    drop(s);
    let s = Storage::open(&path).unwrap();
    for (id, value) in ids.iter().zip(values) {
        assert_eq!(s.read_clip(id).unwrap(), value);
    }
    assert_eq!(s.history(query("")).unwrap().total, 3);
}
#[test]
fn dedupe_is_scoped_and_does_not_merge_protected_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 500);
    let a = s.capture(ClipPayload::Text("A".into())).unwrap().unwrap();
    let b = s.capture(ClipPayload::Text("B".into())).unwrap().unwrap();
    assert_eq!(
        s.capture(ClipPayload::Text("A".into())).unwrap(),
        Some(a.clone())
    );
    assert_eq!(s.history(query("")).unwrap().items[0].id, a);
    let c = Connection::open(&path).unwrap();
    c.execute("UPDATE clips SET pinned=1 WHERE id=?1", [&a])
        .unwrap();
    let a2 = s.capture(ClipPayload::Text("A".into())).unwrap().unwrap();
    assert_ne!(a, a2);
    c.execute("UPDATE clips SET collection='custom' WHERE id=?1", [&b])
        .unwrap();
    let b2 = s.capture(ClipPayload::Text("B".into())).unwrap().unwrap();
    assert_ne!(b, b2);
    assert_eq!(s.history(query("")).unwrap().total, 4);
}
#[test]
fn capacity_never_deletes_the_just_saved_record_and_rolls_back_when_protected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 1);
    let a = s.capture(ClipPayload::Text("A".into())).unwrap().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute("UPDATE clips SET pinned=1 WHERE id=?1", [&a])
        .unwrap();
    let revision = s.history_revision().unwrap();
    assert_eq!(
        s.capture(ClipPayload::Text("B".into())).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    assert_eq!(s.read_clip(&a).unwrap(), ClipPayload::Text("A".into()));
    assert_eq!(s.history(query("")).unwrap().total, 1);
    c.execute("UPDATE clips SET pinned=0", []).unwrap();
    let b = s.capture(ClipPayload::Text("B".into())).unwrap().unwrap();
    assert_eq!(s.read_clip(&a).unwrap_err().code, ErrorCode::NotFound);
    assert!(s.read_clip(&b).is_ok());
}
#[test]
fn paused_filtered_invalid_and_oversize_data_never_land() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: false,
        exclude_pattern: String::new(),
        max_items: p.max_items,
        expected_revision: p.revision,
    })
    .unwrap();
    assert!(s
        .capture(ClipPayload::Text("paused".into()))
        .unwrap()
        .is_none());
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: true,
        exclude_pattern: "secret".into(),
        max_items: 500,
        expected_revision: p.revision,
    })
    .unwrap();
    assert!(s
        .capture(ClipPayload::Text("SECRET value".into()))
        .unwrap()
        .is_none());
    assert!(s
        .capture(ClipPayload::Text("a".repeat(MAX_TEXT_BYTES + 1)))
        .is_err());
    assert!(s
        .capture(ClipPayload::Image {
            width: 2,
            height: 2,
            rgba: vec![0; 3]
        })
        .is_err());
    assert!(s
        .capture(ClipPayload::Files(vec!["relative".into()]))
        .is_err());
    let p = s.capture_policy().unwrap();
    assert_eq!(
        s.update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: "[".into(),
            max_items: 500,
            expected_revision: p.revision.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidPattern
    );
    assert_eq!(s.capture_policy().unwrap().revision, p.revision);
    assert_eq!(s.history(query("")).unwrap().total, 0);
}

#[test]
fn capture_starts_enabled_and_preserves_an_explicit_pause_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capture.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let p = s.capture_policy().unwrap();
    assert!(p.enabled);
    assert!(s
        .capture(ClipPayload::Text("first copy".into()))
        .unwrap()
        .is_some());
    let paused = s
        .update_capture_policy(UpdateCapturePolicy {
            enabled: false,
            exclude_pattern: "private".into(),
            max_items: 25,
            expected_revision: p.revision,
        })
        .unwrap();
    drop(s);
    let mut s = Storage::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(s.capture_policy().unwrap()).unwrap(),
        serde_json::to_value(paused).unwrap()
    );
    assert!(s
        .capture(ClipPayload::Text("paused copy".into()))
        .unwrap()
        .is_none());
}

#[test]
fn capture_default_upgrade_only_enables_untouched_policies_once() {
    let dir = tempfile::tempdir().unwrap();
    for (enabled, revision) in [(false, 0), (false, 1), (true, 2)] {
        let path = dir
            .path()
            .join(format!("capture-{enabled}-{revision}.sqlite3"));
        drop(Storage::open(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA user_version=10;").unwrap();
        connection
            .execute(
                "UPDATE capture_policy SET enabled=?1,revision=?2",
                rusqlite::params![enabled, revision],
            )
            .unwrap();
        drop(connection);
        let storage = Storage::open(&path).unwrap();
        let policy = storage.capture_policy().unwrap();
        assert_eq!(policy.enabled, enabled || revision == 0);
        assert_eq!(
            policy.revision,
            (if revision == 0 { 1 } else { revision }).to_string()
        );
        drop(storage);
        assert_eq!(
            serde_json::to_value(Storage::open(&path).unwrap().capture_policy().unwrap()).unwrap(),
            serde_json::to_value(policy).unwrap()
        );
    }
}
#[test]
fn searches_all_candidates_before_paging_and_stale_pages_fail() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    enabled(&mut s, 500);
    for i in 0..40 {
        s.capture(ClipPayload::Text(format!("中文内容 {i}\nline")))
            .unwrap();
    }
    let page = s.history(query("")).unwrap();
    assert_eq!(page.total, 40);
    assert_eq!(page.items.len(), 25);
    let mut q = query("^中文内容 0$");
    q.regex = true; // anchors without multiline only match the full content
    assert_eq!(s.history(q.clone()).unwrap().total, 0);
    q.query = "(?m)^中文内容 0$".into();
    assert_eq!(s.history(q).unwrap().total, 1);
    let mut q = query("");
    q.offset = 25;
    q.expected_revision = Some(page.revision);
    assert_eq!(s.history(q.clone()).unwrap().items.len(), 15);
    s.capture(ClipPayload::Text("new".into())).unwrap();
    assert_eq!(s.history(q).unwrap_err().code, ErrorCode::Conflict);
    let mut q = query("(?=foo)");
    q.regex = true;
    assert_eq!(s.history(q).unwrap_err().code, ErrorCode::InvalidPattern);
}
#[test]
fn write_failure_never_advances_revision_or_acknowledges_capture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 500);
    let revision = s.history_revision().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON clips BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
    assert!(s.capture(ClipPayload::Text("not saved".into())).is_err());
    assert_eq!(s.history_revision().unwrap(), revision);
    c.execute_batch("DROP TRIGGER fail_insert").unwrap();
    assert!(s
        .capture(ClipPayload::Text("not saved".into()))
        .unwrap()
        .is_some());
    assert_eq!(s.history(query("")).unwrap().total, 1);
}
#[test]
fn copy_usage_is_separate_and_does_not_change_capture_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    enabled(&mut s, 500);
    let a = s.capture(ClipPayload::Text("A".into())).unwrap().unwrap();
    let b = s.capture(ClipPayload::Text("B".into())).unwrap().unwrap();
    s.mark_used(&a).unwrap();
    let page = s.history(query("")).unwrap();
    assert_eq!(page.items[0].id, b);
    assert!(page.items[1].last_used_at.is_some());
}
#[test]
fn paused_policy_update_is_revision_guarded_and_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    enabled(&mut s, 500);
    assert_eq!(
        s.update_capture_policy(UpdateCapturePolicy {
            enabled: false,
            exclude_pattern: String::new(),
            max_items: 500,
            expected_revision: "0".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: false,
        exclude_pattern: String::new(),
        max_items: 500,
        expected_revision: p.revision,
    })
    .unwrap();
    assert!(s
        .capture(ClipPayload::Text("after pause".into()))
        .unwrap()
        .is_none());
}

#[test]
fn exact_text_limit_preserves_every_byte_and_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    enabled(&mut s, 500);
    let mut text = "中文\n\n  ".to_string();
    text.push_str(&"x".repeat(MAX_TEXT_BYTES - text.len() - 2));
    text.push_str("  ");
    assert_eq!(text.len(), MAX_TEXT_BYTES);
    let payload = ClipPayload::Text(text);
    let id = s.capture(payload.clone()).unwrap().unwrap();
    assert_eq!(s.read_clip(&id).unwrap(), payload);
    let spaces = ClipPayload::Text(" \n\t ".into());
    let id = s.capture(spaces.clone()).unwrap().unwrap();
    assert_eq!(s.read_clip(&id).unwrap(), spaces);
}

#[test]
fn file_filters_and_search_use_real_paths_not_json_escapes_and_images_are_not_empty_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("s.sqlite3")).unwrap();
    let private = dir
        .path()
        .join("private\\note 你好#.txt")
        .to_str()
        .unwrap()
        .to_string();
    let public = dir.path().join("public.txt").to_str().unwrap().to_string();
    let anchored = format!("^{}$", regex::escape(&private));
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: true,
        exclude_pattern: anchored.clone(),
        max_items: 500,
        expected_revision: p.revision,
    })
    .unwrap();
    assert!(s
        .capture(ClipPayload::Files(vec![public, private.clone()]))
        .unwrap()
        .is_none());
    assert_eq!(s.history(query("")).unwrap().total, 0);
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: true,
        exclude_pattern: "^$".into(),
        max_items: 500,
        expected_revision: p.revision,
    })
    .unwrap();
    assert!(s
        .capture(ClipPayload::Image {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 128]
        })
        .unwrap()
        .is_some());
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: true,
        exclude_pattern: String::new(),
        max_items: 500,
        expected_revision: p.revision,
    })
    .unwrap();
    s.capture(ClipPayload::Files(vec![private.clone()]))
        .unwrap();
    assert_eq!(s.history(query(&private)).unwrap().total, 1);
    let mut q = query(&anchored);
    q.regex = true;
    assert_eq!(s.history(q).unwrap().total, 1);
}

fn deletion(
    s: &mut Storage,
    id: &str,
    revision: &str,
    deleted: bool,
) -> Result<ClipDeletionSnapshot, AppError> {
    s.change_clip_deletion(ChangeClipDeletion {
        id: id.into(),
        expected_revision: revision.into(),
        deleted,
    })
}

#[test]
fn deletion_and_undo_are_durable_for_all_three_payload_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delete.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 500);
    let values = vec![
        ClipPayload::Text("删除后不复活\n🦀".into()),
        ClipPayload::Image {
            width: 1,
            height: 1,
            rgba: vec![7, 8, 9, 128],
        },
        ClipPayload::Files(vec![dir.path().join("file.txt").to_str().unwrap().into()]),
    ];
    let mut deleted = vec![];
    for value in &values {
        let id = s.capture(value.clone()).unwrap().unwrap();
        let item = s
            .history(query(""))
            .unwrap()
            .items
            .into_iter()
            .find(|v| v.id == id)
            .unwrap();
        let result = deletion(&mut s, &id, &item.revision, true).unwrap();
        assert!(result.deleted);
        assert_eq!(s.read_clip(&id).unwrap_err().code, ErrorCode::NotFound);
        deleted.push(result);
    }
    drop(s);
    let mut s = Storage::open(&path).unwrap();
    assert_eq!(s.history(query("")).unwrap().total, 0);
    for (row, value) in deleted.iter().zip(values) {
        let restored = deletion(&mut s, &row.id, &row.revision, false).unwrap();
        assert!(!restored.deleted);
        assert_eq!(s.read_clip(&row.id).unwrap(), value);
    }
    drop(s);
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.history(query("")).unwrap().total, 3);
}
#[test]
fn deletion_conflicts_and_failed_transactions_keep_the_record_and_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delete.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    enabled(&mut s, 500);
    let id = s
        .capture(ClipPayload::Text("record".into()))
        .unwrap()
        .unwrap();
    s.mark_used(&id).unwrap();
    let revision = s.history_revision().unwrap();
    assert_eq!(
        deletion(&mut s, &id, "0", true).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    assert_eq!(
        deletion(&mut s, &id, "01", true).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_delete BEFORE UPDATE OF deleted ON clips BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        deletion(&mut s, &id, "1", true).unwrap_err().code,
        ErrorCode::Storage
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    assert!(s.read_clip(&id).is_ok());
    conn.execute_batch("DROP TRIGGER fail_delete").unwrap();
    assert!(deletion(&mut s, &id, "1", true).is_ok());
}
#[test]
fn recapture_does_not_resurrect_deleted_identity_and_full_undo_does_not_evict() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("delete.sqlite3")).unwrap();
    enabled(&mut s, 1);
    let a = s
        .capture(ClipPayload::Text("same content".into()))
        .unwrap()
        .unwrap();
    let removed = deletion(&mut s, &a, "0", true).unwrap();
    let b = s
        .capture(ClipPayload::Text("same content".into()))
        .unwrap()
        .unwrap();
    assert_ne!(a, b);
    let revision = s.history_revision().unwrap();
    assert_eq!(
        deletion(&mut s, &a, &removed.revision, false)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    assert!(s.read_clip(&b).is_ok());
    assert_eq!(s.read_clip(&a).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn changes_include_capture_usage_delete_restore_and_capacity_purge_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changes.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    enabled(&mut storage, 1);
    let conn = Connection::open(&path).unwrap();
    let change = |id: &str| -> (i64, String, i64) {
        conn.query_row(
            "SELECT revision,operation,ordinal FROM clip_changes WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let a = storage
        .capture(ClipPayload::Text("A".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        change(&a),
        (
            0,
            "upsert".into(),
            storage.history_revision().unwrap().parse().unwrap()
        )
    );
    storage.mark_used(&a).unwrap();
    assert_eq!(change(&a).0, 1);
    let removed = deletion(&mut storage, &a, "1", true).unwrap();
    assert_eq!(change(&a).1, "delete");
    deletion(&mut storage, &a, &removed.revision, false).unwrap();
    assert_eq!(change(&a).1, "upsert");
    let b = storage
        .capture(ClipPayload::Text("B".into()))
        .unwrap()
        .unwrap();
    assert_eq!(change(&a).1, "purge");
    assert_eq!(change(&a).2, change(&b).2);
    assert_eq!(
        change(&a).2,
        storage.history_revision().unwrap().parse::<i64>().unwrap()
    );
    assert_eq!(storage.read_clip(&a).unwrap_err().code, ErrorCode::NotFound);
    let before = storage.history_revision().unwrap();
    conn.execute_batch("CREATE TRIGGER outbox_failure BEFORE UPDATE ON clip_changes BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(storage.mark_used(&b).is_err());
    assert_eq!(storage.history_revision().unwrap(), before);
    assert_eq!(change(&b).0, 0);
    assert_eq!(
        storage.history(query("")).unwrap().items[0].last_used_at,
        None
    );
}

#[test]
fn v2_migration_seeds_current_identities_and_rolls_back_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    enabled(&mut storage, 500);
    let id = storage
        .capture(ClipPayload::Text("迁移保留".into()))
        .unwrap()
        .unwrap();
    drop(storage);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER clips_change_insert; DROP TRIGGER clips_change_update; DROP TRIGGER clips_change_delete; DROP TABLE clip_changes; DROP TABLE desktop_preferences; DROP TABLE window_placement; DROP TABLE keyboard_bindings; DROP TABLE collections; DROP TABLE collection_changes; DROP INDEX clips_collection_order; ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=2; CREATE TABLE desktop_preferences(fixture TEXT);").unwrap();
    drop(conn);
    assert!(Storage::open(&path).is_err());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='clip_changes'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TABLE desktop_preferences;")
        .unwrap();
    drop(conn);
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        storage.read_clip(&id).unwrap(),
        ClipPayload::Text("迁移保留".into())
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT operation FROM clip_changes WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "upsert"
    );
}

#[test]
fn highlight_ranges_use_utf16_and_regex_matches_beyond_the_preview_still_count() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("highlight.sqlite3")).unwrap();
    enabled(&mut s, 500);
    s.capture(ClipPayload::Text("🦀 中文\nSECOND line".into()))
        .unwrap();
    let page = s.history(query("中文")).unwrap();
    assert_eq!(page.items[0].match_ranges, vec![(3, 5)]);
    s.capture(ClipPayload::Text(format!("{}tail", "x".repeat(400))))
        .unwrap();
    let page = s.history(query("tail")).unwrap();
    assert_eq!(page.total, 1);
    assert!(page.items[0].match_ranges.is_empty());
}

#[test]
fn missing_change_tracking_fails_startup_instead_of_silently_accepting_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing-trigger.sqlite3");
    drop(Storage::open(&path).unwrap());
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER clips_change_delete").unwrap();
    assert!(matches!(
        Storage::open(&path),
        Err(AppError {
            code: ErrorCode::Storage,
            ..
        })
    ));
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        uclip_core::SCHEMA_VERSION
    );
}

#[test]
fn manual_creation_uses_shared_rules_but_preserves_distinct_user_identities_while_paused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manual.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: false,
        exclude_pattern: String::new(),
        max_items: p.max_items,
        expected_revision: p.revision,
    })
    .unwrap();
    let text = "  中文\n\n手工副本 🦀  ".to_string();
    let a = s.create_text(text.clone()).unwrap();
    let b = s.create_text(text.clone()).unwrap();
    assert_ne!(a, b);
    assert!(s.history(query("")).unwrap().items.iter().all(|i| i.manual));
    enabled(&mut s, 3);
    let c = s.capture(ClipPayload::Text(text.clone())).unwrap().unwrap();
    assert_ne!(c, a);
    assert_ne!(c, b);
    assert_eq!(
        s.capture(ClipPayload::Text(text.clone())).unwrap().unwrap(),
        c
    );
    s.capture(ClipPayload::Text("新的采集".into())).unwrap();
    assert_eq!(s.read_clip(&c).unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(s.read_clip(&a).unwrap(), ClipPayload::Text(text.clone()));
    let p = s.capture_policy().unwrap();
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: false,
        exclude_pattern: "private".into(),
        max_items: 3,
        expected_revision: p.revision,
    })
    .unwrap();
    let revision = s.history_revision().unwrap();
    assert_eq!(
        s.create_text("PRIVATE value".into()).unwrap_err().code,
        ErrorCode::Excluded
    );
    assert_eq!(
        s.create_text("x".repeat(MAX_TEXT_BYTES + 1))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(s.history_revision().unwrap(), revision);
    drop(s);
    assert_eq!(
        Storage::open(&path).unwrap().read_clip(&b).unwrap(),
        ClipPayload::Text(text)
    );
}
