use crate::{
    history::{
        allowed_by_capture_policy, check_change_budget, mutation_revision, now, prune_protecting,
        read_capture_policy,
    },
    AppError, ClipTransferBatch, ClipTransferSnapshot, CollectionItem, CollectionsSnapshot,
    CreateCollection, DeleteCollection, ErrorCode, RenameCollection, Storage, TransferClip,
    TransferClips, TransferMode,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

const MAX_COLLECTIONS: u32 = 64;

pub(crate) fn collection_name(value: &str) -> Result<(String, String), AppError> {
    let name = value.trim();
    if name.is_empty()
        || name.len() > 256
        || name.chars().count() > 64
        || name.chars().any(char::is_control)
    {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok((name.into(), name.to_lowercase()))
}

pub(crate) fn ensure_collection(connection: &Connection, id: &str) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    if !connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted=0)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(AppError::new(ErrorCode::NotFound));
    }
    Ok(())
}

impl Storage {
    pub fn collections(&self) -> Result<CollectionsSnapshot, AppError> {
        let mut statement = self.connection.prepare(
            "SELECT id,name,revision FROM collections WHERE deleted=0 ORDER BY sequence,id",
        )?;
        let items = statement
            .query_map([], |r| {
                Ok(CollectionItem {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    revision: r.get::<_, i64>(2)?.to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CollectionsSnapshot {
            items,
            revision: self.history_revision()?,
        })
    }

    pub fn create_collection(
        &mut self,
        request: CreateCollection,
    ) -> Result<CollectionItem, AppError> {
        let (name, key) = collection_name(&request.name)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: u32 = tx.query_row(
            "SELECT count(*) FROM collections WHERE deleted=0",
            [],
            |r| r.get(0),
        )?;
        if count >= MAX_COLLECTIONS {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE name_key=?1 AND deleted=0)",
            [&key],
            |r| r.get(0),
        )?;
        if duplicate {
            return Err(AppError::new(ErrorCode::DuplicateName));
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO collections(id,name,name_key,sequence,revision) VALUES(?1,?2,?3,(SELECT revision+1 FROM history_meta WHERE singleton=1),0)", params![id,name,key])?;
        check_change_budget(&tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(CollectionItem {
            id,
            name,
            revision: "0".into(),
        })
    }

    pub fn rename_collection(
        &mut self,
        request: RenameCollection,
    ) -> Result<CollectionItem, AppError> {
        let (name, key) = collection_name(&request.name)?;
        let revision = mutation_revision(&request.expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_collection(&tx, &request.id)?;
        let (current, name_before): (i64, String) = tx.query_row(
            "SELECT revision,name FROM collections WHERE id=?1",
            [&request.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if current != revision || name == name_before {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE name_key=?1 AND id<>?2 AND deleted=0)",
            params![key, request.id],
            |r| r.get(0),
        )?;
        if duplicate {
            return Err(AppError::new(ErrorCode::DuplicateName));
        }
        tx.execute("UPDATE collections SET name=?1,name_key=?2,revision=revision+1 WHERE id=?3 AND revision=?4", params![name,key,request.id,revision])?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(CollectionItem {
            id: request.id,
            name,
            revision: (revision + 1).to_string(),
        })
    }

    pub fn transfer_clip(
        &mut self,
        request: TransferClip,
    ) -> Result<ClipTransferSnapshot, AppError> {
        let protected = serde_json::to_string(&[&request.id])
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        self.transfer_clip_protecting(request, &protected)
    }

    pub fn delete_collection(&mut self, request: DeleteCollection) -> Result<u32, AppError> {
        let revision = mutation_revision(&request.expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_collection(&tx, &request.id)?;
        if tx.execute(
            "UPDATE collections SET deleted=1,revision=revision+1 WHERE id=?1 AND revision=?2",
            params![request.id, revision],
        )? != 1
        {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        let removed = tx.execute(
            "UPDATE clips SET deleted=1,revision=revision+1 WHERE collection=?1 AND deleted=0",
            [&request.id],
        )?;
        // The whole collection must remain recoverable within the retained trash budget.
        let deleted: u32 =
            tx.query_row("SELECT count(*) FROM clips WHERE deleted=1", [], |row| {
                row.get(0)
            })?;
        if deleted > crate::history::MAX_TRASH_ITEMS {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        check_change_budget(&tx)?;
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(removed as u32)
    }

    pub fn transfer_clips(
        &mut self,
        request: TransferClips,
    ) -> Result<ClipTransferBatch, AppError> {
        if !(1..=1000).contains(&request.clips.len()) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let ids: std::collections::HashSet<_> = request.clips.iter().map(|clip| &clip.id).collect();
        if ids.len() != request.clips.len() {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let protected =
            serde_json::to_string(&ids).map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        let mut result = ClipTransferBatch {
            completed: vec![],
            failure: None,
        };
        for clip in request.clips {
            match self.transfer_clip_protecting(
                TransferClip {
                    id: clip.id,
                    expected_revision: clip.expected_revision,
                    collection_id: request.collection_id.clone(),
                    mode: request.mode,
                },
                &protected,
            ) {
                Ok(transfer) => result.completed.push(transfer),
                Err(error) => {
                    result.failure = Some(error);
                    break;
                }
            }
        }
        Ok(result)
    }

    fn transfer_clip_protecting(
        &mut self,
        request: TransferClip,
        protected: &str,
    ) -> Result<ClipTransferSnapshot, AppError> {
        if uuid::Uuid::parse_str(&request.id).is_err()
            || (request.mode == TransferMode::Copy && request.collection_id.is_none())
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let revision = mutation_revision(&request.expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT revision,collection FROM clips WHERE id=?1 AND deleted=0",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (current, collection) = source.ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
        if current != revision {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        if let Some(id) = &request.collection_id {
            ensure_collection(&tx, id)?;
        }
        let (id, next_revision) = match request.mode {
            TransferMode::Move => {
                if collection == request.collection_id {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                tx.execute("UPDATE clips SET collection=?1,revision=revision+1 WHERE id=?2 AND revision=?3", params![request.collection_id,request.id,revision])?;
                (request.id, revision + 1)
            }
            TransferMode::Copy => {
                let policy = read_capture_policy(&tx)?;
                let payload = Self::read_active_clip(&tx, &request.id, Some(revision))?;
                if !allowed_by_capture_policy(&payload, &policy)? {
                    return Err(AppError::new(ErrorCode::Excluded));
                }
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute("INSERT INTO clips(id,kind,text,data,width,height,file_count,fingerprint,origin,pinned,deleted,collection,captured_at,last_used_at,sequence,revision)
                    SELECT ?1,kind,text,data,width,height,file_count,fingerprint,'manual',0,0,?2,?3,NULL,(SELECT revision+1 FROM history_meta WHERE singleton=1),0 FROM clips WHERE id=?4", params![id,request.collection_id,now(),request.id])?;
                crate::replica::copy_file_reference(&tx, &request.id, &id)?;
                // A copy must preserve its source even when capacity is tight.
                prune_protecting(&tx, policy.max_items, Some(&id), protected)?;
                check_change_budget(&tx)?;
                (id, 0)
            }
        };
        tx.execute(
            "UPDATE history_meta SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(ClipTransferSnapshot {
            id,
            revision: next_revision.to_string(),
            collection_id: request.collection_id,
            mode: request.mode,
        })
    }
}
