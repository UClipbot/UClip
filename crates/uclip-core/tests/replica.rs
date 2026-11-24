use std::{collections::BTreeSet, path::Path};

use uclip_core::{
    ChangeClipDeletion, ChangeClipPin, ClipPayload, ClipVersion, CreateCollection,
    DeleteCollection, EditClip, EditableContent, EmptyTrash, ErrorCode, PurgeTrash, ReplicaBody,
    ReplicaEntry, ReplicaType, Storage,
};

fn storage(path: &Path) -> Storage {
    let mut store = Storage::open(path).unwrap();
    store.enroll_replica().unwrap();
    store
}

#[test]
fn remote_collection_deletion_keeps_unshared_records_in_active_local_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let mut source = Storage::open(&path).unwrap();
    let collection = source
        .create_collection(CreateCollection {
            name: "Existing collection".into(),
        })
        .unwrap();
    let local = source
        .create_text_in_collection("Local only".into(), Some(collection.id.clone()))
        .unwrap();
    source
        .save_replica_profile("new-only".into(), true)
        .unwrap();
    let mut peer = storage(&dir.path().join("peer.sqlite3"));
    transfer(&mut source, &mut peer);

    let shared = peer.collections().unwrap().items.remove(0);
    peer.delete_collection(DeleteCollection {
        id: shared.id,
        expected_revision: shared.revision,
    })
    .unwrap();
    transfer(&mut peer, &mut source);

    assert!(source.collections().unwrap().items.is_empty());
    let active = history_items(&source, false);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, local);
    assert_eq!(active[0].collection_id, None);
    assert!(source
        .replica_inventory()
        .unwrap()
        .iter()
        .all(|stamp| stamp.id.id != local));
    drop(source);

    let legacy = rusqlite::Connection::open(&path).unwrap();
    legacy
        .execute(
            "UPDATE clips SET collection=?1 WHERE id=?2",
            [&collection.id, &local],
        )
        .unwrap();
    legacy.pragma_update(None, "user_version", 20).unwrap();
    drop(legacy);
    let migrated = Storage::open(&path).unwrap();
    let active = history_items(&migrated, false);
    assert_eq!(active[0].id, local);
    assert_eq!(active[0].collection_id, None);
}

#[test]
fn oversized_history_can_create_a_group_with_new_records_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let mut store = Storage::open(&path).unwrap();
    let original = store
        .capture(ClipPayload::Text("Existing clipboard content".into()))
        .unwrap()
        .unwrap();
    let fixture = rusqlite::Connection::open(&path).unwrap();
    fixture.execute_batch(
        "WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<25000)
         INSERT INTO clips(id,kind,text,data,fingerprint,captured_at,sequence,revision)
         SELECT printf('00000000-0000-4000-8000-%012x',n),'text','legacy',X'',zeroblob(32),n,n,0 FROM rows;",
    ).unwrap();
    drop(fixture);

    let failed = store
        .save_replica_profile("full-history".into(), false)
        .unwrap_err();
    assert_eq!(failed.code, ErrorCode::LimitExceeded);
    assert!(store.replica_profile().unwrap().is_none());
    assert!(store.replica_inventory().unwrap().is_empty());

    store.save_replica_profile("new-only".into(), true).unwrap();
    drop(store);
    let mut store = storage(&path);
    assert_eq!(
        store.replica_profile().unwrap().as_deref(),
        Some("new-only")
    );
    assert!(store.replica_inventory().unwrap().is_empty());
    assert_eq!(store.replica_local_only_count().unwrap(), 25_001);
    let shared = store
        .capture(ClipPayload::Text("Existing clipboard content".into()))
        .unwrap()
        .unwrap();
    assert_ne!(shared, original);
    store
        .change_clip_pin(ChangeClipPin {
            id: original.clone(),
            expected_revision: "0".into(),
            pinned: true,
        })
        .unwrap();
    let mut inventory = store.replica_inventory().unwrap();
    assert_eq!(inventory.len(), 1);
    let stamp = inventory.pop().unwrap();
    assert_eq!(stamp.id.id, shared);
    let mut peer = storage(&dir.path().join("peer.sqlite3"));
    assert!(peer
        .apply_replica_entry(store.replica_entry(&stamp).unwrap().unwrap())
        .unwrap());
    assert_eq!(
        peer.read_clip(&shared).unwrap(),
        store.read_clip(&original).unwrap()
    );
    assert!(peer.read_clip(&original).is_err());
    assert_eq!(
        store
            .history_window(
                uclip_core::HistoryQuery {
                    query: String::new(),
                    regex: false,
                    kind: None,
                    pinned_only: false,
                    collection_id: None,
                    offset: 25_000,
                    expected_revision: None,
                },
                false,
                None
            )
            .unwrap()
            .items
            .len(),
        2
    );
    store
        .change_clip_deletion(ChangeClipDeletion {
            id: original,
            expected_revision: "1".into(),
            deleted: true,
        })
        .unwrap();
    assert_eq!(
        store
            .purge_trash(PurgeTrash::All(EmptyTrash {
                expected_revision: store.history_revision().unwrap(),
            }))
            .unwrap(),
        1
    );
    assert!(store.replica_inventory().unwrap() == [stamp]);
    assert_eq!(store.replica_local_only_count().unwrap(), 25_000);
}

