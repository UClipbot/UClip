use rusqlite::{params, Connection};
use uclip_core::*;

fn query(pinned_only: bool) -> HistoryQuery {
    HistoryQuery {
        query: String::new(),
        regex: false,
        kind: None,
        pinned_only,
        offset: 0,
        expected_revision: None,
    }
}
fn item(storage: &Storage, id: &str) -> HistoryItem {
    let mut request = query(false);
    loop {
        let page = storage.history(request.clone()).unwrap();
        if let Some(item) = page.items.into_iter().find(|i| i.id == id) {
            return item;
        }
        request.offset += 25;
        assert!(request.offset < page.total, "fixture item missing");
    }
}
fn pin(storage: &mut Storage, id: &str, pinned: bool) -> ClipPinSnapshot {
    storage
        .change_clip_pin(ChangeClipPin {
            id: id.into(),
            expected_revision: item(storage, id).revision,
            pinned,
        })
        .unwrap()
}
fn policy(storage: &mut Storage, max_items: u32) {
    storage
        .update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: String::new(),
            max_items,
            expected_revision: storage.capture_policy().unwrap().revision,
        })
        .unwrap();
}
fn deduplicate(storage: &mut Storage) -> DeduplicationSnapshot {
    storage
        .deduplicate_pinned(DeduplicatePinned {
            expected_revision: storage.history_revision().unwrap(),
        })
        .unwrap()
}

#[test]
fn pin_keeps_identity_order_metadata_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pins.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let a = s.create_text("first".into()).unwrap();
    let b = s.create_text("second".into()).unwrap();
    let before = item(&s, &a);
    let pinned = pin(&mut s, &a, true);
    assert_eq!(pinned.revision, "1");
    let page = s.history(query(false)).unwrap();
    assert_eq!(page.items[0].id, a);
    assert_eq!(page.items[1].id, b);
    assert!(page.items[0].pinned);
    assert_eq!(page.items[0].captured_at, before.captured_at);
    assert_eq!(page.items[0].last_used_at, before.last_used_at);
    assert_eq!(s.history(query(true)).unwrap().total, 1);
    drop(s);
    let mut s = Storage::open(&path).unwrap();
    assert_eq!(s.history(query(true)).unwrap().items[0].id, a);
    pin(&mut s, &a, false);
    assert_eq!(s.history(query(true)).unwrap().total, 0);
    assert_eq!(s.history(query(false)).unwrap().items[0].id, b);
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT revision,operation FROM clip_changes WHERE id=?1",
            [&a],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        )
        .unwrap(),
        (2, "upsert".into())
    );
}

