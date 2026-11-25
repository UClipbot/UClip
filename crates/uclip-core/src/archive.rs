//! Local, transactional history archives. Transport and legacy decoding live outside the core.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    collections::{collection_name, MAX_COLLECTIONS},
    history::{
        check_change_budget, ensure_tab_capacity, history_payload_bytes, matches_capture_exclusion,
        pattern, read_capture_policy, trash_count, validate_payload_usage, MAX_LOCAL_HISTORY_BYTES,
        MAX_TRASH_ITEMS,
    },
    storage::{validate_change_tracking, APPLICATION_ID},
    AppError, ClipPayload, ErrorCode, SCHEMA_VERSION,
};

#[derive(Serialize, Deserialize)]
pub struct ArchiveCollection {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveClip {
    pub id: Uuid,
    pub collection: Option<Uuid>,
    pub captured_at: i64,
    pub last_used_at: Option<i64>,
    pub source_app: Option<String>,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub deleted: bool,
}

const MAX_TIMESTAMP: i64 = 8_640_000_000_000_000;

pub enum ArchiveItem {
    Collection(ArchiveCollection),
    Clip(ArchiveClip, ClipPayload),
}

#[derive(Default, Serialize)]
pub struct ImportReport {
    collections: u64,
    clips: BTreeMap<&'static str, u64>,
    trash: u64,
    pinned: u64,
    existing_collections: u64,
    existing_clips: u64,
    excluded: u64,
    unavailable_collections: u64,
    payload_bytes: i64,
    backup: Option<PathBuf>,
}

fn open(path: &Path, flags: OpenFlags) -> Result<Connection, AppError> {
    let db = Connection::open_with_flags(path, flags)?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.pragma_update(None, "foreign_keys", true)?;
    Ok(db)
}

fn check_database(db: &Connection) -> Result<(), AppError> {
    let app_id: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if app_id != APPLICATION_ID {
        return Err(AppError::new(ErrorCode::ForeignDatabase));
    }
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(AppError::new(ErrorCode::UnsupportedSchema));
    }
    Ok(())
}

fn check_integrity(db: &Connection) -> Result<(), AppError> {
    let ok = db.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))? == "ok";
    if !ok || db.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(AppError::new(ErrorCode::Storage));
    }
    Ok(())
}

fn check_wal(db: &Connection) -> Result<(), AppError> {
    let mode: String = db.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(AppError::new(ErrorCode::Storage));
    }
    Ok(())
}

