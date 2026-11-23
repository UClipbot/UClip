mod common;

use rusqlite::{params, Connection};
use uclip_core::*;

fn query(collection_id: Option<&str>) -> HistoryQuery {
    HistoryQuery {
        query: String::new(),
        regex: false,
        kind: None,
        pinned_only: false,
        collection_id: collection_id.map(str::to_owned),
        offset: 0,
        expected_revision: None,
    }
}
fn category(s: &mut Storage, name: &str) -> CollectionItem {
    s.create_collection(CreateCollection { name: name.into() })
        .unwrap()
}
fn transfer(
    s: &mut Storage,
    id: &str,
    revision: &str,
    collection: Option<&str>,
    mode: TransferMode,
) -> Result<ClipTransferSnapshot, AppError> {
    s.transfer_clip(TransferClip {
        id: id.into(),
        expected_revision: revision.into(),
        collection_id: collection.map(str::to_owned),
        mode,
    })
}
fn policy(s: &mut Storage, max: u32, exclude: &str) {
    s.update_capture_policy(UpdateCapturePolicy {
        enabled: true,
        exclude_pattern: exclude.into(),
        max_items: max,
        expected_revision: s.capture_policy().unwrap().revision,
    })
    .unwrap();
}

#[test]
fn names_are_bounded_unique_and_renaming_preserves_identity_and_membership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collections.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    for name in ["", "  ", "line\nname", &"x".repeat(65)] {
        assert_eq!(
            s.create_collection(CreateCollection { name: name.into() })
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }
    let a = category(&mut s, "  Work 🦀  ");
    assert_eq!(a.name, "Work 🦀");
    assert_eq!(
        s.create_collection(CreateCollection {
            name: "work 🦀".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::DuplicateName
    );
    let b = category(&mut s, "other");
    let id = s
        .create_text_in_collection("retained".into(), Some(a.id.clone()))
        .unwrap();
    let renamed = s
        .rename_collection(RenameCollection {
            id: a.id.clone(),
            name: "工作".into(),
            expected_revision: a.revision.clone(),
        })
        .unwrap();
    assert_eq!(renamed.id, a.id);
    assert_eq!(s.history(query(Some(&a.id))).unwrap().items[0].id, id);
    assert_eq!(
        s.rename_collection(RenameCollection {
            id: a.id.clone(),
            name: "stale".into(),
            expected_revision: a.revision
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        s.rename_collection(RenameCollection {
            id: a.id.clone(),
            name: b.name,
            expected_revision: renamed.revision
        })
        .unwrap_err()
        .code,
        ErrorCode::DuplicateName
    );
    drop(s);
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.collections().unwrap().items[0].name, "工作");
    assert_eq!(
        s.history(query(Some(&a.id))).unwrap().items[0].collection_id,
        Some(a.id.clone())
    );
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT revision,operation FROM collection_changes WHERE id=?1",
            [a.id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        )
        .unwrap(),
        (1, "upsert".into())
    );
}

#[test]
fn moving_keeps_identity_pin_and_order_and_copying_creates_unpinned_independent_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("collections.sqlite3")).unwrap();
    let a = category(&mut s, "A");
    let b = category(&mut s, "B");
    let source = s.create_text("正文 🦀\n  ".into()).unwrap();
    let original = s.history(query(None)).unwrap().items[0].clone();
    s.change_clip_pin(ChangeClipPin {
        id: source.clone(),
        expected_revision: "0".into(),
        pinned: true,
    })
    .unwrap();
    let moved = transfer(&mut s, &source, "1", Some(&a.id), TransferMode::Move).unwrap();
    assert_eq!(moved.id, source);
    let first = s.history(query(Some(&a.id))).unwrap().items[0].clone();
    assert!(first.pinned);
    assert_eq!(first.captured_at, original.captured_at);
    let copied = transfer(
        &mut s,
        &source,
        &moved.revision,
        Some(&b.id),
        TransferMode::Copy,
    )
    .unwrap();
    assert_ne!(copied.id, source);
    let second = s.history(query(Some(&b.id))).unwrap().items[0].clone();
    assert!(second.manual && !second.pinned);
    assert_eq!(
        s.read_clip(&source).unwrap(),
        s.read_clip(&copied.id).unwrap()
    );
    assert_eq!(
        s.history(query(Some(&a.id))).unwrap().items[0].revision,
        moved.revision
    );
    let detached = transfer(&mut s, &source, &moved.revision, None, TransferMode::Move).unwrap();
    assert_eq!(detached.id, source);
    assert_eq!(s.history(query(Some(&a.id))).unwrap().total, 0);
    assert_eq!(s.history(query(None)).unwrap().total, 2);
    assert_eq!(
        transfer(&mut s, &source, "1", Some(&b.id), TransferMode::Move)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn classification_protects_capture_and_filters_search_before_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("collections.sqlite3")).unwrap();
    let a = category(&mut s, "A");
    let b = category(&mut s, "B");
    policy(&mut s, 500, "");
    let source = s
        .capture(ClipPayload::Text("same".into()))
        .unwrap()
        .unwrap();
    transfer(&mut s, &source, "0", Some(&a.id), TransferMode::Move).unwrap();
    let other = s
        .capture(ClipPayload::Text("same".into()))
        .unwrap()
        .unwrap();
    assert_ne!(source, other);
    for i in 0..30 {
        s.create_text_in_collection(format!("match {i:02}"), Some(a.id.clone()))
            .unwrap();
    }
    s.create_text_in_collection("match outside".into(), Some(b.id.clone()))
        .unwrap();
    let mut q = query(Some(&a.id));
    q.query = "^match".into();
    q.regex = true;
    let first = s.history(q.clone()).unwrap();
    assert_eq!(first.total, 30);
    assert_eq!(first.items.len(), 25);
    q.offset = 25;
    q.expected_revision = Some(first.revision);
    let second = s.history(q.clone()).unwrap();
    assert_eq!(second.items.len(), 5);
    assert!(second
        .items
        .iter()
        .all(|i| first.items.iter().all(|j| i.id != j.id)));
    s.rename_collection(RenameCollection {
        id: a.id.clone(),
        name: "A renamed".into(),
        expected_revision: "0".into(),
    })
    .unwrap();
    assert_eq!(s.history(q).unwrap_err().code, ErrorCode::Conflict);
    let mut pin_query = query(Some(&a.id));
    pin_query.pinned_only = true;
    assert_eq!(s.history(pin_query).unwrap().total, 0);
    let bad = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        s.history(query(Some(&bad))).unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[test]
fn copy_uses_shared_exclusion_capacity_and_change_tracking_without_evicting_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collections.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let a = category(&mut s, "A");
    policy(&mut s, 1, "");
    let source = s
        .capture(ClipPayload::Text("private content".into()))
        .unwrap()
        .unwrap();
    let before = s.history_revision().unwrap();
    assert_eq!(
        transfer(&mut s, &source, "0", Some(&a.id), TransferMode::Copy)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(s.history_revision().unwrap(), before);
    assert!(s.read_clip(&source).is_ok());
    policy(&mut s, 3, "private");
    assert_eq!(
        transfer(&mut s, &source, "0", Some(&a.id), TransferMode::Copy)
            .unwrap_err()
            .code,
        ErrorCode::Excluded
    );
    policy(&mut s, 3, "");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_clip_change BEFORE INSERT ON clip_changes BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    let before = s.history_revision().unwrap();
    assert_eq!(
        transfer(&mut s, &source, "0", Some(&a.id), TransferMode::Copy)
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    assert_eq!(s.history_revision().unwrap(), before);
    assert_eq!(s.history(query(None)).unwrap().total, 1);
    c.execute_batch("DROP TRIGGER fail_clip_change").unwrap();
    assert!(transfer(&mut s, &source, "0", Some(&a.id), TransferMode::Copy).is_ok());
    assert!(s.read_clip(&source).is_ok());
}

#[test]
fn three_payload_copies_preserve_content_and_exclusion_checks_actual_file_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("collections.sqlite3")).unwrap();
    let a = category(&mut s, "A");
    policy(&mut s, 500, "");
    let file = dir
        .path()
        .join("private-file")
        .to_string_lossy()
        .into_owned();
    for value in [
        ClipPayload::Text("exact text".into()),
        ClipPayload::Image {
            width: 2,
            height: 1,
            rgba: vec![77; 8],
        },
        ClipPayload::Files(vec![file.clone()]),
    ] {
        let id = s.capture(value.clone()).unwrap().unwrap();
        let copied = transfer(&mut s, &id, "0", Some(&a.id), TransferMode::Copy).unwrap();
        assert_eq!(s.read_clip(&copied.id).unwrap(), value);
        if matches!(value, ClipPayload::Files(_)) {
            policy(&mut s, 500, "private-file");
            assert_eq!(
                transfer(&mut s, &id, "0", Some(&a.id), TransferMode::Copy)
                    .unwrap_err()
                    .code,
                ErrorCode::Excluded
            );
        }
    }
}

