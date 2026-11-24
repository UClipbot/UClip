use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AppError, ClipPayload, ErrorCode, Storage};

pub(crate) const MAX_ENTITIES: u32 = 25_000;
const MAX_BYTES: i64 = 512 * 1024 * 1024;
const MAX_DEVICES: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ReplicaType {
    Collection = 0,
    Clip = 1,
}

impl ReplicaType {
    fn from_sql(kind: i64) -> rusqlite::Result<Self> {
        match kind {
            0 => Ok(Self::Collection),
            1 => Ok(Self::Clip),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplicaId {
    pub kind: ReplicaType,
    pub id: String,
}

/// Local SQLite revisions protect UI actions. These stamps travel between devices.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaStamp {
    pub id: ReplicaId,
    pub writer: String,
    pub counter: i64,
    pub clock: BTreeMap<String, i64>,
    pub removed: bool,
    /// Monotonic operation time in microseconds. Zero identifies pre-v18 stamps.
    #[serde(default)]
    pub changed_at: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTicket {
    pub device: String,
    pub token: String,
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplicaBody {
    Collection {
        name: String,
        deleted: bool,
        sequence: i64,
    },
    Clip {
        kind: String,
        text: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        width: Option<u32>,
        height: Option<u32>,
        manual: bool,
        pinned: bool,
        deleted: bool,
        collection: Option<String>,
        captured_at: i64,
        sequence: i64,
        source_app: Option<String>,
        file: Option<FileTicket>,
    },
}

#[derive(Serialize, Deserialize)]
pub struct ReplicaEntry {
    pub stamp: ReplicaStamp,
    pub body: Option<ReplicaBody>,
}

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<(), AppError> {
    tx.execute_batch(
        "CREATE TABLE replica_state(
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            device TEXT NOT NULL, counter INTEGER NOT NULL CHECK(counter>=0),
            enrolled INTEGER NOT NULL CHECK(enrolled IN (0,1)),
            applying INTEGER NOT NULL CHECK(applying IN (0,1)),
            profile TEXT CHECK(profile IS NULL OR length(CAST(profile AS BLOB))<=16384)
         ) STRICT;
         CREATE TABLE replica_entities(
            kind INTEGER NOT NULL CHECK(kind IN (0,1)), id TEXT NOT NULL,
            writer TEXT NOT NULL, counter INTEGER NOT NULL CHECK(counter>0),
            clock TEXT NOT NULL, removed INTEGER NOT NULL CHECK(removed IN (0,1)),
            PRIMARY KEY(kind,id)
         ) STRICT;
         CREATE TABLE replica_names(id TEXT PRIMARY KEY, name TEXT NOT NULL) STRICT;
         CREATE TABLE replica_files(id TEXT PRIMARY KEY, device TEXT NOT NULL, token TEXT NOT NULL) STRICT;
         CREATE INDEX replica_files_token ON replica_files(token);
         CREATE INDEX clips_replica_order ON clips(deleted,pinned DESC,captured_at DESC,id);
         CREATE INDEX clips_replica_quick_order ON clips(deleted,captured_at DESC,id);
         CREATE TABLE replica_grants(token TEXT PRIMARY KEY, paths TEXT NOT NULL) STRICT;
         CREATE TABLE replica_grant_refs(token TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(token,id),
            FOREIGN KEY(token) REFERENCES replica_grants(token) ON DELETE CASCADE) STRICT;",
    )?;
    tx.execute(
        "INSERT INTO replica_state VALUES(1,?1,0,0,0,NULL)",
        [Uuid::new_v4().to_string()],
    )?;
    Ok(())
}

pub(crate) fn validate_triggers(conn: &rusqlite::Connection) -> Result<(), AppError> {
    let count: u32 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN
         ('replica_clips_INSERT','replica_clips_UPDATE','replica_clips_DELETE',
          'replica_collections_INSERT','replica_collections_UPDATE','replica_collections_DELETE')",
        [],
        |row| row.get(0),
    )?;
    if count != 6 {
        return Err(storage_error());
    }
    Ok(())
}

pub(crate) fn migrate_frontier(tx: &Transaction<'_>) -> Result<(), AppError> {
    // Existing v16 stamps remain pending with their visible bodies. The old
    // projection cannot recover historical branch contexts that it discarded.
    tx.execute_batch(
        "CREATE TABLE replica_heads(
            kind INTEGER NOT NULL CHECK(kind IN (0,1)), id TEXT NOT NULL,
            key TEXT PRIMARY KEY, writer TEXT NOT NULL, counter INTEGER NOT NULL CHECK(counter>0),
            clock TEXT NOT NULL, removed INTEGER NOT NULL CHECK(removed IN (0,1)),
            body BLOB NOT NULL, payload_bytes INTEGER NOT NULL CHECK(payload_bytes>=0)
         ) STRICT;
         CREATE INDEX replica_heads_entity ON replica_heads(kind,id);
         CREATE TABLE replica_views(
            kind INTEGER NOT NULL CHECK(kind IN (0,1)), id TEXT NOT NULL,
            parent TEXT NOT NULL, head TEXT NOT NULL, PRIMARY KEY(kind,id)
         ) STRICT;
         CREATE INDEX replica_views_parent ON replica_views(kind,parent);",
    )?;
    Ok(())
}

const CHANGE_TIME: &str = "max(change_time+1,CAST(unixepoch('subsec')*1000000 AS INTEGER))";

pub(crate) fn migrate_latest(tx: &Transaction<'_>) -> Result<(), AppError> {
    tx.execute_batch(
        "ALTER TABLE replica_state ADD COLUMN change_time INTEGER NOT NULL DEFAULT 0 CHECK(change_time>=0);
         ALTER TABLE replica_entities ADD COLUMN changed_at INTEGER NOT NULL DEFAULT 0 CHECK(changed_at>=0);
         ALTER TABLE replica_heads ADD COLUMN changed_at INTEGER NOT NULL DEFAULT 0 CHECK(changed_at>=0);
         UPDATE replica_state SET applying=1 WHERE singleton=1;",
    )?;
    let discarded = {
        let mut stmt = tx.prepare("SELECT kind,id FROM replica_views WHERE id<>parent")?;
        let rows = stmt.query_map([], |r| {
            Ok(ReplicaId {
                kind: ReplicaType::from_sql(r.get(0)?)?,
                id: r.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for derived in &discarded {
        // A previously promoted or manually copied item has its own identity.
        if read_stamp(tx, derived)?.is_none() && read_current(tx, derived)?.is_none() {
            delete_projection(tx, derived)?;
        }
    }
    let competing = {
        let mut stmt = tx.prepare(
            "SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_heads
             WHERE (kind,id) IN (SELECT kind,id FROM replica_heads GROUP BY kind,id HAVING count(*)>1)
             ORDER BY kind,id",
        )?;
        let rows = stmt.query_map([], stamp_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for heads in competing.chunk_by(|a, b| a.id == b.id) {
        let winner = heads
            .iter()
            .max_by(|a, b| a.compare_version(b))
            .ok_or_else(storage_error)?;
        let projected = tx
            .query_row(
                "SELECT head FROM replica_views WHERE kind=?1 AND id=?2 AND id=parent",
                params![winner.id.kind as i64, winner.id.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if projected.as_deref() != Some(winner.key().as_str()) {
            let entry = read_head(tx, winner.clone())?.ok_or_else(storage_error)?;
            project_entry(tx, &entry, true)?;
        }
        tx.execute(
            "DELETE FROM replica_heads WHERE kind=?1 AND id=?2 AND key<>?3",
            params![winner.id.kind as i64, winner.id.id, winner.key()],
        )?;
    }
    if !discarded.is_empty() || !competing.is_empty() {
        normalize_names(tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
    }
    install_latest_triggers(tx, false)?;
    tx.execute_batch(
        "DROP TABLE replica_views;
         DROP INDEX replica_heads_entity;
         CREATE UNIQUE INDEX replica_heads_entity ON replica_heads(kind,id);
         UPDATE replica_state SET applying=0 WHERE singleton=1;",
    )?;
    Ok(())
}

pub(crate) fn migrate_local_history(tx: &Transaction<'_>) -> Result<(), AppError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS replica_local_clips(
            id TEXT PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE
         ) STRICT;",
    )?;
    install_latest_triggers(tx, true)
}

fn install_latest_triggers(tx: &Transaction<'_>, local_history: bool) -> Result<(), AppError> {
    for (table, kind) in [("collections", 0), ("clips", 1)] {
        for (action, index, removed, changes) in [
            ("INSERT", "NEW", 0, ""),
            (
                "UPDATE",
                "NEW",
                0,
                if kind == 0 {
                    "AND (NEW.name IS NOT OLD.name OR NEW.deleted IS NOT OLD.deleted OR NEW.sequence IS NOT OLD.sequence)"
                } else {
                    "AND (NEW.kind IS NOT OLD.kind OR NEW.fingerprint IS NOT OLD.fingerprint OR
                      NEW.pinned IS NOT OLD.pinned OR NEW.deleted IS NOT OLD.deleted OR
                      NEW.collection IS NOT OLD.collection OR NEW.captured_at IS NOT OLD.captured_at OR
                      NEW.sequence IS NOT OLD.sequence OR NEW.source_app IS NOT OLD.source_app)"
                },
            ),
            ("DELETE", "OLD", 1, ""),
        ] {
            let auxiliary = match (kind, action) {
                (0, "INSERT") => "INSERT INTO replica_names VALUES(NEW.id,NEW.name);",
                (0, "UPDATE") => "UPDATE replica_names SET name=NEW.name WHERE id=NEW.id AND NEW.name IS NOT OLD.name;",
                (0, "DELETE") => "DELETE FROM replica_names WHERE id=OLD.id;",
                (1, "UPDATE") => "DELETE FROM replica_files WHERE id=NEW.id AND NEW.kind='files' AND NEW.text IS NOT OLD.text; DELETE FROM replica_grant_refs WHERE id=NEW.id AND NEW.kind='files' AND NEW.text IS NOT OLD.text;",
                (1, "DELETE") => "DELETE FROM replica_files WHERE id=OLD.id; DELETE FROM replica_grant_refs WHERE id=OLD.id;",
                _ => "",
            };
            let (timing, shared_clip) = if local_history && kind == 1 {
                // Check the local-only marker before the foreign key cascade removes it.
                let timing = if action == "DELETE" {
                    "BEFORE"
                } else {
                    "AFTER"
                };
                (
                    timing,
                    format!(
                        "AND NOT EXISTS (SELECT 1 FROM replica_local_clips WHERE id={index}.id)"
                    ),
                )
            } else {
                ("AFTER", String::new())
            };
            tx.execute_batch(&format!(
                "DROP TRIGGER IF EXISTS replica_{table}_{action};
                 CREATE TRIGGER replica_{table}_{action} {timing} {action} ON {table}
                 WHEN (SELECT enrolled=1 AND applying=0 FROM replica_state WHERE singleton=1) {changes} {shared_clip}
                 BEGIN
                    SELECT CASE WHEN EXISTS(
                       SELECT 1 FROM replica_state WHERE singleton=1 AND
                       (counter>=9223372036854775806 OR change_time>=9223372036854775806)
                    ) THEN RAISE(ABORT,'uclip_replica_clock_limit') END;
                    UPDATE replica_state SET counter=counter+1,change_time={CHANGE_TIME} WHERE singleton=1;
                    INSERT INTO replica_entities(kind,id,writer,counter,clock,removed,changed_at)
                    SELECT {kind},{index}.id,s.device,s.counter,
                        json_set(coalesce(
                            (SELECT clock FROM replica_entities p WHERE p.kind={kind} AND p.id={index}.id),
                            (SELECT clock FROM replica_heads h WHERE h.kind={kind} AND h.id={index}.id),
                            '{{}}'
                        ), '$.\"' || s.device || '\"', s.counter),{removed},s.change_time
                    FROM replica_state s WHERE s.singleton=1
                    ON CONFLICT(kind,id) DO UPDATE SET writer=excluded.writer,counter=excluded.counter,
                        clock=excluded.clock,removed=excluded.removed,changed_at=excluded.changed_at;
                    SELECT CASE WHEN (SELECT count(*) FROM replica_entities p,json_each(p.clock)
                        WHERE p.kind={kind} AND p.id={index}.id)>{MAX_DEVICES}
                        THEN RAISE(ABORT,'uclip_replica_clock_limit') END;
                    DELETE FROM replica_heads WHERE kind={kind} AND id={index}.id;
                    {auxiliary}
                 END;"
            ))?;
        }
    }
    Ok(())
}

pub(crate) fn migrate_legacy_grants(tx: &Transaction<'_>) -> Result<(), AppError> {
    let mut query = tx.prepare(
        "SELECT f.id,p.writer,p.counter,g.token,r.id FROM replica_files f
         JOIN replica_state s ON s.device=f.device
         JOIN clips c ON c.id=f.id
         JOIN replica_entities p ON p.kind=1 AND p.id=f.id
         JOIN replica_grants g ON g.token=f.token AND g.paths=c.text
         JOIN replica_grant_refs r ON r.token=g.token
         WHERE NOT EXISTS(SELECT 1 FROM replica_grant_refs x WHERE x.token=f.token AND x.id=f.id)",
    )?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, writer, counter, token, origin) in rows {
        let legacy = ReplicaStamp {
            id: ReplicaId {
                kind: ReplicaType::Clip,
                id: origin,
            },
            writer,
            counter,
            clock: BTreeMap::new(),
            removed: false,
            changed_at: 0,
        };
        if legacy.conflict_id().id == id {
            tx.execute(
                "INSERT OR IGNORE INTO replica_grant_refs VALUES(?1,?2)",
                params![token, id],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn enrolled(tx: &rusqlite::Connection) -> Result<bool, AppError> {
    Ok(tx.query_row(
        "SELECT enrolled FROM replica_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}

pub(crate) fn check_capacity(tx: &Transaction<'_>) -> Result<(), AppError> {
    let over_limit: bool = tx.query_row(
        "SELECT (SELECT count(*) FROM replica_entities) + (SELECT count(*) FROM replica_heads)>?1 OR
          (SELECT coalesce(sum(length(CAST(text AS BLOB))+length(data)),0) FROM clips c
             WHERE NOT EXISTS (SELECT 1 FROM replica_local_clips l WHERE l.id=c.id))>?2 OR
          (SELECT coalesce(sum(payload_bytes),0) FROM replica_heads) +
          (SELECT coalesce(sum(length(CAST(c.text AS BLOB))+length(c.data)),0)
             FROM replica_entities p JOIN clips c USING(id) WHERE p.kind=1 AND p.removed=0)>?2",
        params![MAX_ENTITIES, MAX_BYTES],
        |r| r.get(0),
    )?;
    if over_limit {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(())
}

pub(crate) fn copy_file_reference(
    tx: &Transaction<'_>,
    source: &str,
    destination: &str,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO replica_files SELECT ?1,device,token FROM replica_files WHERE id=?2",
        params![destination, source],
    )?;
    tx.execute(
        "INSERT INTO replica_grant_refs SELECT token,?1 FROM replica_grant_refs WHERE id=?2",
        params![destination, source],
    )?;
    Ok(())
}

pub(crate) fn remote_file(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<FileTicket>, AppError> {
    Ok(conn
        .query_row(
            "SELECT f.device,f.token FROM replica_files f, replica_state s
         WHERE f.id=?1 AND f.device<>s.device AND s.singleton=1",
            [id],
            |r| {
                Ok(FileTicket {
                    device: r.get(0)?,
                    token: r.get(1)?,
                })
            },
        )
        .optional()?)
}

/// Staged by the native adapter. This type never reaches IPC or the sync protocol.
pub struct FileReplacement {
    pub id: String,
    pub ticket: FileTicket,
    pub paths: Vec<String>,
}

pub(crate) fn verify_local_file(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    let mismatch = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM replica_files f JOIN clips c USING(id)
         WHERE f.id=?1 AND NOT EXISTS(SELECT 1 FROM replica_grants g WHERE g.token=f.token AND g.paths=c.text))",
        [id], |row| row.get::<_, bool>(0),
    )?;
    if mismatch {
        return Err(AppError::new(ErrorCode::MissingFile));
    }
    Ok(())
}

pub(crate) fn portable_paths(paths: &[String]) -> bool {
    !paths.is_empty()
        && paths.len() <= 256
        && paths.iter().all(|path| {
            let bytes = path.as_bytes();
            bytes.len() <= 32768
                && !path.contains('\0')
                && (path.starts_with('/')
                    || path.starts_with("\\\\")
                    || (bytes.len() >= 3
                        && bytes[0].is_ascii_alphabetic()
                        && bytes[1] == b':'
                        && matches!(bytes[2], b'/' | b'\\')))
        })
}

fn invalid() -> AppError {
    AppError::new(ErrorCode::InvalidInput)
}

fn storage_error() -> AppError {
    AppError::new(ErrorCode::Storage)
}

fn canonical_uuid(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
}

impl ReplicaStamp {
    pub fn validate(&self) -> Result<(), AppError> {
        if !(0..i64::MAX).contains(&self.changed_at)
            || !canonical_uuid(&self.id.id)
            || self.clock.len() > MAX_DEVICES
            || !self
                .clock
                .iter()
                .all(|(id, counter)| canonical_uuid(id) && (1..i64::MAX).contains(counter))
            || self.counter <= 0
            || self
                .clock
                .get(&self.writer)
                .is_none_or(|value| self.counter > *value)
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn key(&self) -> String {
        let mut hash = Sha256::new();
        hash.update([self.id.kind as u8]);
        hash.update(self.id.id.as_bytes());
        hash.update(self.writer.as_bytes());
        hash.update(self.counter.to_be_bytes());
        hash.update([u8::from(self.removed)]);
        for (device, counter) in &self.clock {
            hash.update(device.as_bytes());
            hash.update(counter.to_be_bytes());
        }
        if self.changed_at != 0 {
            hash.update(self.changed_at.to_be_bytes());
        }
        format!("{:x}", hash.finalize())
    }

    pub fn compare_version(&self, other: &Self) -> Ordering {
        // A clip identity cannot be restored after a permanent deletion.
        // Trash entries still carry a body and use the regular version order.
        let permanent_removal = |stamp: &Self| stamp.id.kind == ReplicaType::Clip && stamp.removed;
        // Timestamps advance past observed versions before committing any local write.
        // Legacy stamps lack timestamps. A strict causal successor always has a larger
        // vector sum, so legacy imports also have a consistent total order.
        let weight = |stamp: &Self| {
            stamp
                .clock
                .values()
                .map(|counter| i128::from(*counter))
                .sum::<i128>()
        };
        permanent_removal(self)
            .cmp(&permanent_removal(other))
            .then_with(|| self.changed_at.cmp(&other.changed_at))
            .then_with(|| weight(self).cmp(&weight(other)))
            .then_with(|| {
                (&self.writer, self.counter, self.removed, &self.clock).cmp(&(
                    &other.writer,
                    other.counter,
                    other.removed,
                    &other.clock,
                ))
            })
    }

    pub fn supersedes(&self, other: &Self) -> bool {
        self.id == other.id && !self.compare_version(other).is_lt()
    }

    fn conflict_id(&self) -> ReplicaId {
        let mut hash = Sha256::new();
        hash.update(b"uclip/replica/conflict/v1");
        hash.update([self.id.kind as u8]);
        hash.update(self.id.id.as_bytes());
        hash.update(self.writer.as_bytes());
        hash.update(self.counter.to_be_bytes());
        let output = hash.finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&output[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80; // UUIDv8 namespace.
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        ReplicaId {
            kind: self.id.kind,
            id: Uuid::from_bytes(bytes).to_string(),
        }
    }
}

impl ReplicaEntry {
    fn validate(&self) -> Result<(), AppError> {
        self.stamp.validate()?;
        if self.stamp.removed != self.body.is_none() {
            return Err(invalid());
        }
        match (&self.stamp.id.kind, &self.body) {
            (_, None) => Ok(()),
            (ReplicaType::Collection, Some(ReplicaBody::Collection { name, sequence, .. })) => {
                crate::collections::collection_name(name)?;
                if name.trim() != name || !(0..i64::MAX).contains(sequence) {
                    return Err(invalid());
                }
                Ok(())
            }
            (
                ReplicaType::Clip,
                Some(ReplicaBody::Clip {
                    kind,
                    text,
                    data,
                    width,
                    height,
                    collection,
                    captured_at,
                    sequence,
                    source_app,
                    file,
                    ..
                }),
            ) => {
                if collection.as_deref().is_some_and(|id| !canonical_uuid(id))
                    || !(*captured_at >= 0 && (0..i64::MAX).contains(sequence))
                    || source_app.as_ref().is_some_and(|name| {
                        name.trim().is_empty()
                            || name.len() > 256
                            || name.chars().any(char::is_control)
                    })
                {
                    return Err(invalid());
                }
                match (kind.as_str(), width, height, file) {
                    ("text", None, None, None)
                        if data.is_empty()
                            && !text.is_empty()
                            && text.len() <= crate::MAX_TEXT_BYTES =>
                    {
                        Ok(())
                    }
                    ("image", Some(width), Some(height), None) if text.is_empty() => {
                        let pixels = u64::from(*width) * u64::from(*height);
                        if pixels > 0
                            && pixels <= crate::MAX_IMAGE_PIXELS
                            && pixels * 4 == data.len() as u64
                        {
                            return Ok(());
                        }
                        Err(invalid())
                    }
                    ("files", None, None, Some(ticket))
                        if data.is_empty()
                            && canonical_uuid(&ticket.device)
                            && canonical_uuid(&ticket.token)
                            && text.len() <= 2 + 256 * (6 * 32768 + 3) =>
                    {
                        let paths: Vec<String> =
                            serde_json::from_str(text).map_err(|_| invalid())?;
                        if portable_paths(&paths) {
                            return Ok(());
                        }
                        Err(invalid())
                    }
                    _ => Err(invalid()),
                }
            }
            _ => Err(invalid()),
        }
    }
}

fn stamp_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReplicaStamp> {
    let kind = ReplicaType::from_sql(row.get(0)?)?;
    let clock: String = row.get(4)?;
    Ok(ReplicaStamp {
        id: ReplicaId {
            kind,
            id: row.get(1)?,
        },
        writer: row.get(2)?,
        counter: row.get(3)?,
        clock: serde_json::from_str(&clock).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        removed: row.get(5)?,
        changed_at: row.get(6)?,
    })
}

fn read_stamp(
    conn: &rusqlite::Connection,
    id: &ReplicaId,
) -> Result<Option<ReplicaStamp>, AppError> {
    Ok(conn.query_row(
        "SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_entities WHERE kind=?1 AND id=?2",
        params![id.kind as i64, id.id], stamp_from_row,
    ).optional()?)
}

fn file_ticket(conn: &rusqlite::Connection, id: &str, paths: &str) -> Result<FileTicket, AppError> {
    if let Some(ticket) = conn
        .query_row(
            "SELECT device,token FROM replica_files WHERE id=?1",
            [id],
            |r| {
                Ok(FileTicket {
                    device: r.get(0)?,
                    token: r.get(1)?,
                })
            },
        )
        .optional()?
    {
        return Ok(ticket);
    }
    let device = conn.query_row(
        "SELECT device FROM replica_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let token = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO replica_grants VALUES(?1,?2)",
        params![token, paths],
    )?;
    conn.execute(
        "INSERT INTO replica_files VALUES(?1,?2,?3)",
        params![id, device, token],
    )?;
    conn.execute(
        "INSERT INTO replica_grant_refs VALUES(?1,?2)",
        params![token, id],
    )?;
    Ok(FileTicket { device, token })
}

fn entry_for_stamp(
    conn: &rusqlite::Connection,
    stamp: ReplicaStamp,
) -> Result<ReplicaEntry, AppError> {
    let body = if stamp.removed {
        None
    } else {
        Some(match stamp.id.kind {
            ReplicaType::Collection => conn.query_row(
                "SELECT n.name,c.deleted,c.sequence FROM collections c JOIN replica_names n USING(id) WHERE c.id=?1",
                [&stamp.id.id], |r| Ok(ReplicaBody::Collection {
                    name: r.get(0)?, deleted: r.get(1)?, sequence: r.get(2)?,
                }),
            )?,
            ReplicaType::Clip => {
                let mut body = conn.query_row(
                    "SELECT kind,text,data,width,height,origin='manual',pinned,deleted,collection,captured_at,sequence,source_app
                     FROM clips WHERE id=?1",
                    [&stamp.id.id], |r| Ok(ReplicaBody::Clip {
                        kind: r.get(0)?, text: r.get(1)?, data: r.get(2)?, width: r.get(3)?, height: r.get(4)?,
                        manual: r.get(5)?, pinned: r.get(6)?, deleted: r.get(7)?, collection: r.get(8)?,
                        captured_at: r.get(9)?, sequence: r.get(10)?, source_app: r.get(11)?, file: None,
                    }),
                )?;
                if let ReplicaBody::Clip { kind, text, file, .. } = &mut body {
                    if kind == "files" {
                        *file = Some(file_ticket(conn, &stamp.id.id, text)?);
                    }
                }
                body
            }
        })
    };
    Ok(ReplicaEntry { stamp, body })
}

fn save_head(tx: &Transaction<'_>, entry: &ReplicaEntry) -> Result<(), AppError> {
    let stamp = &entry.stamp;
    let clock = serde_json::to_string(&stamp.clock).map_err(|_| storage_error())?;
    let payload_bytes = match &entry.body {
        Some(ReplicaBody::Clip { text, data, .. }) => (text.len() + data.len()) as i64,
        _ => 0,
    };
    let body = rmp_serde::to_vec(&entry.body).map_err(|_| storage_error())?;
    tx.execute(
        "INSERT INTO replica_heads(kind,id,key,writer,counter,clock,removed,body,payload_bytes,changed_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            stamp.id.kind as i64,
            stamp.id.id,
            stamp.key(),
            stamp.writer,
            stamp.counter,
            clock,
            stamp.removed,
            body,
            payload_bytes,
            stamp.changed_at,
        ],
    )?;
    Ok(())
}

fn read_current(
    conn: &rusqlite::Connection,
    id: &ReplicaId,
) -> Result<Option<ReplicaStamp>, AppError> {
    Ok(conn.query_row(
        "SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_heads WHERE kind=?1 AND id=?2",
        params![id.kind as i64,id.id], stamp_from_row,
    ).optional()?)
}

fn read_head(
    conn: &rusqlite::Connection,
    stamp: ReplicaStamp,
) -> Result<Option<ReplicaEntry>, AppError> {
    let body: Option<Vec<u8>> = conn
        .query_row(
            "SELECT body FROM replica_heads WHERE key=?1",
            [stamp.key()],
            |row| row.get(0),
        )
        .optional()?;
    body.map(|bytes| {
        let body = rmp_serde::from_slice(&bytes).map_err(|_| storage_error())?;
        Ok(ReplicaEntry { stamp, body })
    })
    .transpose()
}

/// The trigger commits the local version with the row. Materialize its
/// immutable body before exporting it.
fn flush_pending(tx: &Transaction<'_>, stamp: ReplicaStamp) -> Result<ReplicaEntry, AppError> {
    let entry = entry_for_stamp(tx, stamp)?;
    entry.validate()?;
    save_head(tx, &entry)?;
    let id = &entry.stamp.id;
    tx.execute(
        "DELETE FROM replica_entities WHERE kind=?1 AND id=?2",
        params![id.kind as i64, id.id],
    )?;
    Ok(entry)
}

fn delete_projection(tx: &Transaction<'_>, id: &ReplicaId) -> Result<(), AppError> {
    match id.kind {
        ReplicaType::Collection => {
            tx.execute("DELETE FROM collections WHERE id=?1", [&id.id])?;
            tx.execute("DELETE FROM replica_names WHERE id=?1", [&id.id])?;
        }
        ReplicaType::Clip => {
            tx.execute("DELETE FROM clips WHERE id=?1", [&id.id])?;
            tx.execute("DELETE FROM replica_files WHERE id=?1", [&id.id])?;
            tx.execute("DELETE FROM replica_grant_refs WHERE id=?1", [&id.id])?;
        }
    }
    Ok(())
}

fn detach_local_clips(tx: &Transaction<'_>, collection: Option<&str>) -> Result<usize, AppError> {
    Ok(tx.execute(
        "UPDATE clips SET collection=NULL, revision=revision+1
         WHERE deleted=0 AND collection IS NOT NULL AND (?1 IS NULL OR collection=?1)
           AND id IN (SELECT id FROM replica_local_clips)
           AND NOT EXISTS (SELECT 1 FROM collections WHERE id=clips.collection AND deleted=0)",
        [collection],
    )?)
}

pub(crate) fn repair_local_collection_membership(tx: &Transaction<'_>) -> Result<(), AppError> {
    if detach_local_clips(tx, None)? > 0 {
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
    }
    Ok(())
}

fn project_entry(
    tx: &Transaction<'_>,
    entry: &ReplicaEntry,
    previously_projected: bool,
) -> Result<(), AppError> {
    let id = &entry.stamp.id.id;
    let revision = if previously_projected {
        tx.query_row(
            "SELECT revision+1 FROM history_meta WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )?
    } else {
        0
    };
    match &entry.body {
        None => delete_projection(tx, &entry.stamp.id)?,
        Some(ReplicaBody::Collection {
            name,
            deleted,
            sequence,
        }) => {
            tx.execute(
                "INSERT INTO replica_names VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET name=excluded.name",
                params![id, name],
            )?;
            tx.execute(
                "INSERT INTO collections(id,name,name_key,deleted,sequence,revision)
                 VALUES(?1,?2,char(1)||?1,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET deleted=excluded.deleted,sequence=excluded.sequence,
                     name_key=excluded.name_key,revision=collections.revision+1",
                params![id, name, deleted, sequence, revision],
            )?;
        }
        Some(ReplicaBody::Clip {
            kind,
            text,
            data,
            width,
            height,
            manual,
            pinned,
            deleted,
            collection,
            captured_at,
            sequence,
            source_app,
            file,
        }) => {
            let (fingerprint, count) = match kind.as_str() {
                "text" => (ClipPayload::Text(text.clone()).fingerprint(), 0),
                "files" => {
                    let paths: Vec<String> = serde_json::from_str(text).map_err(|_| invalid())?;
                    let count = paths.len() as u32;
                    (ClipPayload::Files(paths).fingerprint(), count)
                }
                "image" => {
                    let mut hash = Sha256::new();
                    hash.update(b"image\0");
                    hash.update(width.ok_or_else(invalid)?.to_le_bytes());
                    hash.update(height.ok_or_else(invalid)?.to_le_bytes());
                    hash.update(data);
                    (hash.finalize().to_vec(), 0)
                }
                _ => return Err(invalid()),
            };
            tx.execute(
                "INSERT INTO clips(id,kind,text,data,width,height,file_count,fingerprint,origin,pinned,
                        deleted,collection,captured_at,sequence,revision,source_app)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?16,?15)
                 ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,text=excluded.text,data=excluded.data,
                   width=excluded.width,height=excluded.height,file_count=excluded.file_count,
                   fingerprint=excluded.fingerprint,origin=excluded.origin,pinned=excluded.pinned,
                   deleted=excluded.deleted,collection=excluded.collection,captured_at=excluded.captured_at,
                   sequence=excluded.sequence,source_app=excluded.source_app,revision=clips.revision+1",
                params![id,kind,text,data,width,height,count,fingerprint,if *manual { "manual" } else { "capture" },
                    pinned,deleted,collection,captured_at,sequence,source_app,revision],
            )?;
            tx.execute("DELETE FROM replica_files WHERE id=?1", [id])?;
            if let Some(file) = file {
                // A remote entry can reference an existing grant, but cannot create or change one.
                tx.execute(
                    "INSERT INTO replica_files VALUES(?1,?2,?3)",
                    params![id, file.device, file.token],
                )?;
            }
        }
    }
    Ok(())
}

fn normalize_names(conn: &rusqlite::Connection) -> Result<bool, AppError> {
    let mut statement = conn.prepare(
        "SELECT c.id,c.name,c.name_key,n.name FROM collections c JOIN replica_names n USING(id)
         WHERE c.deleted=0 ORDER BY c.id",
    )?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let reserved = rows
        .iter()
        .map(|row| row.3.to_lowercase())
        .collect::<HashSet<_>>();
    let mut used = HashSet::new();
    let mut updates = Vec::new();
    for (id, display, key, name) in rows {
        let mut next = name.clone();
        let mut next_key = next.to_lowercase();
        if !used.insert(next_key.clone()) {
            let base = name.chars().take(40).collect::<String>();
            for attempt in 0..=MAX_ENTITIES {
                let suffix = if attempt == 0 {
                    id[..8].to_string()
                } else {
                    format!("{}-{attempt}", &id[..8])
                };
                next = format!("{base} ({suffix})");
                next_key = next.to_lowercase();
                if !reserved.contains(&next_key) && used.insert(next_key.clone()) {
                    break;
                }
            }
        }
        if display != next || key != next_key {
            updates.push((id, next, next_key));
        }
    }
    for (id, _, _) in &updates {
        conn.execute(
            "UPDATE collections SET name_key=char(1)||id WHERE id=?1",
            [id],
        )?;
    }
    for (id, name, key) in &updates {
        conn.execute(
            "UPDATE collections SET name=?1,name_key=?2,revision=revision+1 WHERE id=?3",
            params![name, key, id],
        )?;
    }
    Ok(!updates.is_empty())
}

fn enroll(tx: &Transaction<'_>, from_now: bool) -> Result<(), AppError> {
    if !enrolled(tx)? {
        if from_now {
            tx.execute("INSERT INTO replica_local_clips SELECT id FROM clips", [])?;
        }
        tx.execute_batch(
            &format!("UPDATE replica_state SET counter=1,enrolled=1,change_time={CHANGE_TIME} WHERE singleton=1;
             INSERT INTO replica_names SELECT id,name FROM collections;
             INSERT INTO replica_entities
                SELECT 0,c.id,s.device,1,json_object(s.device,1),0,s.change_time FROM collections c, replica_state s
                UNION ALL
                SELECT 1,c.id,s.device,1,json_object(s.device,1),0,s.change_time FROM clips c, replica_state s
                WHERE NOT EXISTS (SELECT 1 FROM replica_local_clips l WHERE l.id=c.id);
             UPDATE history_meta SET revision=revision+1 WHERE singleton=1;"),
        )?;
        check_capacity(tx)?;
    } else if from_now {
        return Err(AppError::new(ErrorCode::Conflict));
    }
    Ok(())
}

impl Storage {
    pub(crate) fn validate_replica_schema(&self) -> Result<(), AppError> {
        validate_triggers(&self.connection)?;
        let valid = self.connection.query_row(
            "SELECT device FROM replica_state WHERE singleton=1 AND applying=0",
            [],
            |r| r.get::<_, String>(0),
        )?;
        if !canonical_uuid(&valid) {
            return Err(storage_error());
        }
        self.connection.prepare(
            "SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_entities LIMIT 0",
        )?;
        self.connection
            .prepare("SELECT id,device,token FROM replica_files LIMIT 0")?;
        self.connection
            .prepare("SELECT id FROM replica_local_clips LIMIT 0")?;
        self.connection
            .prepare("SELECT token,paths FROM replica_grants LIMIT 0")?;
        self.connection
            .prepare("SELECT id,name FROM replica_names LIMIT 0")?;
        self.connection
            .prepare("SELECT profile FROM replica_state LIMIT 0")?;
        self.connection
            .prepare("SELECT token,id FROM replica_grant_refs LIMIT 0")?;
        self.connection.prepare(
            "SELECT kind,id,key,writer,counter,clock,removed,body,payload_bytes,changed_at FROM replica_heads LIMIT 0",
        )?;
        self.connection
            .prepare("SELECT change_time FROM replica_state LIMIT 0")?;
        Ok(())
    }

    pub fn replica_device(&self) -> Result<String, AppError> {
        Ok(self.connection.query_row(
            "SELECT device FROM replica_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn enroll_replica(&mut self) -> Result<(), AppError> {
        let tx = self.connection.transaction()?;
        enroll(&tx, false)?;
        tx.commit()?;
        Ok(())
    }

    pub fn replica_profile(&self) -> Result<Option<String>, AppError> {
        Ok(self.connection.query_row(
            "SELECT profile FROM replica_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn save_replica_profile(
        &mut self,
        profile: String,
        from_now: bool,
    ) -> Result<(), AppError> {
        if profile.len() > 16384 {
            return Err(invalid());
        }
        let tx = self.connection.transaction()?;
        enroll(&tx, from_now)?;
        tx.execute(
            "UPDATE replica_state SET profile=?1 WHERE singleton=1",
            [profile],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn replica_inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
        let tx = self.connection.transaction()?;
        tx.execute("UPDATE replica_state SET applying=1 WHERE singleton=1", [])?;
        if normalize_names(&tx)? {
            tx.execute(
                "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        tx.execute("UPDATE replica_state SET applying=0 WHERE singleton=1", [])?;
        tx.execute(
            "DELETE FROM replica_grants WHERE NOT EXISTS(
                SELECT 1 FROM replica_grant_refs r WHERE r.token=replica_grants.token
            )",
            [],
        )?;
        let entries = {
            let mut statement = tx.prepare(
                "SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_entities
                 UNION ALL SELECT kind,id,writer,counter,clock,removed,changed_at FROM replica_heads
                 ORDER BY kind,id,writer,counter",
            )?;
            let rows = statement.query_map([], stamp_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        Ok(entries)
    }

    pub fn replica_entry(
        &mut self,
        stamp: &ReplicaStamp,
    ) -> Result<Option<ReplicaEntry>, AppError> {
        // Other processes may edit the library while the sync worker exports an entry.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry = if read_stamp(&tx, &stamp.id)?.as_ref() == Some(stamp) {
            Some(flush_pending(&tx, stamp.clone())?)
        } else {
            read_head(&tx, stamp.clone())?
        };
        tx.commit()?;
        Ok(entry)
    }

    pub fn apply_replica_entry(&mut self, incoming: ReplicaEntry) -> Result<bool, AppError> {
        incoming.validate()?;
        let tx = self.connection.transaction()?;
        if !enrolled(&tx)? {
            return Err(AppError::new(ErrorCode::Unavailable));
        }
        let id = &incoming.stamp.id;
        if id.kind == ReplicaType::Clip
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM replica_local_clips WHERE id=?1)",
                [&id.id],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Ok(false);
        }
        let pending = read_stamp(&tx, id)?;
        let head = read_current(&tx, id)?;
        let current = pending.as_ref().or(head.as_ref());
        if current.is_some_and(|stamp| stamp.supersedes(&incoming.stamp)) {
            return Ok(false);
        }
        tx.execute(
            "DELETE FROM replica_entities WHERE kind=?1 AND id=?2",
            params![id.kind as i64, id.id],
        )?;
        tx.execute(
            "DELETE FROM replica_heads WHERE kind=?1 AND id=?2",
            params![id.kind as i64, id.id],
        )?;
        save_head(&tx, &incoming)?;
        tx.execute("UPDATE replica_state SET applying=1 WHERE singleton=1", [])?;
        project_entry(&tx, &incoming, current.is_some())?;
        if id.kind == ReplicaType::Collection {
            if incoming
                .body
                .as_ref()
                .is_none_or(|body| matches!(body, ReplicaBody::Collection { deleted: true, .. }))
            {
                // Unshared clips stay in active local history when their shared collection is removed.
                detach_local_clips(&tx, Some(&id.id))?;
            }
            normalize_names(&tx)?;
        }
        check_capacity(&tx)?;
        let device: String = tx.query_row(
            "SELECT device FROM replica_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let local_counter = incoming
            .stamp
            .clock
            .get(&device)
            .copied()
            .unwrap_or_default();
        tx.execute(
            "UPDATE replica_state SET counter=max(counter,?1),change_time=max(change_time,?2),
             applying=0 WHERE singleton=1",
            params![local_counter, incoming.stamp.changed_at],
        )?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn remote_file_ticket(&self, id: &str) -> Result<Option<FileTicket>, AppError> {
        remote_file(&self.connection, id)
    }

    pub fn replica_grant(&self, token: &str) -> Result<Vec<String>, AppError> {
        if !canonical_uuid(token) {
            return Err(invalid());
        }
        let paths: String = self
            .connection
            .query_row(
                "SELECT g.paths FROM replica_grants g
             WHERE g.token=?1 AND EXISTS(
                SELECT 1 FROM replica_grant_refs r JOIN replica_files f USING(token,id)
                    JOIN clips c ON c.id=f.id JOIN replica_state s ON s.device=f.device
                WHERE r.token=g.token AND c.deleted=0 AND c.kind='files' AND c.text=g.paths
             )",
                [token],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| AppError::new(ErrorCode::MissingFile))?;
        let paths: Vec<String> = serde_json::from_str(&paths).map_err(|_| storage_error())?;
        ClipPayload::Files(paths.clone()).validate()?;
        Ok(paths)
    }
}