fn entries(store: &mut Storage) -> Vec<ReplicaEntry> {
    store
        .replica_inventory()
        .unwrap()
        .into_iter()
        .map(|stamp| store.replica_entry(&stamp).unwrap().unwrap())
        .collect()
}

fn set_pending_time(path: &Path, id: &str, time: i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE replica_entities SET changed_at=?1 WHERE id=?2",
                rusqlite::params![time, id]
            )
            .unwrap(),
        1
    );
}

fn transfer(source: &mut Storage, target: &mut Storage) {
    for entry in entries(source) {
        target.apply_replica_entry(entry).unwrap();
    }
}

fn keys(store: &mut Storage) -> BTreeSet<String> {
    store
        .replica_inventory()
        .unwrap()
        .into_iter()
        .map(|stamp| stamp.key())
        .collect()
}

fn bodies(store: &mut Storage) -> BTreeSet<(String, bool)> {
    entries(store)
        .into_iter()
        .filter_map(|entry| match entry.body {
            Some(ReplicaBody::Clip {
                text,
                kind,
                deleted,
                ..
            }) if kind == "text" => Some((text, deleted)),
            _ => None,
        })
        .collect()
}

#[test]
fn two_existing_histories_merge_and_the_latest_edit_wins() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Storage::open(&dir.path().join("a")).unwrap();
    let mut b = Storage::open(&dir.path().join("b")).unwrap();
    let item = a.create_text("shared".into()).unwrap();
    b.create_text("b first".into()).unwrap();
    a.enroll_replica().unwrap();
    b.enroll_replica().unwrap();
    transfer(&mut a, &mut b);
    a.edit_clip(EditClip {
        clip: ClipVersion {
            id: item.clone(),
            expected_revision: "0".into(),
        },
        content: EditableContent::Text("a edit".into()),
    })
    .unwrap();
    b.edit_clip(EditClip {
        clip: ClipVersion {
            id: item.clone(),
            expected_revision: "0".into(),
        },
        content: EditableContent::Text("b edit".into()),
    })
    .unwrap();
    set_pending_time(&dir.path().join("a"), &item, 10);
    set_pending_time(&dir.path().join("b"), &item, 20);
    transfer(&mut b, &mut a);
    transfer(&mut a, &mut b);
    assert_eq!(keys(&mut a), keys(&mut b));
    assert_eq!(
        bodies(&mut a),
        BTreeSet::from([("b edit".into(), false), ("b first".into(), false)])
    );
    assert_eq!(bodies(&mut a), bodies(&mut b));
    let before = a.history_revision().unwrap();
    transfer(&mut b, &mut a);
    assert_eq!(a.history_revision().unwrap(), before);
}

