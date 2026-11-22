use rusqlite::Connection;
use uclip_core::{AuxiliaryWindow, AuxiliaryWindowSize, ErrorCode, Storage};

#[test]
fn window_sizes_upgrade_from_v14_and_persist_independently() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("window-sizes.sqlite3");
    let storage = Storage::open(&path).unwrap();
    let main = storage.desktop_preferences().unwrap();
    drop(storage);
    let prior = Connection::open(&path).unwrap();
    prior
        .execute_batch("DROP TABLE auxiliary_window_sizes; PRAGMA user_version=14;")
        .unwrap();
    drop(prior);

    let mut storage = Storage::open(&path).unwrap();
    let quick = AuxiliaryWindow::QuickPaste;
    let settings = AuxiliaryWindow::Settings;
    assert_eq!(storage.auxiliary_window_size(quick).unwrap().width, 520);
    assert_eq!(storage.auxiliary_window_size(settings).unwrap().width, 720);

    let panel = AuxiliaryWindowSize {
        width: 640,
        height: 400,
    };
    let preferences = AuxiliaryWindowSize {
        width: 810,
        height: 675,
    };
    storage.save_auxiliary_window_size(quick, panel).unwrap();
    storage
        .save_auxiliary_window_size(settings, preferences)
        .unwrap();
    assert_eq!(
        storage
            .save_auxiliary_window_size(
                settings,
                AuxiliaryWindowSize {
                    width: 10,
                    height: 10
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    drop(storage);

    let storage = Storage::open(&path).unwrap();
    assert_eq!(storage.auxiliary_window_size(quick).unwrap(), panel);
    assert_eq!(
        storage.auxiliary_window_size(settings).unwrap(),
        preferences
    );
    assert_eq!(storage.desktop_preferences().unwrap(), main);
}
