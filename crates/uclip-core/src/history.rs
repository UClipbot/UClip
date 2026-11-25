use crate::{
    AppError, CapturePolicy, ClipKind, ClipVersion, ErrorCode, HistoryItem, HistoryPage,
    HistoryQuery, Storage, UpdateCapturePolicy,
};
use regex::{Regex, RegexBuilder};
use rusqlite::{
    params, params_from_iter, types::Value, OptionalExtension, Transaction, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
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
pub const MAX_HISTORY_ITEMS_PER_TAB: u32 = 30_000;
pub const MAX_HISTORY_ITEMS: u32 =
    MAX_HISTORY_ITEMS_PER_TAB * (crate::collections::MAX_COLLECTIONS + 1);
pub const PAGE_SIZE: usize = 25;
pub(crate) const MAX_TRASH_ITEMS: u32 = MAX_HISTORY_ITEMS_PER_TAB;
const MAX_FULL_HISTORY_RESPONSE_BYTES: i64 = 128 * 1024 * 1024;
pub(crate) const MAX_LOCAL_HISTORY_BYTES: i64 = 128 * 1024 * 1024;

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

pub(crate) fn read_capture_policy(
    connection: &rusqlite::Connection,
) -> Result<CapturePolicy, AppError> {
    Ok(connection.query_row(
        "SELECT enabled,exclude_pattern,max_items,revision FROM capture_policy WHERE singleton=1",
        [],
        |row| {
            Ok(CapturePolicy {
                enabled: row.get(0)?,
                exclude_pattern: row.get(1)?,
                max_items: row.get(2)?,
                revision: row.get::<_, i64>(3)?.to_string(),
            })
        },
    )?)
}

impl Storage {
    pub fn capture_policy(&self) -> Result<CapturePolicy, AppError> {
        read_capture_policy(&self.connection)
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
        let max_items = request.max_items.clamp(1, MAX_HISTORY_ITEMS_PER_TAB);
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
        let changed = tx.execute("UPDATE capture_policy SET enabled=?1,exclude_pattern=?2,max_items=?3,revision=revision+1 WHERE singleton=1 AND revision=?4", params![request.enabled, request.exclude_pattern, max_items, revision])?;
        if changed != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        if !crate::replica::enrolled(&tx)?
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM clips WHERE deleted=0 AND collection IS NOT NULL
             GROUP BY collection HAVING count(*)>?1)",
                [max_items],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        prune(&tx, max_items, None)?;
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
            .map(|clip| clip.map(|clip| clip.id))
    }
    pub fn create_text(&mut self, text: String) -> Result<String, AppError> {
        self.create_text_in_collection(text, None)
    }
    pub fn create_text_in_collection(
        &mut self,
        text: String,
        collection_id: Option<String>,
    ) -> Result<String, AppError> {
        self.create_clip(ClipPayload::Text(text), collection_id)
    }

    pub fn create_clip(
        &mut self,
        payload: ClipPayload,
        collection_id: Option<String>,
    ) -> Result<String, AppError> {
        self.create_clip_version(payload, collection_id)
            .map(|clip| clip.id)
    }

    pub fn create_clip_version(
        &mut self,
        payload: ClipPayload,
        collection_id: Option<String>,
    ) -> Result<ClipVersion, AppError> {
        self.save_payload(payload, false, collection_id, None)?
            .ok_or_else(|| AppError::new(ErrorCode::Excluded))
    }
    fn save_payload(
        &mut self,
        payload: ClipPayload,
        captured: bool,
        collection_id: Option<String>,
        source_app: Option<String>,
    ) -> Result<Option<ClipVersion>, AppError> {
        if source_app.as_ref().is_some_and(|name| {
            name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
        }) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        payload.validate()?;
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
        let policy = read_capture_policy(&tx)?;
        if (captured && !policy.enabled) || !allowed_by_capture_policy(&payload, &policy)? {
            return Ok(None);
        }
        // Fingerprints are not identities. Only the unclassified, unpinned capture scope deduplicates.
        if let Some(id) = &collection_id {
            crate::collections::ensure_collection(&tx, id)?;
        }
        let existing: Option<String> = if captured {
            tx.query_row("SELECT id FROM clips WHERE fingerprint=?1 AND origin='capture' AND pinned=0 AND deleted=0 AND collection IS NULL
                AND NOT EXISTS (SELECT 1 FROM replica_local_clips l WHERE l.id=clips.id)
                AND NOT EXISTS (SELECT 1 FROM replica_files f,replica_state s WHERE f.id=clips.id AND f.device<>s.device) LIMIT 1", [&fingerprint], |r| r.get(0)).optional()?
        } else {
            None
        };
        let sequence: i64 = tx.query_row(
            "SELECT revision+1 FROM history_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let previous_bytes = history_payload_bytes(&tx)?;
        let revision: i64 = tx.query_row(
            "INSERT INTO clips(id,kind,text,data,width,height,file_count,fingerprint,captured_at,sequence,origin,collection,revision,source_app)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,?13)
             ON CONFLICT(id) DO UPDATE SET captured_at=excluded.captured_at,sequence=excluded.sequence,revision=clips.revision+1,source_app=excluded.source_app
             RETURNING revision",
            params![id, kind_name(kind), text, data, width, height, count, fingerprint, now(), sequence, if captured {"capture"} else {"manual"}, collection_id, source_app],
            |row| row.get(0),
        )?;
        if let Some(collection) = collection_id.as_deref() {
            ensure_tab_capacity(&tx, policy.max_items, Some(collection))?;
        }
        prune(&tx, policy.max_items, Some(&id))?;
        enforce_local_payload_budget(&tx, previous_bytes, &id, "[]")?;
        check_change_budget(&tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(Some(ClipVersion {
            id,
            expected_revision: revision.to_string(),
        }))
    }
    pub fn history(&self, query: HistoryQuery) -> Result<HistoryPage, AppError> {
        self.history_window(query, false, None)
    }

    pub fn history_window(
        &self,
        query: HistoryQuery,
        trash: bool,
        count: Option<u32>,
    ) -> Result<HistoryPage, AppError> {
        if count.is_some() && query.expected_revision.is_none() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let _snapshot = self.connection.unchecked_transaction()?;
        self.history_in_snapshot(query, trash, count, None, || true)
    }

    /// Read a cancellable window and optionally publish an ordered prefix for the first page.
    pub fn search_history(
        &self,
        query: HistoryQuery,
        trash: bool,
        count: Option<u32>,
        on_first_page: Option<impl FnOnce(HistoryPage)>,
        is_current: impl Fn() -> bool,
    ) -> Result<HistoryPage, AppError> {
        if (count.is_some() && query.expected_revision.is_none())
            || (on_first_page.is_some() && (query.offset != 0 || count.is_some()))
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let _snapshot = self.connection.unchecked_transaction()?;
        self.history_in_snapshot(
            query,
            trash,
            count,
            on_first_page.map(|publish| Box::new(publish) as Box<dyn FnOnce(HistoryPage)>),
            is_current,
        )
    }

    pub(crate) fn history_content(
        &self,
        query: HistoryQuery,
        count: u32,
    ) -> Result<Vec<(HistoryItem, ClipPayload)>, AppError> {
        let snapshot = self.connection.unchecked_transaction()?;
        let page = self.history_in_snapshot(query, false, Some(count), None, || true)?;
        let mut remaining = MAX_FULL_HISTORY_RESPONSE_BYTES;
        page.items
            .into_iter()
            .map(|item| {
                let size: i64 = snapshot.query_row(
                    "SELECT length(CAST(text AS BLOB))+length(data) FROM clips WHERE id=?1",
                    [&item.id],
                    |row| row.get(0),
                )?;
                if size > remaining {
                    return Err(AppError::new(ErrorCode::LimitExceeded));
                }
                remaining -= size;
                let payload = Self::read_active_clip(&snapshot, &item.id, None)?;
                Ok((item, payload))
            })
            .collect()
    }

    fn history_in_snapshot(
        &self,
        query: HistoryQuery,
        trash: bool,
        count: Option<u32>,
        mut on_first_page: Option<Box<dyn FnOnce(HistoryPage) + '_>>,
        is_current: impl Fn() -> bool,
    ) -> Result<HistoryPage, AppError> {
        if count.is_some_and(|count| !(1..=1000).contains(&count))
            || (trash && (query.pinned_only || query.collection_id.is_some()))
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        if !is_current() {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let limit = count.map_or(PAGE_SIZE, |count| count as usize);
        let started = std::time::Instant::now();
        let revision = self.history_revision()?;
        if query
            .expected_revision
            .as_ref()
            .is_some_and(|v| *v != revision)
        {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let synchronized = crate::replica::enrolled(&self.connection)?;
        let max_offset = if synchronized {
            MAX_HISTORY_ITEMS + MAX_TRASH_ITEMS + crate::replica::MAX_ENTITIES
        } else {
            MAX_HISTORY_ITEMS
        };
        if query.offset > max_offset {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let order = if synchronized {
            "captured_at"
        } else {
            "sequence"
        };
        let matcher = (!query.query.is_empty())
            .then(|| pattern(&query.query, !query.regex))
            .transpose()?;
        if matcher.is_none() {
            on_first_page = None;
        }
        let mut filter = String::from("deleted=?");
        let mut values = vec![Value::Integer(i64::from(trash))];
        if query.pinned_only {
            filter.push_str(" AND pinned=1");
        }
        if let Some(id) = &query.collection_id {
            crate::collections::ensure_collection(&self.connection, id)?;
            filter.push_str(" AND collection=?");
            values.push(Value::Text(id.clone()));
        }
        if let Some(kind) = query.kind {
            filter.push_str(" AND kind=?");
            values.push(Value::Text(kind_name(kind).into()));
        }
        let mut total = 0;
        let paging = if matcher.is_none() {
            total = self.connection.query_row(
                &format!("SELECT count(*) FROM clips WHERE {filter}"),
                params_from_iter(&values),
                |row| row.get(0),
            )?;
            values.push(Value::Integer(limit as i64));
            values.push(Value::Integer(i64::from(query.offset)));
            "LIMIT ? OFFSET ?"
        } else {
            ""
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT id,kind,text,captured_at,last_used_at,width,height,file_count,revision,origin,pinned,collection,source_app
             FROM clips WHERE {filter} ORDER BY pinned DESC,{order} DESC,id {paging}"
        ))?;
        let mut rows = statement.query(params_from_iter(&values))?;
        let mut items = vec![];
        while let Some(row) = rows.next()? {
            if !is_current() {
                return Err(AppError::new(ErrorCode::Conflict));
            }
            let elapsed = started.elapsed();
            if elapsed > std::time::Duration::from_secs(2) {
                return Err(AppError::new(ErrorCode::QueryTimeout));
            }
            if on_first_page.is_some()
                && !items.is_empty()
                && (items.len() == limit || elapsed >= std::time::Duration::from_millis(30))
            {
                if let Some(publish) = on_first_page.take() {
                    publish(HistoryPage {
                        items: items.clone(),
                        total,
                        revision: revision.clone(),
                    });
                }
            }
            let kind = parse_kind(&row.get::<_, String>(1)?)?;
            let text = row
                .get_ref(2)?
                .as_str()
                .map_err(|_| AppError::new(ErrorCode::Storage))?;
            let paths = (kind == ClipKind::Files)
                .then(|| serde_json::from_str::<Vec<String>>(text))
                .transpose()
                .map_err(|_| AppError::new(ErrorCode::Storage))?;
            let searchable = match (kind, paths.as_ref(), matcher.is_some()) {
                (ClipKind::Image, _, _) => Cow::Owned(format!(
                    "图片 {} × {}",
                    row.get::<_, Option<u32>>(5)?.unwrap_or(0),
                    row.get::<_, Option<u32>>(6)?.unwrap_or(0)
                )),
                (_, Some(paths), true) => Cow::Owned(paths.join("\n")),
                _ => Cow::Borrowed(text),
            };
            if matcher
                .as_ref()
                .is_some_and(|regex| !regex.is_match(&searchable))
            {
                continue;
            }
            if matcher.is_none() || (total >= query.offset && items.len() < limit) {
                let preview: String = if let Some(paths) = paths {
                    paths
                        .iter()
                        .map(|path| {
                            if path.starts_with('/') {
                                path.rsplit('/').find(|part| !part.is_empty())
                            } else {
                                path.rsplit(['/', '\\']).find(|part| !part.is_empty())
                            }
                            .unwrap_or_default()
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
                    match_ranges: matcher.as_ref().map_or_else(Vec::new, |regex| {
                        regex
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
                    }),
                    preview,
                    captured_at: row.get::<_, i64>(3)?.to_string(),
                    last_used_at: row.get::<_, Option<i64>>(4)?.map(|v| v.to_string()),
                    width: row.get(5)?,
                    height: row.get(6)?,
                    file_count: row.get(7)?,
                    revision: row.get::<_, i64>(8)?.to_string(),
                });
            }
            if matcher.is_some() {
                total += 1;
            }
        }
        Ok(HistoryPage {
            items,
            total,
            revision,
        })
    }
    pub fn quick_paste_slots(&self, include_pinned: bool) -> Result<Vec<QuickPasteSlot>, AppError> {
        let order = if crate::replica::enrolled(&self.connection)? {
            "captured_at"
        } else {
            "sequence"
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT id, revision, kind, CASE WHEN kind='text' THEN substr(text,1,300) ELSE '' END, file_count
             FROM clips WHERE deleted=0 AND (?1 OR pinned=0) ORDER BY {order} DESC,id LIMIT 5",
        ))?;
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
        let snapshot = self.connection.unchecked_transaction()?;
        Self::read_active_clip(&snapshot, id, Some(mutation_revision(revision)?))
    }

    pub fn read_clip(&self, id: &str) -> Result<ClipPayload, AppError> {
        self.read_clip_in_history(id, None)
    }

    pub fn read_clip_in_history(
        &self,
        id: &str,
        expected_revision: Option<&str>,
    ) -> Result<ClipPayload, AppError> {
        let _snapshot = self.connection.unchecked_transaction()?;
        if let Some(revision) = expected_revision {
            if self.history_revision()? != revision {
                return Err(AppError::new(ErrorCode::Conflict));
            }
        }
        Self::read_active_clip(&self.connection, id, None)
    }

    pub fn editable_content(
        &self,
        clip: crate::ClipVersion,
    ) -> Result<crate::EditableContent, AppError> {
        let snapshot = self.connection.unchecked_transaction()?;
        Self::read_editable_content(&snapshot, &clip)
    }

    fn read_editable_content(
        connection: &rusqlite::Connection,
        clip: &crate::ClipVersion,
    ) -> Result<crate::EditableContent, AppError> {
        if crate::replica::remote_file(connection, &clip.id)?.is_some() {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        match Self::read_active_clip(
            connection,
            &clip.id,
            Some(mutation_revision(&clip.expected_revision)?),
        )? {
            ClipPayload::Text(text) => Ok(crate::EditableContent::Text(text)),
            ClipPayload::Files(files) => Ok(crate::EditableContent::Files(files)),
            ClipPayload::Image { .. } => Err(AppError::new(ErrorCode::Restricted)),
        }
    }

    pub fn edit_clip(&mut self, request: crate::EditClip) -> Result<(), AppError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = Self::read_editable_content(&tx, &request.clip)?;
        let (payload, text, count) = match (current, request.content) {
            (crate::EditableContent::Text(before), crate::EditableContent::Text(text))
                if before != text =>
            {
                (ClipPayload::Text(text.clone()), text, 0)
            }
            (crate::EditableContent::Files(before), crate::EditableContent::Files(paths))
                if before != paths =>
            {
                let text = serde_json::to_string(&paths)
                    .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
                let count = paths.len();
                (ClipPayload::Files(paths), text, count)
            }
            _ => return Err(AppError::new(ErrorCode::InvalidInput)),
        };
        let policy = read_capture_policy(&tx)?;
        if !validate_save_payload(&payload, &policy)? {
            return Err(AppError::new(ErrorCode::Excluded));
        }
        let id = request.clip.id;
        let previous_bytes = history_payload_bytes(&tx)?;
        if tx.execute(
            "UPDATE clips SET text=?1,file_count=?2,fingerprint=?3,captured_at=?4,revision=revision+1 WHERE id=?5 AND deleted=0 AND revision=?6",
            params![text, count as u32, payload.fingerprint(), now(), id, mutation_revision(&request.clip.expected_revision)?],
        )? != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        prune(&tx, policy.max_items, Some(&id))?;
        enforce_local_payload_budget(&tx, previous_bytes, &id, "[]")?;
        check_change_budget(&tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn read_active_clip(
        connection: &rusqlite::Connection,
        id: &str,
        revision: Option<i64>,
    ) -> Result<ClipPayload, AppError> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let row = connection
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
        match &payload {
            ClipPayload::Files(paths)
                if crate::replica::remote_file(connection, id)?.is_some()
                    && crate::replica::portable_paths(paths) => {}
            _ => payload.validate()?,
        }
        Ok(payload)
    }
    /// Build one bounded clipboard publication in caller-specified display order.
    pub fn read_clips(&self, ids: &[String]) -> Result<ClipPayload, AppError> {
        self.clipboard_payload(ids, None, None, Vec::new())
    }

    pub fn clipboard_payload(
        &self,
        ids: &[String],
        history_revision: Option<&str>,
        slot_revision: Option<&str>,
        staged: Vec<crate::FileReplacement>,
    ) -> Result<ClipPayload, AppError> {
        validate_clip_ids(ids)?;
        let _snapshot = self.connection.unchecked_transaction()?;
        if let Some(revision) = history_revision {
            if self.history_revision()? != revision {
                return Err(AppError::new(ErrorCode::Conflict));
            }
        }
        let slot_revision = slot_revision.map(mutation_revision).transpose()?;
        if slot_revision.is_some() && ids.len() != 1 {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let mut staged = staged
            .into_iter()
            .map(|file| (file.id.clone(), file))
            .collect::<std::collections::HashMap<_, _>>();
        let mut read = |id: &str| -> Result<ClipPayload, AppError> {
            let original = Self::read_active_clip(&self.connection, id, slot_revision)?;
            if matches!(original, ClipPayload::Files(_)) {
                if let Some(ticket) = crate::replica::remote_file(&self.connection, id)? {
                    let file = staged
                        .remove(id)
                        .ok_or_else(|| AppError::new(ErrorCode::FilePeerOffline))?;
                    if file.ticket != ticket {
                        return Err(AppError::new(ErrorCode::Conflict));
                    }
                    let payload = ClipPayload::Files(file.paths);
                    payload.validate()?;
                    return Ok(payload);
                }
                crate::replica::verify_local_file(&self.connection, id)?;
            }
            if staged.contains_key(id) {
                return Err(AppError::new(ErrorCode::Conflict));
            }
            Ok(original)
        };
        let mut payload = read(&ids[0])?;
        for id in &ids[1..] {
            match (&mut payload, read(id)?) {
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

    pub fn remote_file_tickets(
        &self,
        ids: Vec<String>,
    ) -> Result<Vec<(String, crate::FileTicket)>, AppError> {
        validate_clip_ids(&ids)?;
        let mut files = Vec::new();
        for id in ids {
            if let Some(ticket) = crate::replica::remote_file(&self.connection, &id)? {
                files.push((id, ticket));
            }
        }
        Ok(files)
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
            "UPDATE clips SET deleted=?1,revision=revision+1,collection=CASE WHEN ?1=0 AND NOT EXISTS(SELECT 1 FROM collections WHERE id=clips.collection AND deleted=0) THEN NULL ELSE collection END WHERE id=?2 AND revision=?3",
            params![request.deleted, request.id, revision],
        )?;
        if !request.deleted && !crate::replica::enrolled(&tx)? {
            // Undo must never evict another active record to make room.
            let collection: Option<String> = tx.query_row(
                "SELECT collection FROM clips WHERE id=?1",
                [&request.id],
                |row| row.get(0),
            )?;
            ensure_tab_capacity(&tx, policy.max_items, collection.as_deref())?;
        }
        // Retained deleted payloads have a bounded count.
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

    pub fn purge_trash(&mut self, request: crate::PurgeTrash) -> Result<u32, AppError> {
        use crate::PurgeTrash::{All, Selected};
        let (revision, ids) = match request {
            Selected(request) => (request.expected_revision, Some(request.ids)),
            All(request) => (request.expected_revision, None),
        };
        let revision = mutation_revision(&revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 = tx.query_row(
            "SELECT revision FROM history_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if current != revision {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let removed = if let Some(ids) = ids {
            if ids.is_empty()
                || ids.len() > 1000
                || ids.iter().any(|id| uuid::Uuid::parse_str(id).is_err())
            {
                return Err(AppError::new(ErrorCode::InvalidInput));
            }
            let ids_json =
                serde_json::to_string(&ids).map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
            let removed = tx.execute(
                "DELETE FROM clips WHERE deleted=1 AND id IN (SELECT value FROM json_each(?1))",
                [ids_json],
            )?;
            if removed != ids.len() {
                return Err(AppError::new(ErrorCode::Conflict));
            }
            removed
        } else {
            tx.execute("DELETE FROM clips WHERE deleted=1", [])?
        };
        if removed > 0 {
            check_change_budget(&tx)?;
            tx.execute(
                "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        tx.commit()?;
        Ok(removed as u32)
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
                  AND (keeper.kind<>'files' OR
                    coalesce((SELECT device FROM replica_files WHERE id=keeper.id),
                                     (SELECT device FROM replica_state WHERE singleton=1)) =
                    coalesce((SELECT device FROM replica_files WHERE id=candidate.id),
                                     (SELECT device FROM replica_state WHERE singleton=1)))
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
fn validate_clip_ids(ids: &[String]) -> Result<(), AppError> {
    if ids.is_empty()
        || ids.len() > 100
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
        || ids.iter().any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok(())
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
    prune_protecting(tx, max, keep, "[]")
}

pub(crate) fn migrate_payload_usage(tx: &Transaction<'_>) -> Result<(), AppError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS history_usage (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            payload_bytes INTEGER NOT NULL CHECK(payload_bytes>=0)
         ) STRICT;
         INSERT OR REPLACE INTO history_usage SELECT 1,coalesce(sum(length(CAST(text AS BLOB))+length(data)),0) FROM clips;
         CREATE TRIGGER IF NOT EXISTS history_usage_insert AFTER INSERT ON clips BEGIN
            UPDATE history_usage SET payload_bytes=payload_bytes+length(CAST(NEW.text AS BLOB))+length(NEW.data) WHERE singleton=1;
         END;
         CREATE TRIGGER IF NOT EXISTS history_usage_update AFTER UPDATE OF text,data ON clips BEGIN
            UPDATE history_usage SET payload_bytes=payload_bytes+length(CAST(NEW.text AS BLOB))+length(NEW.data)
              -length(CAST(OLD.text AS BLOB))-length(OLD.data) WHERE singleton=1;
         END;
         CREATE TRIGGER IF NOT EXISTS history_usage_delete AFTER DELETE ON clips BEGIN
            UPDATE history_usage SET payload_bytes=payload_bytes-length(CAST(OLD.text AS BLOB))-length(OLD.data) WHERE singleton=1;
         END;",
    )?;
    Ok(())
}

pub(crate) fn validate_payload_usage(connection: &rusqlite::Connection) -> Result<(), AppError> {
    history_payload_bytes(connection)?;
    let triggers: u32 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='trigger'
         AND name IN ('history_usage_insert','history_usage_update','history_usage_delete')",
        [],
        |row| row.get(0),
    )?;
    if triggers != 3 {
        return Err(AppError::new(ErrorCode::Storage));
    }
    Ok(())
}

pub(crate) fn history_payload_bytes(connection: &rusqlite::Connection) -> Result<i64, AppError> {
    Ok(connection.query_row(
        "SELECT payload_bytes FROM history_usage WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn enforce_local_payload_budget(
    tx: &Transaction<'_>,
    previous_bytes: i64,
    keep: &str,
    protected: &str,
) -> Result<(), AppError> {
    let bytes = history_payload_bytes(tx)?;
    if bytes <= MAX_LOCAL_HISTORY_BYTES || bytes <= previous_bytes || crate::replica::enrolled(tx)?
    {
        return Ok(());
    }
    // An already oversized library can shrink without evicting its existing records on upgrade.
    if previous_bytes > MAX_LOCAL_HISTORY_BYTES {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    tx.execute(
        "DELETE FROM clips WHERE id IN (
            SELECT id FROM (
                SELECT id, coalesce(sum(length(CAST(text AS BLOB))+length(data)) OVER (
                    ORDER BY deleted DESC,sequence,id ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                ),0) AS prior_bytes
                FROM clips WHERE id<>?1 AND id NOT IN (SELECT value FROM json_each(?2))
                  AND (deleted=1 OR (origin='capture' AND pinned=0 AND collection IS NULL))
            ) WHERE prior_bytes<?3
         )",
        params![keep, protected, bytes - MAX_LOCAL_HISTORY_BYTES],
    )?;
    if history_payload_bytes(tx)? > MAX_LOCAL_HISTORY_BYTES {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(())
}

fn tab_over_capacity(
    tx: &rusqlite::Transaction<'_>,
    max: u32,
    collection: Option<&str>,
) -> Result<bool, AppError> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM clips WHERE collection IS ?1 AND deleted=0 LIMIT 1 OFFSET ?2)",
        params![collection, max],
        |row| row.get(0),
    )?)
}

pub(crate) fn ensure_tab_capacity(
    tx: &rusqlite::Transaction<'_>,
    max: u32,
    collection: Option<&str>,
) -> Result<(), AppError> {
    if !crate::replica::enrolled(tx)? && tab_over_capacity(tx, max, collection)? {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(())
}

pub(crate) fn trash_count(tx: &rusqlite::Connection) -> Result<u32, AppError> {
    Ok(
        tx.query_row("SELECT count(*) FROM clips WHERE deleted=1", [], |row| {
            row.get(0)
        })?,
    )
}
pub(crate) fn prune_protecting(
    tx: &rusqlite::Transaction<'_>,
    max: u32,
    keep: Option<&str>,
    protected: &str,
) -> Result<(), AppError> {
    if crate::replica::enrolled(tx)? {
        return crate::replica::check_capacity(tx);
    }
    let unclassified: u32 = tx.query_row(
        "SELECT count(*) FROM clips WHERE collection IS NULL AND deleted=0",
        [],
        |row| row.get(0),
    )?;
    let trash_excess = trash_count(tx)?.saturating_sub(MAX_TRASH_ITEMS);
    if trash_excess > 0 {
        let removed = tx.execute(
            "DELETE FROM clips WHERE id IN (
                SELECT id FROM clips WHERE deleted=1 AND (?1 IS NULL OR id<>?1)
                ORDER BY sequence,id LIMIT ?2
            )",
            params![keep, trash_excess],
        )?;
        if removed != trash_excess as usize {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
    }
    let excess = unclassified.saturating_sub(max);
    if excess > 0 {
        let removed = tx.execute(
            "DELETE FROM clips WHERE id IN (
                SELECT id FROM clips WHERE origin='capture' AND deleted=0 AND pinned=0
                AND collection IS NULL AND (?1 IS NULL OR id<>?1)
                AND id NOT IN (SELECT value FROM json_each(?2))
                ORDER BY sequence,id LIMIT ?3
            )",
            params![keep, protected, excess],
        )?;
        if removed != excess as usize {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
    }
    Ok(())
}

pub(crate) fn check_change_budget(tx: &rusqlite::Transaction<'_>) -> Result<(), AppError> {
    let pending: u32 = tx.query_row(
        "SELECT (SELECT count(*) FROM clip_changes)+(SELECT count(*) FROM collection_changes)",
        [],
        |r| r.get(0),
    )?;
    if pending > MAX_HISTORY_ITEMS + MAX_TRASH_ITEMS + 100_000 {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(())
}

pub(crate) fn validate_save_payload(
    payload: &ClipPayload,
    policy: &CapturePolicy,
) -> Result<bool, AppError> {
    payload.validate()?;
    allowed_by_capture_policy(payload, policy)
}

pub(crate) fn allowed_by_capture_policy(
    payload: &ClipPayload,
    policy: &CapturePolicy,
) -> Result<bool, AppError> {
    if policy.exclude_pattern.is_empty() {
        return Ok(true);
    }
    let excluded = pattern(&policy.exclude_pattern, false)?;
    Ok(!matches_capture_exclusion(payload, &excluded))
}

pub(crate) fn matches_capture_exclusion(payload: &ClipPayload, excluded: &Regex) -> bool {
    match payload {
        ClipPayload::Text(text) => excluded.is_match(text),
        ClipPayload::Files(paths) => paths.iter().any(|path| excluded.is_match(path)),
        ClipPayload::Image { .. } => false,
    }
}

#[cfg(test)]
mod disk_tests {
    use super::*;

    #[test]
    fn saved_version_rejects_a_concurrent_edit_before_copy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("version.sqlite3");
        let mut storage = Storage::open(&path).unwrap();
        let original = ClipPayload::Text("original".into());
        let saved = storage.create_clip_version(original.clone(), None).unwrap();
        assert_eq!(
            storage
                .clipboard_payload(
                    std::slice::from_ref(&saved.id),
                    None,
                    Some(&saved.expected_revision),
                    vec![]
                )
                .unwrap(),
            original
        );
        let mut editor = Storage::open(&path).unwrap();
        editor
            .edit_clip(crate::EditClip {
                clip: saved.clone(),
                content: crate::EditableContent::Text("concurrent edit".into()),
            })
            .unwrap();
        assert_eq!(
            storage
                .clipboard_payload(&[saved.id], None, Some(&saved.expected_revision), vec![])
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
    }

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
    fn trash_purge_is_atomic_revision_checked_and_replicated() {
        use crate::{ChangeClipDeletion, EmptyTrash, PurgeSelectedTrash, PurgeTrash};

        assert!(serde_json::from_str::<PurgeTrash>(r#"{"expectedRevision":"0"}"#).is_err());
        assert!(serde_json::from_str::<PurgeTrash>(
            r#"{"scope":"all","expectedRevision":"0","ids":[]}"#
        )
        .is_err());
        let dir = tempfile::tempdir().unwrap();
        let mut source = Storage::open(&dir.path().join("source.sqlite3")).unwrap();
        let mut peer = Storage::open(&dir.path().join("peer.sqlite3")).unwrap();
        source.enroll_replica().unwrap();
        peer.enroll_replica().unwrap();
        let active = source.create_text("active".into()).unwrap();
        let first = source.create_text("first".into()).unwrap();
        let second = source.create_text("second".into()).unwrap();
        for id in [&first, &second] {
            source
                .change_clip_deletion(ChangeClipDeletion {
                    id: id.clone(),
                    expected_revision: "0".into(),
                    deleted: true,
                })
                .unwrap();
        }
        let first_stamp = source
            .replica_inventory()
            .unwrap()
            .into_iter()
            .find(|stamp| stamp.id.id == first)
            .unwrap();
        let previous = source.replica_entry(&first_stamp).unwrap().unwrap();
        assert!(peer
            .apply_replica_entry(source.replica_entry(&first_stamp).unwrap().unwrap())
            .unwrap());
        let revision = source.history_revision().unwrap();
        assert_eq!(
            source
                .purge_trash(PurgeTrash::Selected(PurgeSelectedTrash {
                    expected_revision: revision.clone(),
                    ids: vec![first.clone(), active.clone()],
                }))
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(source.history_revision().unwrap(), revision);
        assert!(source.replica_entry(&first_stamp).unwrap().is_some());
        assert_eq!(
            source
                .purge_trash(PurgeTrash::Selected(PurgeSelectedTrash {
                    expected_revision: revision.clone(),
                    ids: vec![first.clone()],
                }))
                .unwrap(),
            1
        );
        assert_eq!(
            source
                .purge_trash(PurgeTrash::All(EmptyTrash {
                    expected_revision: revision
                }))
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        let tombstone = source
            .replica_inventory()
            .unwrap()
            .into_iter()
            .find(|stamp| stamp.id.id == first)
            .unwrap();
        assert!(tombstone.removed);
        assert!(source.replica_entry(&first_stamp).unwrap().is_none());
        let tombstone = source.replica_entry(&tombstone).unwrap().unwrap();
        assert!(tombstone.body.is_none());
        assert!(peer.apply_replica_entry(tombstone).unwrap());
        assert!(!peer.apply_replica_entry(previous).unwrap());
        assert_eq!(
            peer.change_clip_deletion(ChangeClipDeletion {
                id: first,
                expected_revision: "0".into(),
                deleted: false,
            })
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            source
                .purge_trash(PurgeTrash::All(EmptyTrash {
                    expected_revision: source.history_revision().unwrap(),
                }))
                .unwrap(),
            1
        );
        let revision = source.history_revision().unwrap();
        assert_eq!(
            source
                .purge_trash(PurgeTrash::All(EmptyTrash {
                    expected_revision: revision.clone()
                }))
                .unwrap(),
            0
        );
        assert_eq!(source.history_revision().unwrap(), revision);
        assert_eq!(
            source.read_clip(&active).unwrap(),
            ClipPayload::Text("active".into())
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