#[test]
fn concurrent_edit_and_delete_use_the_latest_operation_time() {
    for delete_wins in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut a = storage(&dir.path().join("a"));
        let mut b = storage(&dir.path().join("b"));
        let id = a.create_text("old".into()).unwrap();
        transfer(&mut a, &mut b);
        a.change_clip_deletion(ChangeClipDeletion {
            id: id.clone(),
            expected_revision: "0".into(),
            deleted: true,
        })
        .unwrap();
        b.edit_clip(EditClip {
            clip: ClipVersion {
                id: id.clone(),
                expected_revision: "0".into(),
            },
            content: EditableContent::Text("edit".into()),
        })
        .unwrap();
        set_pending_time(
            &dir.path().join("a"),
            &id,
            if delete_wins { 20 } else { 10 },
        );
        set_pending_time(
            &dir.path().join("b"),
            &id,
            if delete_wins { 10 } else { 20 },
        );
        let from_a = entries(&mut a);
        let from_b = entries(&mut b);
        for entry in from_a {
            b.apply_replica_entry(entry).unwrap();
        }
        for entry in from_b {
            a.apply_replica_entry(entry).unwrap();
        }
        let expected = if delete_wins {
            ("old".into(), true)
        } else {
            ("edit".into(), false)
        };
        assert_eq!(bodies(&mut a), BTreeSet::from([expected]));
        assert_eq!(keys(&mut a), keys(&mut b));
        let items = history_items(&b, delete_wins);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert!(history_items(&b, !delete_wins).is_empty());
    }
}

#[test]
fn same_collection_name_on_each_device_preserves_both_identities() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = storage(&dir.path().join("a"));
    let mut b = storage(&dir.path().join("b"));
    let a_collection = a
        .create_collection(CreateCollection {
            name: "Notes".into(),
        })
        .unwrap();
    let b_collection = b
        .create_collection(CreateCollection {
            name: "Notes".into(),
        })
        .unwrap();
    a.create_text_in_collection("Alpha".into(), Some(a_collection.id))
        .unwrap();
    b.create_text_in_collection("Beta".into(), Some(b_collection.id))
        .unwrap();
    transfer(&mut a, &mut b);
    transfer(&mut b, &mut a);
    let names = |store: &Storage| {
        store
            .collections()
            .unwrap()
            .items
            .into_iter()
            .map(|item| (item.id, item.name))
            .collect::<BTreeSet<_>>()
    };
    let projected = names(&a);
    assert_eq!(projected, names(&b));
    assert_eq!(projected.len(), 2);
    assert!(projected.iter().any(|(_, name)| name == "Notes"));
    assert!(projected
        .iter()
        .any(|(_, name)| name.starts_with("Notes (")));
    assert_eq!(keys(&mut a), keys(&mut b));
}

#[test]
fn usage_does_not_replicate_and_remote_file_ticket_requires_a_local_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = storage(&dir.path().join("a"));
    let mut b = storage(&dir.path().join("b"));
    let path = dir.path().join("file.txt").to_string_lossy().into_owned();
    let id = a
        .capture(ClipPayload::Files(vec![path.clone()]))
        .unwrap()
        .unwrap();
    transfer(&mut a, &mut b);
    let ticket = b.remote_file_ticket(&id).unwrap().unwrap();
    assert_eq!(ticket.device, a.replica_device().unwrap());
    assert_eq!(a.replica_grant(&ticket.token).unwrap(), [path]);
    assert!(b.replica_grant(&ticket.token).is_err());
    let before = keys(&mut b);
    b.mark_used(&id).unwrap();
    assert_eq!(keys(&mut b), before);
    b.change_clip_pin(ChangeClipPin {
        id,
        expected_revision: "1".into(),
        pinned: true,
    })
    .unwrap();
    transfer(&mut b, &mut a);
    assert_eq!(keys(&mut a), keys(&mut b));
    assert!(a
        .replica_inventory()
        .unwrap()
        .iter()
        .all(|stamp| stamp.id.kind == ReplicaType::Clip));
}

