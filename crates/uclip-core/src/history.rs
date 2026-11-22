use crate::{
    AppError, CapturePolicy, ClipKind, ErrorCode, HistoryItem, HistoryPage, HistoryQuery, Storage,
    UpdateCapturePolicy,
};
use regex::{Regex, RegexBuilder};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct QuickPasteSlot {
    pub id: String,
    pub revision: String,
    pub item: crate::QuickPasteItem,
}

pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 8_388_608;
pub const MAX_HISTORY_BYTES: i64 = 128 * 1024 * 1024;
pub const PAGE_SIZE: usize = 25;

/// Only Rust platform adapters construct capture payloads. There is no arbitrary-payload IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipPayload {
    Text(String),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Files(Vec<String>),
}
impl ClipPayload {
    pub fn validate(&self) -> Result<(), AppError> {
        let valid = match self {
            Self::Text(text) => !text.is_empty() && text.len() <= MAX_TEXT_BYTES,
            Self::Image {
                width,
                height,
                rgba,
            } => {
                let pixels = u64::from(*width) * u64::from(*height);
                pixels > 0 && pixels <= MAX_IMAGE_PIXELS && pixels * 4 == rgba.len() as u64
            }
            Self::Files(paths) => {
                !paths.is_empty()
                    && paths.len() <= 256
                    && paths.iter().all(|path| {
                        !path.contains('\0') && path.len() <= 32768 && Path::new(path).is_absolute()
                    })
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AppError::new(ErrorCode::LimitExceeded))
        }
    }
    pub fn fingerprint(&self) -> Vec<u8> {
        let mut hash = Sha256::new();
        match self {
            Self::Text(text) => {
                hash.update(b"text\0");
                hash.update(text.as_bytes());
            }
            Self::Image {
                width,
                height,
                rgba,
            } => {
                hash.update(b"image\0");
                hash.update(width.to_le_bytes());
                hash.update(height.to_le_bytes());
                hash.update(rgba);
            }
            Self::Files(files) => {
                hash.update(b"files\0");
                for path in files {
                    hash.update((path.len() as u64).to_le_bytes());
                    hash.update(path.as_bytes());
                }
            }
        }
        hash.finalize().to_vec()
    }
}

