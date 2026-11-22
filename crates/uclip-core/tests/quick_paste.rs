use rusqlite::Connection;
use uclip_core::*;

#[test]
fn slots_filter_before_limiting_and_read_the_exact_snapshot_revision() {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(&directory.path().join("quick.sqlite3")).unwrap();

    let mut regular = vec![];
    for index in 0..6 {
        regular.push(storage.create_text(format!("Regular {index}")).unwrap());
    }
    let pin = storage.create_text("Pinned".into()).unwrap();
    storage
        .change_clip_pin(ChangeClipPin {
            id: pin.clone(),
            expected_revision: "0".into(),
            pinned: true,
        })
        .unwrap();

    let slots = storage.quick_paste_slots(false).unwrap();
    assert_eq!(
        slots.iter().map(|slot| &slot.id).collect::<Vec<_>>(),
        regular.iter().rev().take(5).collect::<Vec<_>>()
    );
    assert_eq!(storage.quick_paste_slots(true).unwrap()[0].id, pin);

    let chosen = &slots[1];
    assert_eq!(
        storage
            .read_quick_paste_slot(&chosen.id, &chosen.revision)
            .unwrap(),
        ClipPayload::Text("Regular 4".into())
    );
    storage.mark_used(&chosen.id).unwrap();
    assert_eq!(
        storage
            .read_quick_paste_slot(&chosen.id, &chosen.revision)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn upgrade_keeps_main_binding_and_persists_quick_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("quick.sqlite3");
    let mut storage = Storage::open(&path).unwrap();
    let desktop = storage.desktop_preferences().unwrap();
    storage
        .update_desktop_preferences(UpdateDesktopPreferences {
            close_to_tray: desktop.close_to_tray,
            auto_hide: desktop.auto_hide,
            native_titlebar: desktop.native_titlebar,
            shortcut: "Control+Alt+K".into(),
            expected_revision: desktop.revision,
        })
        .unwrap();
    drop(storage);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE quick_paste_preferences; DROP TABLE auxiliary_window_sizes; PRAGMA user_version=13;")
        .unwrap();
    drop(connection);

    let mut storage = Storage::open(&path).unwrap();
    let quick = storage.quick_paste_preferences().unwrap();
    assert!(quick.shortcut.is_none());
    assert!(!quick.include_pinned);
    let request = UpdateQuickPastePreferences {
        shortcut: Some("Control+1".into()),
        include_pinned: true,
        expected_revision: quick.revision,
    };
    storage
        .update_quick_paste_preferences(request.clone())
        .unwrap();
    assert_eq!(
        storage
            .update_quick_paste_preferences(request)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    drop(storage);
    let storage = Storage::open(&path).unwrap();
    let saved = storage.quick_paste_preferences().unwrap();
    assert_eq!(saved.shortcut.as_deref(), Some("Control+1"));
    assert!(saved.include_pinned);
    assert_eq!(
        storage.desktop_preferences().unwrap().shortcut,
        "Control+Alt+K"
    );
}