#[test]
fn three_independent_edits_and_new_items_converge_by_item_time() {
    let directory = tempfile::tempdir().unwrap();
    let paths =
        ["chronology-a", "chronology-b", "chronology-c"].map(|name| directory.path().join(name));
    let [mut a, mut b, mut c] = paths.each_ref().map(|path| storage(path));
    let shared = a.create_text("before edits".into()).unwrap();
    transfer(&mut a, &mut b);
    transfer(&mut a, &mut c);

    let set_time = |path: &Path, id: &str, time: i64| {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE clips SET captured_at=?1 WHERE id=?2",
                rusqlite::params![time, id],
            )
            .unwrap();
    };
    for (store, path, edit, time, new_item, new_time) in [
        (&mut a, &paths[0], "edit from A", 20, "new A", 40),
        (&mut b, &paths[1], "edit from B", 60, "new B", 30),
        (&mut c, &paths[2], "edit from C", 10, "new C", 50),
    ] {
        let version = ClipVersion {
            id: shared.clone(),
            expected_revision: "0".into(),
        };
        store
            .edit_clip(EditClip {
                clip: version,
                content: EditableContent::Text(edit.into()),
            })
            .unwrap();
        set_time(path, &shared, time);
        set_pending_time(path, &shared, time);
        let new_id = store.create_text(new_item.into()).unwrap();
        set_time(path, &new_id, new_time);
    }
    transfer(&mut c, &mut a);
    transfer(&mut a, &mut b);
    transfer(&mut b, &mut c);
    transfer(&mut c, &mut a);
    let ordered = |store: &Storage| {
        store
            .history(uclip_core::HistoryQuery {
                query: String::new(),
                regex: false,
                kind: None,
                pinned_only: false,
                collection_id: None,
                offset: 0,
                expected_revision: None,
            })
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.preview)
            .collect::<Vec<_>>()
    };
    let expected = ["edit from B", "new C", "new A", "new B"];
    for store in [&mut a, &mut b, &mut c] {
        assert_eq!(ordered(store), expected);
        assert_eq!(
            store.read_clip(&shared).unwrap(),
            ClipPayload::Text("edit from B".into())
        );
    }
    assert_eq!(keys(&mut a), keys(&mut b));
    assert_eq!(keys(&mut b), keys(&mut c));
    assert_eq!(a.quick_paste_slots(false).unwrap()[0].id, shared);
}

fn history_items(store: &Storage, trash: bool) -> Vec<uclip_core::HistoryItem> {
    store
        .history_window(
            uclip_core::HistoryQuery {
                query: String::new(),
                regex: false,
                kind: None,
                pinned_only: false,
                collection_id: None,
                offset: 0,
                expected_revision: None,
            },
            trash,
            None,
        )
        .unwrap()
        .items
}

