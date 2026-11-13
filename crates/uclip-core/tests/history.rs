use rusqlite::Connection;
use uclip_core::*;
fn query(text: &str) -> HistoryQuery {
    HistoryQuery {
        query: text.into(),
        regex: false,
        kind: None,
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
    assert!(!s.capture_policy().unwrap().enabled);
    drop(s);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        2
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