fn snapshot(path: &Path, copy: &Path) -> Result<(), AppError> {
    let reader = open(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    check_database(&reader)?;
    check_wal(&reader)?;
    reader.execute(
        "VACUUM main INTO ?1",
        [copy
            .to_str()
            .ok_or_else(|| AppError::new(ErrorCode::Storage))?],
    )?;
    Ok(())
}

/// The caller holds the SQLite write reservation while a second connection backs up
/// the last committed state, including any pages already committed to the WAL.
fn backup(path: &Path) -> Result<PathBuf, AppError> {
    let folder = path.with_file_name(format!("clip-history-backup-{}", Uuid::new_v4()));
    let io_error = |_| AppError::new(ErrorCode::Storage);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .map_err(io_error)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(&folder).map_err(io_error)?;
    let copy = folder.join("foundation.sqlite3");
    let result = (|| {
        snapshot(path, &copy)?;
        check_integrity(&open(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)?)?;
        fs::File::options()
            .write(true)
            .open(&copy)
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
        #[cfg(unix)]
        {
            fs::File::open(&folder)
                .and_then(|file| file.sync_all())
                .map_err(io_error)?;
            fs::File::open(
                folder
                    .parent()
                    .ok_or_else(|| AppError::new(ErrorCode::Storage))?,
            )
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&copy);
        let _ = fs::remove_dir(&folder);
    }
    result.map(|()| copy)
}

pub fn export_history(
    path: &Path,
    mut emit: impl FnMut(ArchiveItem) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut db = open(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tx = db.transaction()?;
    check_database(&tx)?;
    let uuid = |id: String| Uuid::parse_str(&id).map_err(|_| AppError::new(ErrorCode::Storage));
    let mut collections =
        tx.prepare("SELECT id,name,deleted FROM collections ORDER BY sequence,id")?;
    let mut rows = collections.query([])?;
    while let Some(row) = rows.next()? {
        emit(ArchiveItem::Collection(ArchiveCollection {
            id: uuid(row.get(0)?)?,
            name: row.get(1)?,
            deleted: row.get(2)?,
        }))?;
    }
    let order = if crate::replica::enrolled(&tx)? {
        "captured_at"
    } else {
        "sequence"
    };
    // Sort only IDs; keep at most one clipboard payload in memory.
    let mut ids = tx.prepare(&format!("SELECT id FROM clips ORDER BY {order},id DESC"))?;
    let mut clips = tx.prepare_cached(
        "SELECT kind,text,data,width,height,
             (SELECT id FROM collections WHERE id=clips.collection),
             captured_at,last_used_at,source_app,origin='manual',pinned,deleted
         FROM clips WHERE id=?1",
    )?;
    let mut rows = ids.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let mut item = clips.query([&id])?;
        let entry = item
            .next()?
            .ok_or_else(|| AppError::new(ErrorCode::Storage))?;
        let payload = match entry
            .get_ref(0)?
            .as_str()
            .map_err(|_| AppError::new(ErrorCode::Storage))?
        {
            "text" => ClipPayload::Text(entry.get(1)?),
            // A remote file ticket cannot be restored as a local path reference.
            "files" if crate::replica::remote_file(&tx, &id)?.is_some() => continue,
            "files" => ClipPayload::Files(
                serde_json::from_str(
                    entry
                        .get_ref(1)?
                        .as_str()
                        .map_err(|_| AppError::new(ErrorCode::Storage))?,
                )
                .map_err(|_| AppError::new(ErrorCode::Storage))?,
            ),
            "image" => ClipPayload::Image {
                width: entry.get(3)?,
                height: entry.get(4)?,
                rgba: entry.get(2)?,
            },
            _ => return Err(AppError::new(ErrorCode::Storage)),
        };
        emit(ArchiveItem::Clip(
            ArchiveClip {
                id: uuid(id)?,
                collection: entry.get::<_, Option<String>>(5)?.map(uuid).transpose()?,
                captured_at: entry.get(6)?,
                last_used_at: entry.get(7)?,
                source_app: entry.get(8)?,
                manual: entry.get(9)?,
                pinned: entry.get(10)?,
                deleted: entry.get(11)?,
            },
            payload,
        ))?;
    }
    Ok(())
}

pub fn import_history(
    path: &Path,
    apply: bool,
    entries: impl IntoIterator<Item = Result<ArchiveItem, AppError>>,
) -> Result<ImportReport, AppError> {
    if apply {
        return import_into(path, true, entries);
    }
    // Preview runs the actual transaction on a private copy. Neither preview
    // payloads nor uncommitted database pages enter the live library's WAL.
    let mut builder = tempfile::Builder::new();
    builder.prefix("uclip-history-preview-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let scratch = builder
        .tempdir()
        .map_err(|_| AppError::new(ErrorCode::Storage))?;
    let copy = scratch.path().join("foundation.sqlite3");
    snapshot(path, &copy)?;
    open(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE)?.pragma_update(None, "journal_mode", "WAL")?;
    import_into(&copy, false, entries)
}

fn import_into(
    path: &Path,
    apply: bool,
    entries: impl IntoIterator<Item = Result<ArchiveItem, AppError>>,
) -> Result<ImportReport, AppError> {
    let mut db = open(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.pragma_update(None, "synchronous", "FULL")?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_database(&tx)?;
    check_wal(&tx)?;
    if tx.query_row(
        "SELECT enrolled OR applying OR profile IS NOT NULL FROM replica_state WHERE singleton=1",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(AppError::new(ErrorCode::Restricted));
    }
    validate_payload_usage(&tx)?;
    validate_change_tracking(&tx)?;
    check_integrity(&tx)?;
    let policy = read_capture_policy(&tx)?;
    let exclusion = (!policy.exclude_pattern.is_empty())
        .then(|| pattern(&policy.exclude_pattern, false))
        .transpose()?;
    let initial_bytes = history_payload_bytes(&tx)?;
    let mut revision: i64 = tx.query_row(
        "SELECT revision FROM history_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut report = ImportReport::default();
    let mut tabs = BTreeSet::new();
    let mut saw_clip = false;
    for entry in entries {
        let (clip, payload) = match entry? {
            ArchiveItem::Collection(collection) if !saw_clip => {
                if insert_collection(&tx, collection, &mut revision)? {
                    report.collections += 1;
                } else {
                    report.existing_collections += 1;
                }
                continue;
            }
            ArchiveItem::Collection(_) => return Err(AppError::new(ErrorCode::InvalidInput)),
            ArchiveItem::Clip(clip, payload) => (clip, payload),
        };
        saw_clip = true;
        let id = clip.id.to_string();
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM clips WHERE id=?1 UNION ALL SELECT 1 FROM clip_changes WHERE id=?1)",
            [&id], |row| row.get::<_, bool>(0))? {
            report.existing_clips += 1;
            continue;
        }
        payload.validate()?;
        if exclusion
            .as_ref()
            .is_some_and(|regex| matches_capture_exclusion(&payload, regex))
        {
            report.excluded += 1;
            continue;
        }
        let collection = clip.collection.map(|uuid| uuid.to_string());
        if let Some(id) = collection.as_deref() {
            let deleted: Option<bool> = tx
                .query_row("SELECT deleted FROM collections WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            if !(deleted == Some(false) || clip.deleted && deleted == Some(true)) {
                report.unavailable_collections += 1;
                continue;
            }
        }
        let kind = insert_clip(
            &tx,
            &id,
            &clip,
            collection.as_deref(),
            &payload,
            &mut revision,
        )?;
        *report.clips.entry(kind).or_default() += 1;
        if clip.deleted {
            report.trash += 1;
        } else {
            report.pinned += u64::from(clip.pinned);
            tabs.insert(collection);
        }
        if history_payload_bytes(&tx)? > MAX_LOCAL_HISTORY_BYTES.max(initial_bytes) {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
    }
    for collection in tabs {
        ensure_tab_capacity(&tx, policy.max_items, collection.as_deref())?;
    }
    if report.trash > 0 && trash_count(&tx)? > MAX_TRASH_ITEMS {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    check_change_budget(&tx)?;
    report.payload_bytes = history_payload_bytes(&tx)? - initial_bytes;
    if apply && (report.collections > 0 || !report.clips.is_empty()) {
        report.backup = Some(backup(path)?);
        tx.commit()?;
    } else {
        tx.rollback()?;
    }
    Ok(report)
}

fn changed(tx: &Transaction<'_>, revision: &mut i64) -> Result<(), AppError> {
    *revision = revision
        .checked_add(1)
        .ok_or_else(|| AppError::new(ErrorCode::LimitExceeded))?;
    tx.execute(
        "UPDATE history_meta SET revision=?1 WHERE singleton=1",
        [*revision],
    )?;
    Ok(())
}

fn insert_collection(
    tx: &Transaction<'_>,
    collection: ArchiveCollection,
    revision: &mut i64,
) -> Result<bool, AppError> {
    let id = collection.id.to_string();
    if tx.query_row("SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 UNION ALL SELECT 1 FROM collection_changes WHERE id=?1)",
        [&id], |row| row.get::<_, bool>(0))? {
        return Ok(false);
    }
    let (name, key) = collection_name(&collection.name)?;
    if !collection.deleted {
        if tx.query_row(
            "SELECT count(*) FROM collections WHERE deleted=0",
            [],
            |row| row.get::<_, u32>(0),
        )? >= MAX_COLLECTIONS
        {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE deleted=0 AND name_key=?1)",
            [&key],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(AppError::new(ErrorCode::DuplicateName));
        }
    }
    tx.execute(
        "INSERT INTO collections(id,name,name_key,deleted,sequence,revision) VALUES(?1,?2,?3,?4,
         (SELECT revision+1 FROM history_meta WHERE singleton=1),0)",
        params![id, name, key, collection.deleted],
    )?;
    changed(tx, revision)?;
    Ok(true)
}

fn insert_clip(
    tx: &Transaction<'_>,
    id: &str,
    clip: &ArchiveClip,
    collection: Option<&str>,
    payload: &ClipPayload,
    revision: &mut i64,
) -> Result<&'static str, AppError> {
    use std::borrow::Cow;
    let timestamp_valid = |time| (0..=MAX_TIMESTAMP).contains(&time);
    if !timestamp_valid(clip.captured_at)
        || clip.last_used_at.is_some_and(|time| !timestamp_valid(time))
    {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    let source_app = clip.source_app.as_deref().filter(|name| {
        !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
    });
    let (kind, text, data, width, height, count) = match payload {
        ClipPayload::Text(text) => ("text", Cow::Borrowed(text.as_str()), &[][..], None, None, 0),
        ClipPayload::Files(paths) => (
            "files",
            Cow::Owned(
                serde_json::to_string(paths).map_err(|_| AppError::new(ErrorCode::InvalidInput))?,
            ),
            &[][..],
            None,
            None,
            paths.len() as u32,
        ),
        ClipPayload::Image {
            width,
            height,
            rgba,
        } => (
            "image",
            Cow::Borrowed(""),
            rgba.as_slice(),
            Some(*width),
            Some(*height),
            0,
        ),
    };
    tx.execute(
        "INSERT INTO clips(id,kind,text,data,width,height,file_count,fingerprint,origin,pinned,deleted,collection,
         captured_at,last_used_at,sequence,revision,source_app)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,
            (SELECT revision+1 FROM history_meta WHERE singleton=1),0,?15)",
        params![id, kind, text.as_ref(), data, width, height, count, payload.fingerprint(),
            if clip.manual { "manual" } else { "capture" }, clip.pinned, clip.deleted, collection,
            clip.captured_at, clip.last_used_at, source_app],
    )?;
    changed(tx, revision)?;
    Ok(kind)
}