#[test]
fn a_restore_replaces_the_deleted_head_in_every_delivery_order() {
    let temp = tempfile::tempdir().unwrap();
    let mut a = storage(&temp.path().join("a"));
    let mut b = storage(&temp.path().join("b"));
    let id = a.create_text("original".into()).unwrap();
    transfer(&mut a, &mut b);
    a.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    let mut deleted = entries(&mut a).remove(0);
    deleted.stamp.changed_at = 10;
    a.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "1".into(),
        deleted: false,
    })
    .unwrap();
    let mut restored = entries(&mut a).remove(0);
    restored.stamp.changed_at = 30;
    b.edit_clip(EditClip {
        clip: ClipVersion {
            id: id.clone(),
            expected_revision: "0".into(),
        },
        content: EditableContent::Text("independent edit".into()),
    })
    .unwrap();
    let mut edited = entries(&mut b).remove(0);
    edited.stamp.changed_at = 20;
    let deliveries = [deleted, restored, edited].map(|entry| rmp_serde::to_vec(&entry).unwrap());

    let mut expected = None;
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let path = temp.path().join(format!("c-{order:?}"));
        let mut c = storage(&path);
        for index in order {
            c.apply_replica_entry(rmp_serde::from_slice(&deliveries[index]).unwrap())
                .unwrap();
        }
        drop(c);
        let mut c = Storage::open(&path).unwrap();
        let active = history_items(&c, false)
            .into_iter()
            .map(|item| (item.id, item.preview))
            .collect::<BTreeSet<_>>();
        assert!(history_items(&c, true).is_empty(), "{order:?}");
        assert_eq!(
            active
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["original"]),
            "{order:?}"
        );
        let state = (keys(&mut c), active);
        if let Some(previous) = &expected {
            assert_eq!(&state, previous, "{order:?}");
        } else {
            expected = Some(state);
        }
        let old_revision = c.history_revision().unwrap();
        assert!(!c
            .apply_replica_entry(rmp_serde::from_slice(&deliveries[0]).unwrap())
            .unwrap());
        assert_eq!(c.history_revision().unwrap(), old_revision);
    }
}

#[test]
fn permanent_deletion_wins_over_a_later_offline_restore_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let a_path = temp.path().join("a");
    let b_path = temp.path().join("b");
    let mut a = storage(&a_path);
    let mut b = storage(&b_path);
    let id = a.create_text("shared".into()).unwrap();
    a.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    set_pending_time(&a_path, &id, 10);
    let deleted = entries(&mut a).remove(0);
    transfer(&mut a, &mut b);

    a.purge_trash(PurgeTrash::All(EmptyTrash {
        expected_revision: a.history_revision().unwrap(),
    }))
    .unwrap();
    set_pending_time(&a_path, &id, 20);
    // B hasn't observed the purge. Its restore is deliberately later.
    b.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: history_items(&b, true)[0].revision.clone(),
        deleted: false,
    })
    .unwrap();
    set_pending_time(&b_path, &id, 30);
    let restored = entries(&mut b).remove(0);
    let replay = rmp_serde::to_vec(&restored).unwrap();
    let revision = a.history_revision().unwrap();
    assert!(!a.apply_replica_entry(restored).unwrap());
    assert_eq!(a.history_revision().unwrap(), revision);

    let tombstone = entries(&mut a).remove(0);
    assert!(tombstone.stamp.removed && tombstone.body.is_none());
    assert!(b.apply_replica_entry(tombstone).unwrap());
    drop((a, b));

    let mut a = Storage::open(&a_path).unwrap();
    let mut b = Storage::open(&b_path).unwrap();
    // A fresh replica receives the live version before the purge and still converges.
    let mut fresh = storage(&temp.path().join("fresh"));
    fresh
        .apply_replica_entry(rmp_serde::from_slice(&replay).unwrap())
        .unwrap();
    assert!(fresh.read_clip(&id).is_ok());
    transfer(&mut b, &mut fresh);
    assert_eq!(keys(&mut a), keys(&mut b));
    assert_eq!(keys(&mut b), keys(&mut fresh));
    for store in [&mut a, &mut b, &mut fresh] {
        assert!(!store
            .apply_replica_entry(rmp_serde::from_slice(&replay).unwrap())
            .unwrap());
        assert!(history_items(store, false).is_empty());
        assert!(history_items(store, true).is_empty());
        assert_eq!(store.read_clip(&id).unwrap_err().code, ErrorCode::NotFound);
    }
    assert!(!fresh.apply_replica_entry(deleted).unwrap());
    let copied = fresh.create_text("shared".into()).unwrap();
    assert_ne!(copied, id);
    transfer(&mut fresh, &mut a);
    assert_eq!(
        a.read_clip(&copied).unwrap(),
        ClipPayload::Text("shared".into())
    );
}