#[test]
fn categories_are_bounded_and_failed_or_missing_tracking_never_acknowledges_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collections.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let a = category(&mut s, "A");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_category_change BEFORE INSERT ON collection_changes BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    let before = s.history_revision().unwrap();
    assert_eq!(
        s.create_collection(CreateCollection {
            name: "fail".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(
        s.rename_collection(RenameCollection {
            id: a.id.clone(),
            name: "fail".into(),
            expected_revision: "0".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(s.history_revision().unwrap(), before);
    assert_eq!(s.collections().unwrap().items[0].name, "A");
    c.execute_batch("DROP TRIGGER fail_category_change")
        .unwrap();
    for i in 1..64 {
        category(&mut s, &format!("category {i}"));
    }
    assert_eq!(
        s.create_collection(CreateCollection {
            name: "too many".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::LimitExceeded
    );
    c.execute_batch("DROP TRIGGER collections_change_update")
        .unwrap();
    drop(s);
    assert!(Storage::open(&path).is_err());
}

#[test]
fn v4_migration_preserves_history_and_rolls_back_partial_schema_without_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collections.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let id = s.create_text("v4 text".into()).unwrap();
    s.change_clip_pin(ChangeClipPin {
        id: id.clone(),
        expected_revision: "0".into(),
        pinned: true,
    })
    .unwrap();
    let before = s.history_revision().unwrap();
    drop(s);
    let c = Connection::open(&path).unwrap();
    common::remove_replica_schema(&c);
    c.execute_batch("DROP TABLE collections; DROP TABLE collection_changes; DROP INDEX clips_collection_order; ALTER TABLE desktop_preferences DROP COLUMN native_titlebar; ALTER TABLE desktop_preferences DROP COLUMN autostart_initialized; ALTER TABLE clips DROP COLUMN source_app; ALTER TABLE settings DROP COLUMN show_search_by_default; DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; ALTER TABLE settings DROP COLUMN language; PRAGMA user_version=4; CREATE TABLE collection_changes(fixture TEXT)").unwrap();
    assert!(Storage::open(&path).is_err());
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='collections'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    c.execute_batch("DROP TABLE collection_changes").unwrap();
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.history_revision().unwrap(), before);
    assert!(s.history(query(None)).unwrap().items[0].pinned);
    assert_eq!(
        s.read_clip(&id).unwrap(),
        ClipPayload::Text("v4 text".into())
    );
    assert!(s.collections().unwrap().items.is_empty());
    assert_eq!(
        c.query_row("SELECT revision FROM clips WHERE id=?1", params![id], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn deleting_a_collection_rolls_back_as_one_unit_and_preserves_trash_identity() {
    let folder = tempfile::tempdir().unwrap();
    let db = folder.path().join("collections.sqlite3");
    let mut storage = Storage::open(&db).unwrap();
    let group = category(&mut storage, "Work");
    let clip = storage
        .create_text_in_collection("keep the payload".into(), Some(group.id.clone()))
        .unwrap();
    let delete = DeleteCollection {
        id: group.id.clone(),
        expected_revision: group.revision,
    };
    let other_connection = Connection::open(&db).unwrap();
    other_connection
        .execute_batch("CREATE TRIGGER fail_collection_delete BEFORE UPDATE ON clips WHEN NEW.deleted=1 BEGIN SELECT RAISE(ABORT,'fixture'); END")
        .unwrap();
    let revision = storage.history_revision().unwrap();
    assert!(storage.delete_collection(delete.clone()).is_err());
    assert_eq!(storage.history_revision().unwrap(), revision);
    assert_eq!(storage.collections().unwrap().items.len(), 1);
    assert_eq!(storage.history(query(None)).unwrap().total, 1);
    other_connection
        .execute_batch("DROP TRIGGER fail_collection_delete")
        .unwrap();
    assert_eq!(storage.delete_collection(delete).unwrap(), 1);
    assert!(storage.collections().unwrap().items.is_empty());
    assert_eq!(storage.history(query(None)).unwrap().total, 0);
    let trash = storage.history_window(query(None), true, None).unwrap();
    assert_eq!(trash.items[0].id, clip);
    assert_eq!(
        trash.items[0].collection_id.as_deref(),
        Some(group.id.as_str())
    );
    let recreated = category(&mut storage, "Work");
    assert_ne!(recreated.id, group.id);
    storage
        .change_clip_deletion(ChangeClipDeletion {
            id: clip.clone(),
            expected_revision: trash.items[0].revision.clone(),
            deleted: false,
        })
        .unwrap();
    assert_eq!(
        storage.history(query(None)).unwrap().items[0].collection_id,
        None
    );
    assert_eq!(
        storage.read_clip(&clip).unwrap(),
        ClipPayload::Text("keep the payload".into())
    );
}

#[test]
fn batch_copy_preserves_unprocessed_sources_and_reports_the_confirmed_prefix() {
    let folder = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(&folder.path().join("batch.sqlite3")).unwrap();
    let destination = category(&mut storage, "Copies");
    let first = storage
        .capture(ClipPayload::Text("first".into()))
        .unwrap()
        .unwrap();
    let second = storage
        .capture(ClipPayload::Text("second".into()))
        .unwrap()
        .unwrap();
    policy(&mut storage, 3, "");
    let batch = storage
        .transfer_clips(TransferClips {
            clips: vec![
                ClipVersion {
                    id: first.clone(),
                    expected_revision: "0".into(),
                },
                ClipVersion {
                    id: second.clone(),
                    expected_revision: "0".into(),
                },
            ],
            collection_id: Some(destination.id.clone()),
            mode: TransferMode::Copy,
        })
        .unwrap();
    assert_eq!(batch.completed.len(), 1);
    assert_eq!(batch.failure.unwrap().code, ErrorCode::LimitExceeded);
    assert_eq!(
        storage.history(query(Some(&destination.id))).unwrap().total,
        1
    );
    assert!(storage.read_clip(&first).is_ok());
    assert!(storage.read_clip(&second).is_ok());
}

#[test]
fn edits_preserve_identity_and_pinning_and_range_queries_require_a_current_snapshot() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("edit.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    let id = storage.create_text("original".into()).unwrap();
    storage
        .change_clip_pin(ChangeClipPin {
            id: id.clone(),
            expected_revision: "0".into(),
            pinned: true,
        })
        .unwrap();
    let version = ClipVersion {
        id: id.clone(),
        expected_revision: "1".into(),
    };
    assert_eq!(
        storage.editable_content(version.clone()).unwrap(),
        EditableContent::Text("original".into())
    );
    for index in 0..30 {
        storage.create_text(format!("range {index:02}")).unwrap();
    }
    let mut range = query(None);
    assert_eq!(
        storage
            .history_window(range.clone(), false, Some(27))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    range.expected_revision = Some(storage.history_revision().unwrap());
    range.offset = 20;
    assert_eq!(
        storage
            .history_window(range.clone(), false, Some(10))
            .unwrap()
            .items
            .len(),
        10
    );
    storage
        .edit_clip(EditClip {
            clip: version.clone(),
            content: EditableContent::Text("fresh text\n ".into()),
        })
        .unwrap();
    assert_eq!(
        storage
            .history_window(range, false, Some(10))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        storage
            .edit_clip(EditClip {
                clip: version,
                content: EditableContent::Text("stale write".into())
            })
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let source_file = folder.path().join("original-file");
    std::fs::write(&source_file, "unchanged").unwrap();
    let original = source_file.to_str().unwrap().to_owned();
    let replacement = folder
        .path()
        .join("replacement-file")
        .to_str()
        .unwrap()
        .to_owned();
    let files = storage
        .capture(ClipPayload::Files(vec![original]))
        .unwrap()
        .unwrap();
    storage
        .edit_clip(EditClip {
            clip: ClipVersion {
                id: files.clone(),
                expected_revision: "0".into(),
            },
            content: EditableContent::Files(vec![replacement.clone()]),
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(source_file).unwrap(), "unchanged");
    drop(storage);
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        storage.read_clip(&files).unwrap(),
        ClipPayload::Files(vec![replacement])
    );
    let first = &storage.history(query(None)).unwrap().items[0];
    assert_eq!(first.id, id);
    assert!(first.pinned);
    assert_eq!(
        storage.read_clip(&id).unwrap(),
        ClipPayload::Text("fresh text\n ".into())
    );
}