pub fn pattern(value: &str, literal: bool) -> Result<Regex, AppError> {
    if value.len() > 4096 {
        return Err(AppError::new(ErrorCode::InvalidPattern));
    }
    RegexBuilder::new(&if literal {
        regex::escape(value)
    } else {
        value.into()
    })
    .case_insensitive(true)
    .size_limit(1024 * 1024)
    .dfa_size_limit(1024 * 1024)
    .build()
    .map_err(|_| AppError::new(ErrorCode::InvalidPattern))
}
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn kind_name(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Text => "text",
        ClipKind::Image => "image",
        ClipKind::Files => "files",
    }
}
fn parse_kind(kind: &str) -> rusqlite::Result<ClipKind> {
    match kind {
        "text" => Ok(ClipKind::Text),
        "image" => Ok(ClipKind::Image),
        "files" => Ok(ClipKind::Files),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

impl Storage {
    pub fn capture_policy(&self) -> Result<CapturePolicy, AppError> {
        Ok(self.connection.query_row("SELECT enabled,exclude_pattern,max_items,revision FROM capture_policy WHERE singleton=1", [], |r| Ok(CapturePolicy { enabled: r.get(0)?, exclude_pattern: r.get(1)?, max_items: r.get(2)?, revision: r.get::<_,i64>(3)?.to_string() }))?)
    }
    pub fn history_revision(&self) -> Result<String, AppError> {
        Ok(self
            .connection
            .query_row(
                "SELECT revision FROM history_meta WHERE singleton=1",
                [],
                |r| r.get::<_, i64>(0),
            )?
            .to_string())
    }
    pub fn update_capture_policy(
        &mut self,
        request: UpdateCapturePolicy,
    ) -> Result<CapturePolicy, AppError> {
        pattern(&request.exclude_pattern, false)?;
        if !(1..=1000).contains(&request.max_items) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let revision: i64 = request
            .expected_revision
            .parse()
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        if revision < 0 || revision == i64::MAX || revision.to_string() != request.expected_revision
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE capture_policy SET enabled=?1,exclude_pattern=?2,max_items=?3,revision=revision+1 WHERE singleton=1 AND revision=?4", params![request.enabled, request.exclude_pattern, request.max_items, revision])?;
        if changed != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        prune(&tx, request.max_items, None)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        self.capture_policy()
    }
    pub fn capture(&mut self, payload: ClipPayload) -> Result<Option<String>, AppError> {
        self.capture_with_source(payload, None)
    }
    pub fn capture_with_source(
        &mut self,
        payload: ClipPayload,
        source_app: Option<String>,
    ) -> Result<Option<String>, AppError> {
        self.save_payload(payload, true, None, source_app)
    }
    pub fn create_text(&mut self, text: String) -> Result<String, AppError> {
        self.create_text_in_collection(text, None)
    }
    pub fn create_text_in_collection(
        &mut self,
        text: String,
        collection_id: Option<String>,
    ) -> Result<String, AppError> {
        self.save_payload(ClipPayload::Text(text), false, collection_id, None)?
            .ok_or_else(|| AppError::new(ErrorCode::Excluded))
    }
    fn save_payload(
        &mut self,
        payload: ClipPayload,
        captured: bool,
        collection_id: Option<String>,
        source_app: Option<String>,
    ) -> Result<Option<String>, AppError> {
        if source_app.as_ref().is_some_and(|name| {
            name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
        }) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let policy = self.capture_policy()?;
        if !validate_save_payload(&payload, &policy)? {
            return Ok(None);
        }
        if captured && !policy.enabled {
            return Ok(None);
        }
        let (kind, text, data, width, height, count) = match &payload {
            ClipPayload::Text(text) => (ClipKind::Text, text.clone(), vec![], None, None, 0),
            ClipPayload::Image {
                width,
                height,
                rgba,
            } => (
                ClipKind::Image,
                String::new(),
                rgba.clone(),
                Some(*width),
                Some(*height),
                0,
            ),
            ClipPayload::Files(files) => (
                ClipKind::Files,
                serde_json::to_string(files).map_err(|_| AppError::new(ErrorCode::InvalidInput))?,
                vec![],
                None,
                None,
                files.len() as u32,
            ),
        };
        let fingerprint = payload.fingerprint();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Fingerprints are not identities. Only the unclassified, unpinned capture scope deduplicates.
        if let Some(id) = &collection_id {
            crate::collections::ensure_collection(&tx, id)?;
        }
        let existing: Option<String> = if captured {
            tx.query_row("SELECT id FROM clips WHERE fingerprint=?1 AND origin='capture' AND pinned=0 AND deleted=0 AND collection IS NULL LIMIT 1", [&fingerprint], |r| r.get(0)).optional()?
        } else {
            None
        };
        let sequence: i64 = tx.query_row(
            "SELECT revision+1 FROM history_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        tx.execute("INSERT INTO clips(id,kind,text,data,width,height,file_count,fingerprint,captured_at,sequence,origin,collection,revision,source_app) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,?13) ON CONFLICT(id) DO UPDATE SET captured_at=excluded.captured_at,sequence=excluded.sequence,revision=clips.revision+1,source_app=excluded.source_app", params![id,kind_name(kind),text,data,width,height,count,fingerprint,now(),sequence,if captured {"capture"} else {"manual"},collection_id,source_app])?;
        prune(&tx, policy.max_items, Some(&id))?;
        check_change_budget(&tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(Some(id))
    }
    pub fn history(&self, query: HistoryQuery) -> Result<HistoryPage, AppError> {
        let started = std::time::Instant::now();
        let revision = self.history_revision()?;
        if query
            .expected_revision
            .as_ref()
            .is_some_and(|v| *v != revision)
        {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        if query.offset > 1000 {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let matcher = pattern(&query.query, !query.regex)?;
        if let Some(id) = &query.collection_id {
            crate::collections::ensure_collection(&self.connection, id)?;
        }
        let mut statement = self.connection.prepare("SELECT id,kind,text,captured_at,last_used_at,width,height,file_count,revision,origin,pinned,collection,source_app FROM clips WHERE deleted=0 AND (?1=0 OR pinned=1) AND (?2 IS NULL OR collection=?2) ORDER BY pinned DESC,sequence DESC,id")?;
        let mut rows = statement.query(params![query.pinned_only, query.collection_id])?;
        let mut total = 0;
        let mut items = vec![];
        while let Some(row) = rows.next()? {
            if started.elapsed() > std::time::Duration::from_secs(2) {
                return Err(AppError::new(ErrorCode::QueryTimeout));
            }
            let kind = parse_kind(&row.get::<_, String>(1)?)?;
            if query.kind.is_some_and(|k| k != kind) {
                continue;
            }
            let text: String = row.get(2)?;
            let width: Option<u32> = row.get(5)?;
            let height: Option<u32> = row.get(6)?;
            let searchable = if kind == ClipKind::Image {
                format!("图片 {} × {}", width.unwrap_or(0), height.unwrap_or(0))
            } else if kind == ClipKind::Files {
                serde_json::from_str::<Vec<String>>(&text)
                    .map_err(|_| AppError::new(ErrorCode::Storage))?
                    .join("\n")
            } else {
                text.clone()
            };
            if !matcher.is_match(&searchable) {
                continue;
            }
            if total >= query.offset && items.len() < PAGE_SIZE {
                let preview: String = if kind == ClipKind::Files {
                    serde_json::from_str::<Vec<String>>(&text)
                        .map_err(|_| AppError::new(ErrorCode::Storage))?
                        .iter()
                        .map(|p| {
                            Path::new(p)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        })
                        .collect::<Vec<_>>()
                        .join("、")
                        .chars()
                        .take(300)
                        .collect()
                } else {
                    searchable.chars().take(300).collect()
                };
                items.push(HistoryItem {
                    id: row.get(0)?,
                    kind,
                    manual: row.get::<_, String>(9)? == "manual",
                    source_app: row.get(12)?,
                    pinned: row.get(10)?,
                    collection_id: row.get(11)?,
                    match_ranges: if query.query.is_empty() {
                        vec![]
                    } else {
                        matcher
                            .find_iter(&preview)
                            .filter(|m| !m.is_empty())
                            .take(30)
                            .map(|m| {
                                (
                                    preview[..m.start()].encode_utf16().count() as u32,
                                    preview[..m.end()].encode_utf16().count() as u32,
                                )
                            })
                            .collect()
                    },
                    preview,
                    captured_at: row.get::<_, i64>(3)?.to_string(),
                    last_used_at: row.get::<_, Option<i64>>(4)?.map(|v| v.to_string()),
                    width,
                    height,
                    file_count: row.get(7)?,
                    revision: row.get::<_, i64>(8)?.to_string(),
                });
            }
            total += 1;
        }
        Ok(HistoryPage {
            items,
            total,
            revision,
        })
    }
    pub fn quick_paste_slots(&self, include_pinned: bool) -> Result<Vec<QuickPasteSlot>, AppError> {
        let mut statement = self.connection.prepare(
            "SELECT id, revision, kind, CASE WHEN kind='text' THEN substr(text,1,300) ELSE '' END, file_count
             FROM clips WHERE deleted=0 AND (?1 OR pinned=0) ORDER BY sequence DESC,id LIMIT 5",
        )?;
        let slots = statement.query_map([include_pinned], |row| {
            Ok(QuickPasteSlot {
                id: row.get(0)?,
                revision: row.get::<_, i64>(1)?.to_string(),
                item: crate::QuickPasteItem {
                    kind: parse_kind(&row.get::<_, String>(2)?)?,
                    text: row.get(3)?,
                    file_count: row.get(4)?,
                    thumbnail: None,
                },
            })
        })?;
        Ok(slots.collect::<Result<_, _>>()?)
    }

    pub fn read_quick_paste_slot(&self, id: &str, revision: &str) -> Result<ClipPayload, AppError> {
        self.read_active_clip(id, Some(mutation_revision(revision)?))
    }

    pub fn read_clip(&self, id: &str) -> Result<ClipPayload, AppError> {
        self.read_active_clip(id, None)
    }

    fn read_active_clip(&self, id: &str, revision: Option<i64>) -> Result<ClipPayload, AppError> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let row = self
            .connection
            .query_row(
                "SELECT kind,text,data,width,height,revision FROM clips WHERE id=?1 AND deleted=0",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                        r.get::<_, Option<u32>>(3)?,
                        r.get::<_, Option<u32>>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
        if revision.is_some_and(|revision| revision != row.5) {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let payload = match row.0.as_str() {
            "text" => ClipPayload::Text(row.1),
            "files" => ClipPayload::Files(
                serde_json::from_str(&row.1).map_err(|_| AppError::new(ErrorCode::Storage))?,
            ),
            "image" => ClipPayload::Image {
                width: row.3.ok_or_else(|| AppError::new(ErrorCode::Storage))?,
                height: row.4.ok_or_else(|| AppError::new(ErrorCode::Storage))?,
                rgba: row.2,
            },
            _ => return Err(AppError::new(ErrorCode::Storage)),
        };
        payload.validate()?;
        Ok(payload)
    }
    /// Build one bounded clipboard publication in caller-specified display order.
    pub fn read_clips(&self, ids: &[String]) -> Result<ClipPayload, AppError> {
        if ids.is_empty()
            || ids.len() > 100
            || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let mut payload = self.read_clip(&ids[0])?;
        for id in &ids[1..] {
            match (&mut payload, self.read_clip(id)?) {
                (ClipPayload::Text(text), ClipPayload::Text(next)) => {
                    if text.len() + 1 + next.len() > MAX_TEXT_BYTES {
                        return Err(AppError::new(ErrorCode::LimitExceeded));
                    }
                    text.push('\n');
                    text.push_str(&next);
                }
                (ClipPayload::Files(files), ClipPayload::Files(next)) => files.extend(next),
                _ => return Err(AppError::new(ErrorCode::Restricted)),
            }
            payload.validate()?;
        }
        Ok(payload)
    }

    pub fn change_clip_deletion(
        &mut self,
        request: crate::ChangeClipDeletion,
    ) -> Result<crate::ClipDeletionSnapshot, AppError> {
        if uuid::Uuid::parse_str(&request.id).is_err() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let revision: i64 = request
            .expected_revision
            .parse()
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        if revision < 0 || revision == i64::MAX || revision.to_string() != request.expected_revision
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let policy = self.capture_policy()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(i64, bool)> = tx
            .query_row(
                "SELECT revision,deleted FROM clips WHERE id=?1",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (current_revision, deleted) =
            current.ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
        if current_revision != revision || deleted == request.deleted {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        tx.execute(
            "UPDATE clips SET deleted=?1,revision=revision+1 WHERE id=?2 AND revision=?3",
            params![request.deleted, request.id, revision],
        )?;
        if !request.deleted {
            // Undo must never evict another active record to make room.
            let count: u32 =
                tx.query_row("SELECT count(*) FROM clips WHERE deleted=0", [], |r| {
                    r.get(0)
                })?;
            let bytes: i64 = tx.query_row(
                "SELECT coalesce(sum(length(CAST(text AS BLOB))+length(data)),0) FROM clips",
                [],
                |r| r.get(0),
            )?;
            if count > policy.max_items || bytes > MAX_HISTORY_BYTES {
                return Err(AppError::new(ErrorCode::LimitExceeded));
            }
        }
        // Retained deleted payloads share the byte budget and have a bounded count.
        // Keep the record just changed so a successful delete always starts undoable.
        prune(&tx, policy.max_items, Some(&request.id))?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(crate::ClipDeletionSnapshot {
            id: request.id,
            revision: (revision + 1).to_string(),
            deleted: request.deleted,
        })
    }

    pub fn change_clip_pin(
        &mut self,
        request: crate::ChangeClipPin,
    ) -> Result<crate::ClipPinSnapshot, AppError> {
        if uuid::Uuid::parse_str(&request.id).is_err() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let revision = mutation_revision(&request.expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(i64, bool)> = tx
            .query_row(
                "SELECT revision,pinned FROM clips WHERE id=?1 AND deleted=0",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (current_revision, pinned) =
            current.ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
        if current_revision != revision || pinned == request.pinned {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        tx.execute(
            "UPDATE clips SET pinned=?1,revision=revision+1 WHERE id=?2 AND revision=?3",
            params![request.pinned, request.id, revision],
        )?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(crate::ClipPinSnapshot {
            id: request.id,
            revision: (revision + 1).to_string(),
            pinned: request.pinned,
        })
    }

    pub fn deduplicate_pinned(
        &mut self,
        request: crate::DeduplicatePinned,
    ) -> Result<crate::DeduplicationSnapshot, AppError> {
        let revision = mutation_revision(&request.expected_revision)?;
        let policy = self.capture_policy()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 = tx.query_row(
            "SELECT revision FROM history_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if current != revision {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        // Fingerprints narrow the candidates; exact typed payload equality decides deletion.
        // Keep every pinned identity, including multiple pinned copies of the same payload.
        let removed = tx.execute(
            "UPDATE clips AS candidate SET deleted=1,revision=revision+1
            WHERE candidate.deleted=0 AND candidate.pinned=0 AND EXISTS (
                SELECT 1 FROM clips AS keeper
                WHERE keeper.deleted=0 AND keeper.pinned=1
                  AND keeper.fingerprint=candidate.fingerprint
                  AND keeper.kind=candidate.kind AND keeper.text=candidate.text
                  AND keeper.data=candidate.data AND keeper.width IS candidate.width
                  AND keeper.height IS candidate.height AND keeper.file_count=candidate.file_count
            )",
            [],
        )? as u32;
        if removed > 0 {
            // Same bounded deletion-payload retention as ordinary deletion.
            prune(&tx, policy.max_items, None)?;
            tx.execute(
                "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        tx.commit()?;
        Ok(crate::DeduplicationSnapshot {
            removed,
            revision: (revision + i64::from(removed > 0)).to_string(),
        })
    }

    pub fn mark_used(&mut self, id: &str) -> Result<(), AppError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute(
            "UPDATE clips SET last_used_at=?1,revision=revision+1 WHERE id=?2 AND deleted=0",
            params![now(), id],
        )? != 1
        {
            return Err(AppError::new(ErrorCode::NotFound));
        }
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
}
pub(crate) fn mutation_revision(value: &str) -> Result<i64, AppError> {
    let revision: i64 = value
        .parse()
        .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
    if revision < 0 || revision == i64::MAX || revision.to_string() != value {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok(revision)
}
fn prune(tx: &rusqlite::Transaction<'_>, max: u32, keep: Option<&str>) -> Result<(), AppError> {
    prune_protecting(tx, max, keep, None)
}
pub(crate) fn prune_protecting(
    tx: &rusqlite::Transaction<'_>,
    max: u32,
    keep: Option<&str>,
    source: Option<&str>,
) -> Result<(), AppError> {
    loop {
        let (count, bytes, deleted): (i64,i64,i64) = tx.query_row("SELECT count(*) FILTER(WHERE deleted=0),coalesce(sum(length(CAST(text AS BLOB))+length(data)),0),count(*) FILTER(WHERE deleted<>0) FROM clips", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if count <= i64::from(max) && bytes <= MAX_HISTORY_BYTES && deleted <= 1000 {
            return Ok(());
        }
        let removed_deleted = if bytes > MAX_HISTORY_BYTES || deleted > 1000 {
            tx.execute("DELETE FROM clips WHERE id=(SELECT id FROM clips WHERE deleted<>0 AND (?1 IS NULL OR id<>?1) ORDER BY sequence LIMIT 1)",[keep])?
        } else {
            0
        };
        if removed_deleted > 0 {
            continue;
        }
        let changed = tx.execute("DELETE FROM clips WHERE id=(SELECT id FROM clips WHERE origin='capture' AND deleted=0 AND pinned=0 AND collection IS NULL AND (?1 IS NULL OR id<>?1) AND (?2 IS NULL OR id<>?2) ORDER BY sequence LIMIT 1)", params![keep,source])?;
        if changed == 0 {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
    }
}

pub(crate) fn check_change_budget(tx: &rusqlite::Transaction<'_>) -> Result<(), AppError> {
    let pending: u32 = tx.query_row(
        "SELECT (SELECT count(*) FROM clip_changes)+(SELECT count(*) FROM collection_changes)",
        [],
        |r| r.get(0),
    )?;
    if pending > 100_000 {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(())
}

pub(crate) fn validate_save_payload(
    payload: &ClipPayload,
    policy: &CapturePolicy,
) -> Result<bool, AppError> {
    payload.validate()?;
    if policy.exclude_pattern.is_empty() {
        return Ok(true);
    }
    let excluded = pattern(&policy.exclude_pattern, false)?;
    Ok(!match payload {
        ClipPayload::Text(text) => excluded.is_match(text),
        ClipPayload::Files(paths) => paths.iter().any(|path| excluded.is_match(path)),
        ClipPayload::Image { .. } => false,
    })
}

#[cfg(test)]
mod disk_tests {
    use super::*;

    #[test]
    fn batch_read_preserves_order_and_rejects_invalid_or_incompatible_selection() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("batch.sqlite3")).unwrap();
        let a = storage
            .capture(ClipPayload::Text("first".into()))
            .unwrap()
            .unwrap();
        let b = storage
            .capture(ClipPayload::Text("second".into()))
            .unwrap()
            .unwrap();
        assert_eq!(
            storage.read_clips(&[b.clone(), a.clone()]).unwrap(),
            ClipPayload::Text("second\nfirst".into())
        );
        assert_eq!(
            storage.read_clips(&[]).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            storage
                .read_clips(&[a.clone(), a.clone()])
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
        let f = storage
            .capture(ClipPayload::Files(vec![std::env::temp_dir()
                .join("one.txt")
                .to_str()
                .unwrap()
                .into()]))
            .unwrap()
            .unwrap();
        let g = storage
            .capture(ClipPayload::Files(vec![std::env::temp_dir()
                .join("two.txt")
                .to_str()
                .unwrap()
                .into()]))
            .unwrap()
            .unwrap();
        assert!(
            matches!(storage.read_clips(&[f.clone(), g]).unwrap(), ClipPayload::Files(paths) if paths.len() == 2)
        );
        assert_eq!(
            storage.read_clips(&[a.clone(), f]).unwrap_err().code,
            ErrorCode::Restricted
        );
        let image = storage
            .capture(ClipPayload::Image {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            storage.read_clips(&[a.clone(), image]).unwrap_err().code,
            ErrorCode::Restricted
        );
        let large = storage
            .capture(ClipPayload::Text("x".repeat(MAX_TEXT_BYTES)))
            .unwrap()
            .unwrap();
        assert_eq!(
            storage.read_clips(&[large, b]).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        storage
            .change_clip_deletion(crate::ChangeClipDeletion {
                id: a.clone(),
                expected_revision: "0".into(),
                deleted: true,
            })
            .unwrap();
        assert_eq!(
            storage.read_clips(&[a]).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn sqlite_full_rolls_back_content_revision_and_change_record_then_recovers() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("full.sqlite3")).unwrap();
        storage
            .update_capture_policy(UpdateCapturePolicy {
                enabled: true,
                exclude_pattern: String::new(),
                max_items: 500,
                expected_revision: "0".into(),
            })
            .unwrap();
        let before = storage.history_revision().unwrap();
        let pages: i64 = storage
            .connection
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        storage
            .connection
            .query_row(&format!("PRAGMA max_page_count={pages}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        let payload = ClipPayload::Text("x".repeat(1024 * 1024));
        assert_eq!(
            storage.capture(payload.clone()).unwrap_err().code,
            ErrorCode::Storage
        );
        assert_eq!(storage.history_revision().unwrap(), before);
        assert_eq!(
            storage
                .connection
                .query_row("SELECT count(*) FROM clips", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            storage
                .connection
                .query_row("SELECT count(*) FROM clip_changes", [], |r| r
                    .get::<_, u32>(0))
                .unwrap(),
            0
        );
        storage
            .connection
            .query_row("PRAGMA max_page_count=1073741823", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        let id = storage.capture(payload.clone()).unwrap().unwrap();
        assert_eq!(storage.read_clip(&id).unwrap(), payload);
    }
}