#[test]
fn the_winning_file_keeps_valid_source_authorization_without_a_copy() {
    let temp = tempfile::tempdir().unwrap();
    let mut a = storage(&temp.path().join("a"));
    let mut b = storage(&temp.path().join("b"));
    let source_path = temp
        .path()
        .join("live-file.txt")
        .to_string_lossy()
        .into_owned();
    let id = a
        .capture(ClipPayload::Files(vec![source_path.clone()]))
        .unwrap()
        .unwrap();
    transfer(&mut a, &mut b);
    let ticket = b.remote_file_ticket(&id).unwrap().unwrap();
    a.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    assert!(a.replica_grant(&ticket.token).is_err());
    b.change_clip_pin(ChangeClipPin {
        id: id.clone(),
        expected_revision: "0".into(),
        pinned: true,
    })
    .unwrap();

    set_pending_time(&temp.path().join("a"), &id, 10);
    set_pending_time(&temp.path().join("b"), &id, 20);
    // A remote entry may be aware of an existing token without owning the paths.
    let mut forged = entries(&mut b).remove(0);
    if let Some(ReplicaBody::Clip { text, .. }) = &mut forged.body {
        *text = serde_json::to_string(&[temp.path().join("unshared.txt")]).unwrap();
    }
    forged.stamp.id.id = uuid::Uuid::new_v4().to_string();
    a.apply_replica_entry(forged).unwrap();
    assert!(a.replica_grant(&ticket.token).is_err());
    transfer(&mut b, &mut a);
    transfer(&mut a, &mut b);
    let item = history_items(&b, false)
        .into_iter()
        .find(|item| item.pinned && item.preview == "live-file.txt")
        .unwrap();
    assert_eq!(item.id, id);
    assert_eq!(
        b.remote_file_ticket(&item.id).unwrap().unwrap().token,
        ticket.token
    );
    assert_eq!(a.replica_grant(&ticket.token).unwrap(), vec![source_path]);
}

#[test]
fn restores_in_an_enrolled_library_use_the_shared_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = storage(&temp.path().join("library"));
    let policy = store.capture_policy().unwrap();
    store
        .update_capture_policy(uclip_core::UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: String::new(),
            max_items: 1,
            expected_revision: policy.revision,
        })
        .unwrap();
    let first = store.create_text("first".into()).unwrap();
    store.create_text("second".into()).unwrap();
    store
        .change_clip_deletion(ChangeClipDeletion {
            id: first.clone(),
            expected_revision: "0".into(),
            deleted: true,
        })
        .unwrap();
    store
        .change_clip_deletion(ChangeClipDeletion {
            id: first,
            expected_revision: "1".into(),
            deleted: false,
        })
        .unwrap();
    assert_eq!(history_items(&store, false).len(), 2);
}

#[test]
fn a_full_clock_rejects_only_the_mutation_that_would_exceed_the_protocol_limit() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = storage(&temp.path().join("source"));
    let mut target = storage(&temp.path().join("target"));
    let id = source.create_text("full context".into()).unwrap();
    let mut entry = entries(&mut source).remove(0);
    for _ in 0..31 {
        entry
            .stamp
            .clock
            .insert(uuid::Uuid::new_v4().to_string(), 1);
    }
    target.apply_replica_entry(entry).unwrap();
    let before = target.history_revision().unwrap();
    let err = target
        .change_clip_pin(ChangeClipPin {
            id: id.clone(),
            expected_revision: "0".into(),
            pinned: true,
        })
        .unwrap_err();
    assert_eq!(err.code, uclip_core::ErrorCode::LimitExceeded);
    assert_eq!(target.history_revision().unwrap(), before);
    assert!(!history_items(&target, false)[0].pinned);
    let other = target.create_text("independent record".into()).unwrap();
    let inventory = target.replica_inventory().unwrap();
    assert_eq!(inventory.len(), 2);
    assert!(inventory.iter().any(|stamp| stamp.id.id == other));
    for stamp in inventory {
        stamp.validate().unwrap();
    }
}