#[test]
fn pin_rejects_stale_deleted_missing_and_malformed_requests_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("pins.sqlite3")).unwrap();
    let id = s.create_text("item".into()).unwrap();
    pin(&mut s, &id, true);
    let before = s.history_revision().unwrap();
    for (rev, value, code) in [
        ("0", false, ErrorCode::Conflict),
        ("1", true, ErrorCode::Conflict),
        ("01", false, ErrorCode::InvalidInput),
        ("-1", false, ErrorCode::InvalidInput),
        ("9223372036854775807", false, ErrorCode::InvalidInput),
    ] {
        assert_eq!(
            s.change_clip_pin(ChangeClipPin {
                id: id.clone(),
                expected_revision: rev.into(),
                pinned: value
            })
            .unwrap_err()
            .code,
            code
        );
        assert_eq!(s.history_revision().unwrap(), before);
    }
    for bad in ["invalid".to_owned(), uuid::Uuid::new_v4().to_string()] {
        let expected = if bad == "invalid" {
            ErrorCode::InvalidInput
        } else {
            ErrorCode::NotFound
        };
        assert_eq!(
            s.change_clip_pin(ChangeClipPin {
                id: bad,
                expected_revision: "0".into(),
                pinned: true
            })
            .unwrap_err()
            .code,
            expected
        );
    }
    s.change_clip_deletion(ChangeClipDeletion {
        id: id.clone(),
        expected_revision: "1".into(),
        deleted: true,
    })
    .unwrap();
    assert_eq!(
        s.change_clip_pin(ChangeClipPin {
            id,
            expected_revision: "2".into(),
            pinned: false
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn pinned_filter_search_and_pagination_apply_before_page_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("pins.sqlite3")).unwrap();
    for n in 0..30 {
        let id = s.create_text(format!("pinned match {n:02}")).unwrap();
        pin(&mut s, &id, true);
    }
    s.create_text("new ordinary match".into()).unwrap();
    let first = s.history(query(true)).unwrap();
    assert_eq!(first.total, 30);
    assert_eq!(first.items.len(), 25);
    assert!(first.items.iter().all(|i| i.pinned));
    let mut next = query(true);
    next.offset = 25;
    next.expected_revision = Some(first.revision.clone());
    let last = s.history(next.clone()).unwrap();
    assert_eq!(last.items.len(), 5);
    assert!(last
        .items
        .iter()
        .all(|i| first.items.iter().all(|j| j.id != i.id)));
    let mut search = query(true);
    search.query = "^pinned match 0[0-2]$".into();
    search.regex = true;
    assert_eq!(s.history(search.clone()).unwrap().total, 3);
    search.kind = Some(ClipKind::Image);
    assert_eq!(s.history(search).unwrap().total, 0);
    s.create_text("changes history".into()).unwrap();
    assert_eq!(s.history(next).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn pinned_capture_is_separate_from_new_capture_and_capacity_protects_pins() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("pins.sqlite3")).unwrap();
    policy(&mut s, 2);
    let payload = ClipPayload::Text("protected".into());
    let a = s.capture(payload.clone()).unwrap().unwrap();
    pin(&mut s, &a, true);
    let duplicate = s.capture(payload.clone()).unwrap().unwrap();
    assert_ne!(a, duplicate);
    assert_eq!(s.capture(payload.clone()).unwrap().unwrap(), duplicate);
    let other = s
        .capture(ClipPayload::Text("other".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        s.read_clip(&duplicate).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(item(&s, &a).pinned);
    policy(&mut s, 1);
    assert_eq!(s.read_clip(&other).unwrap_err().code, ErrorCode::NotFound);
    let before = s.history_revision().unwrap();
    assert_eq!(
        s.capture(ClipPayload::Text("no room".into()))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(s.history_revision().unwrap(), before);
    assert_eq!(
        s.update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: String::new(),
            max_items: 1,
            expected_revision: "0".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    pin(&mut s, &a, false);
    s.capture(ClipPayload::Text("room after unpin".into()))
        .unwrap();
    assert_eq!(s.read_clip(&a).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn dedup_removes_all_exact_non_pinned_copies_and_keeps_every_pinned_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pins.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    policy(&mut s, 500);
    let text = format!("{} tail", "前".repeat(310));
    let a = s.create_text(text.clone()).unwrap();
    pin(&mut s, &a, true);
    let second_pin = s.create_text(text.clone()).unwrap();
    pin(&mut s, &second_pin, true);
    let manual = s.create_text(text.clone()).unwrap();
    let capture = s.capture(ClipPayload::Text(text.clone())).unwrap().unwrap();
    let similar = s
        .create_text(format!("{} different", "前".repeat(310)))
        .unwrap();
    let whitespace = s.create_text(format!("{text} ")).unwrap();
    let deleted = s.create_text(text.clone()).unwrap();
    s.change_clip_deletion(ChangeClipDeletion {
        id: deleted.clone(),
        expected_revision: "0".into(),
        deleted: true,
    })
    .unwrap();
    let before = s.history_revision().unwrap().parse::<i64>().unwrap();
    let result = deduplicate(&mut s);
    assert_eq!(result.removed, 2);
    assert_eq!(result.revision, (before + 1).to_string());
    assert_eq!(s.history(query(false)).unwrap().total, 4);
    assert_eq!(s.history(query(true)).unwrap().total, 2);
    for id in [&a, &second_pin, &similar, &whitespace] {
        assert!(s.read_clip(id).is_ok());
    }
    let c = Connection::open(&path).unwrap();
    for id in [&manual, &capture] {
        assert_eq!(s.read_clip(id).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(
            c.query_row(
                "SELECT deleted,revision FROM clips WHERE id=?1",
                [id],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            (true, 1)
        );
        assert_eq!(
            c.query_row(
                "SELECT operation,ordinal FROM clip_changes WHERE id=?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            ("delete".into(), before + 1)
        );
    }
    assert_eq!(
        c.query_row("SELECT revision FROM clips WHERE id=?1", [&deleted], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    let repeat = deduplicate(&mut s);
    assert_eq!(repeat.removed, 0);
    assert_eq!(repeat.revision, result.revision);
    drop(s);
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.history(query(true)).unwrap().total, 2);
    assert_eq!(s.history(query(false)).unwrap().total, 4);
}

#[test]
fn dedup_compares_typed_image_and_file_payloads_and_does_not_trust_hash_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pins.sqlite3");
    let file_a = dir.path().join("a").to_string_lossy().into_owned();
    let file_b = dir.path().join("b").to_string_lossy().into_owned();
    let mut s = Storage::open(&path).unwrap();
    policy(&mut s, 500);
    for payload in [
        ClipPayload::Image {
            width: 2,
            height: 1,
            rgba: vec![100; 8],
        },
        ClipPayload::Files(vec![file_a.clone(), file_b.clone()]),
    ] {
        let a = s.capture(payload.clone()).unwrap().unwrap();
        pin(&mut s, &a, true);
        let b = s.capture(payload).unwrap().unwrap();
        assert_ne!(a, b);
    }
    let dim = s
        .capture(ClipPayload::Image {
            width: 1,
            height: 2,
            rgba: vec![100; 8],
        })
        .unwrap()
        .unwrap();
    let pixels = s
        .capture(ClipPayload::Image {
            width: 2,
            height: 1,
            rgba: vec![101; 8],
        })
        .unwrap()
        .unwrap();
    let files = s
        .capture(ClipPayload::Files(vec![file_b, file_a]))
        .unwrap()
        .unwrap();
    let a = s.create_text("anchor".into()).unwrap();
    pin(&mut s, &a, true);
    let different = s.create_text("different".into()).unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute(
        "UPDATE clips SET fingerprint=(SELECT fingerprint FROM clips WHERE id=?1) WHERE id=?2",
        params![a, different],
    )
    .unwrap();
    assert_eq!(deduplicate(&mut s).removed, 2);
    for id in [&dim, &pixels, &files, &different] {
        assert!(s.read_clip(id).is_ok());
    }
}

#[test]
fn dedup_without_live_pins_is_noop_and_rejects_old_history_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("pins.sqlite3")).unwrap();
    let a = s.create_text("same".into()).unwrap();
    s.create_text("same".into()).unwrap();
    let before = s.history_revision().unwrap();
    assert_eq!(deduplicate(&mut s).revision, before);
    pin(&mut s, &a, true);
    assert_eq!(
        s.deduplicate_pinned(DeduplicatePinned {
            expected_revision: before
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    for rev in ["01", "-1", "bad", "9223372036854775807"] {
        assert_eq!(
            s.deduplicate_pinned(DeduplicatePinned {
                expected_revision: rev.into()
            })
            .unwrap_err()
            .code,
            ErrorCode::InvalidInput
        );
    }
    s.change_clip_deletion(ChangeClipDeletion {
        id: a,
        expected_revision: "1".into(),
        deleted: true,
    })
    .unwrap();
    assert_eq!(deduplicate(&mut s).removed, 0);
    assert_eq!(s.history(query(false)).unwrap().total, 1);
}

#[test]
fn failed_change_tracking_rolls_back_pin_and_entire_dedup_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pins.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let a = s.create_text("same".into()).unwrap();
    pin(&mut s, &a, true);
    let b = s.create_text("same".into()).unwrap();
    let d = s.create_text("same".into()).unwrap();
    let before = s.history_revision().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_changes BEFORE UPDATE ON clip_changes BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        s.change_clip_pin(ChangeClipPin {
            id: b.clone(),
            expected_revision: "0".into(),
            pinned: true
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(
        s.deduplicate_pinned(DeduplicatePinned {
            expected_revision: before.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(s.history_revision().unwrap(), before);
    assert!(!item(&s, &b).pinned);
    for id in [&b, &d] {
        assert_eq!(item(&s, id).revision, "0");
        assert_eq!(
            c.query_row(
                "SELECT operation FROM clip_changes WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "upsert"
        );
    }
    c.execute_batch("DROP TRIGGER fail_changes").unwrap();
    assert_eq!(deduplicate(&mut s).removed, 2);
}