fn make_legacy_schema(connection: &rusqlite::Connection, version: u32) {
    for table in ["clips", "collections"] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            connection
                .execute_batch(&format!(
                    "DROP TRIGGER replica_{table}_{action};
                 CREATE TRIGGER replica_{table}_{action} AFTER {action} ON {table}
                 WHEN 0 BEGIN SELECT 1; END;"
                ))
                .unwrap();
        }
    }
    connection
        .execute_batch(
            "ALTER TABLE replica_state DROP COLUMN change_time;
         ALTER TABLE replica_entities DROP COLUMN changed_at;
         ALTER TABLE replica_heads DROP COLUMN changed_at;",
        )
        .unwrap();
    if version == 16 {
        connection
            .execute_batch("DROP TABLE replica_heads")
            .unwrap();
    } else {
        connection.execute_batch(
            "DROP INDEX replica_heads_entity;
             CREATE INDEX replica_heads_entity ON replica_heads(kind,id);
             CREATE TABLE replica_views(kind INTEGER,id TEXT,parent TEXT,head TEXT,PRIMARY KEY(kind,id));"
        ).unwrap();
    }
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
}

#[test]
fn v16_migration_keeps_the_existing_replica_stamp_and_body() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v16");
    let mut legacy = storage(&path);
    let id = legacy
        .create_text("visible before upgrading".into())
        .unwrap();
    let mut stamp = legacy.replica_inventory().unwrap().remove(0);
    stamp.changed_at = 0;
    let device = legacy.replica_device().unwrap();
    drop(legacy);
    let connection = rusqlite::Connection::open(&path).unwrap();
    make_legacy_schema(&connection, 16);
    drop(connection);
    let mut upgraded = Storage::open(&path).unwrap();
    assert_eq!(upgraded.replica_device().unwrap(), device);
    let after = upgraded.replica_inventory().unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].key(), stamp.key());
    assert_eq!(
        upgraded.read_clip(&id).unwrap(),
        ClipPayload::Text("visible before upgrading".into())
    );
    assert!(upgraded.replica_entry(&after[0]).unwrap().is_some());
}

#[test]
fn v17_migration_removes_derived_conflict_rows_and_keeps_independent_items() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v17");
    let mut local = storage(&path);
    let id = local.create_text("winner".into()).unwrap();
    let alias = local.create_text("derived conflict".into()).unwrap();
    let independent = local.create_text("independent record".into()).unwrap();
    let mut stamp = local
        .replica_inventory()
        .unwrap()
        .into_iter()
        .find(|stamp| stamp.id.id == id)
        .unwrap();
    let entry = local.replica_entry(&stamp).unwrap().unwrap();
    let modern_key = stamp.key();
    stamp.changed_at = 0;
    let key = stamp.key();
    let mut loser = entry.stamp;
    loser.writer = uuid::Uuid::new_v4().to_string();
    loser.counter = 1;
    loser.changed_at = 0;
    loser.clock = std::collections::BTreeMap::from([(loser.writer.clone(), 1)]);
    let mut body = entry.body;
    if let Some(ReplicaBody::Clip { text, .. }) = &mut body {
        *text = "derived conflict".into();
    }
    drop(local);
    let connection = rusqlite::Connection::open(&path).unwrap();
    make_legacy_schema(&connection, 17);
    connection
        .execute(
            "UPDATE replica_heads SET key=?1 WHERE key=?2",
            rusqlite::params![key, modern_key],
        )
        .unwrap();
    connection
        .execute("DELETE FROM replica_entities WHERE id=?1", [&alias])
        .unwrap();
    connection
        .execute(
            "INSERT INTO replica_heads(kind,id,key,writer,counter,clock,removed,body,payload_bytes)
        VALUES(1,?1,?2,?3,1,?4,0,?5,16)",
            rusqlite::params![
                id,
                loser.key(),
                loser.writer,
                serde_json::to_string(&loser.clock).unwrap(),
                rmp_serde::to_vec(&body).unwrap()
            ],
        )
        .unwrap();
    // The v17 projection pointed at the competing branch for the original.
    connection
        .execute(
            "UPDATE clips SET text='derived conflict' WHERE id=?1",
            [&id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO replica_views VALUES(1,?1,?1,?2)",
            rusqlite::params![id, loser.key()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO replica_views VALUES(1,?1,?2,?3)",
            rusqlite::params![alias, id, key],
        )
        .unwrap();
    drop(connection);
    let mut upgraded = Storage::open(&path).unwrap();
    assert_eq!(
        upgraded.read_clip(&id).unwrap(),
        ClipPayload::Text("winner".into())
    );
    assert!(upgraded.read_clip(&alias).is_err());
    assert_eq!(
        upgraded.read_clip(&independent).unwrap(),
        ClipPayload::Text("independent record".into())
    );
    assert_eq!(upgraded.replica_inventory().unwrap().len(), 2);
}

#[test]
fn a_new_local_delete_advances_past_an_observed_future_clock() {
    let temp = tempfile::tempdir().unwrap();
    let mut a = storage(&temp.path().join("a"));
    let mut b = storage(&temp.path().join("b"));
    let id = a.create_text("shared".into()).unwrap();
    let mut original = entries(&mut a).remove(0);
    original.stamp.changed_at = 4_000_000_000_000_000;
    let delayed = rmp_serde::to_vec(&original).unwrap();
    assert!(b.apply_replica_entry(original).unwrap());
    b.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    let deleted = entries(&mut b).remove(0);
    assert!(deleted.stamp.changed_at > 4_000_000_000_000_000);
    let path = temp.path().join("b");
    drop(b);
    let mut b = Storage::open(&path).unwrap();
    assert!(!b
        .apply_replica_entry(rmp_serde::from_slice(&delayed).unwrap())
        .unwrap());
    assert!(history_items(&b, false).is_empty());
    assert_eq!(history_items(&b, true)[0].id, id);
}

#[test]
fn archived_stamps_without_operation_times_still_decode_and_keep_their_keys() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = storage(&temp.path().join("legacy-body"));
    store.create_text("old archived record".into()).unwrap();
    let mut entry = entries(&mut store).remove(0);
    entry.stamp.changed_at = 0;
    let expected_key = entry.stamp.key();
    let stamp = &entry.stamp;
    let legacy = (
        (
            &stamp.id,
            &stamp.writer,
            stamp.counter,
            &stamp.clock,
            stamp.removed,
        ),
        &entry.body,
    );
    let decoded: ReplicaEntry =
        rmp_serde::from_slice(&rmp_serde::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(decoded.stamp.changed_at, 0);
    assert_eq!(decoded.stamp.key(), expected_key);
    assert!(decoded.body == entry.body);
}

#[test]
fn a_delete_wins_after_both_replicas_restart_in_the_opposite_order() {
    let temp = tempfile::tempdir().unwrap();
    let a_path = temp.path().join("a");
    let b_path = temp.path().join("b");
    let mut a = storage(&a_path);
    let mut b = storage(&b_path);
    let id = a.create_text("shared".into()).unwrap();
    transfer(&mut a, &mut b);
    drop(a);
    b.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    drop(b);
    let mut a = Storage::open(&a_path).unwrap();
    assert_eq!(
        a.read_clip(&id).unwrap(),
        ClipPayload::Text("shared".into())
    );
    let mut b = Storage::open(&b_path).unwrap();
    // A becomes the coordinator and its old inventory is offered first.
    transfer(&mut a, &mut b);
    transfer(&mut b, &mut a);
    assert_eq!(keys(&mut a), keys(&mut b));
    for store in [&a, &b] {
        assert!(history_items(store, false).is_empty());
        assert_eq!(history_items(store, true)[0].id, id);
    }
}
