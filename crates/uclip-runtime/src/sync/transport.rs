use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
};

use serde::{de::DeserializeOwned, Serialize};
use uclip_core::{
    AppError, CoreService, ErrorCode, ReplicaBody, ReplicaEntry, ReplicaStamp, ReplicaType,
    SyncRemoteProgress,
};

pub(super) const MAX_RECORD: usize = 52 * 1024 * 1024;
pub(super) const MAX_INVENTORY: usize = 48 * 1024 * 1024;
pub(super) const MAX_STAMPS: usize = 25_000;
pub(super) const UPLOAD_BATCH_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn block<T>(future: impl Future<Output = T>) -> T {
    futures_executor::block_on(future)
}

pub(super) fn unavailable() -> AppError {
    AppError::new(ErrorCode::SyncConnection)
}

pub(super) fn invalid() -> AppError {
    AppError::new(ErrorCode::SyncData)
}

pub(super) fn limit() -> AppError {
    AppError::new(ErrorCode::LimitExceeded)
}

pub(super) fn encode<T: Serialize + ?Sized>(
    value: &T,
    limit_bytes: usize,
) -> Result<Vec<u8>, AppError> {
    let data = rmp_serde::to_vec(value).map_err(|_| invalid())?;
    if data.len() > limit_bytes {
        return Err(limit());
    }
    Ok(data)
}

pub(super) fn decode<T: DeserializeOwned>(data: &[u8]) -> Result<T, AppError> {
    let mut reader = std::io::Cursor::new(data);
    let mut decoder = rmp_serde::Deserializer::new(&mut reader);
    decoder.set_max_depth(24);
    let value = serde::Deserialize::deserialize(&mut decoder).map_err(|_| invalid())?;
    if reader.position() != data.len() as u64 {
        return Err(invalid());
    }
    Ok(value)
}

pub(super) enum FetchSource<'a> {
    Ready,
    Shared(&'a str),
    Single,
}

pub(super) trait ReplicaTransport {
    fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError>;
    /// Whether permanent removals (`stamp.removed`) are carried by `commit()`
    /// without per-entry `fetch()` or `publish()` object transfers.
    fn inline_removals(&self) -> bool {
        false
    }
    fn fetch_source(&self, _stamp: &ReplicaStamp) -> FetchSource<'_> {
        FetchSource::Single
    }
    fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError>;
    fn fetch_batch_size(&self) -> usize {
        1
    }
    fn fetch_batch(
        &mut self,
        stamps: &[&ReplicaStamp],
    ) -> Vec<Result<Option<ReplicaEntry>, AppError>> {
        stamps.iter().map(|stamp| self.fetch(stamp)).collect()
    }
    fn contains_body(&mut self, _stamp: &ReplicaStamp) -> Result<bool, AppError> {
        Ok(true)
    }
    fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError>;
    fn publish_batch_size(&self) -> usize {
        1
    }
    fn publish_batch(&mut self, entries: Vec<ReplicaEntry>) -> Vec<Result<(), AppError>> {
        entries
            .into_iter()
            .map(|entry| self.publish(entry))
            .collect()
    }
    fn compaction_candidates(&self, _available: &[ReplicaStamp]) -> Vec<ReplicaStamp> {
        Vec::new()
    }
    fn repack(&mut self, _entries: Vec<ReplicaEntry>) -> Result<(), AppError> {
        Ok(())
    }
    /// Archive transports can publish available versions before the round is complete.
    fn commit(&mut self, available: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError>;
}

fn payload_bytes(entry: &ReplicaEntry) -> usize {
    match &entry.body {
        Some(ReplicaBody::Clip { text, data, .. }) => text.len() + data.len(),
        _ => 0,
    }
}

fn finish_batch(
    transport: &mut impl ReplicaTransport,
    batch: &mut Vec<ReplicaEntry>,
    available: &mut Vec<ReplicaStamp>,
    failed: &mut Option<AppError>,
) -> bool {
    if batch.is_empty() {
        return true;
    }
    let stamps: Vec<_> = batch.iter().map(|entry| entry.stamp.clone()).collect();
    let results = transport.publish_batch(std::mem::take(batch));
    if results.len() != stamps.len() {
        *failed = Some(invalid());
        return false;
    }
    let mut keep_going = true;
    for (stamp, result) in stamps.into_iter().zip(results) {
        match result {
            Ok(()) => available.push(stamp),
            Err(error) if matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) => {
                failed.get_or_insert(error);
            }
            Err(error) => {
                if keep_going {
                    *failed = Some(error);
                    keep_going = false;
                }
            }
        }
    }
    keep_going
}

/// All backends run the same pull, causal merge and push through the SQLite owner.
pub(super) fn reconcile(
    core: &CoreService,
    transport: &mut impl ReplicaTransport,
) -> Result<bool, AppError> {
    reconcile_with_progress(core, transport, |_| {}).map(|records| records.is_some())
}

pub(super) fn reconcile_with_progress(
    core: &CoreService,
    transport: &mut impl ReplicaTransport,
    mut progress: impl FnMut(SyncRemoteProgress),
) -> Result<Option<u32>, AppError> {
    let mut pending = false;
    let mut failed = None;
    progress(SyncRemoteProgress::Checking);
    let mut remote = transport.inventory()?;
    // Try the latest first. A missing body cannot suppress an available predecessor.
    remote.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| b.compare_version(a)));
    let mut keys = HashSet::new();
    let mut unique = Vec::with_capacity(remote.len());
    for stamp in &remote {
        stamp.validate().map_err(|_| invalid())?;
        if keys.insert(stamp.key()) {
            unique.push(stamp);
        }
    }
    let mut known = block(core.replica_inventory())?
        .into_iter()
        .map(|stamp| (stamp.id.clone(), stamp))
        .collect::<BTreeMap<_, _>>();
    let inline_removals = transport.inline_removals();
    let mut waves = BTreeMap::<_, Vec<_>>::new();
    for versions in unique.chunk_by(|first, second| first.id == second.id) {
        for (priority, &stamp) in versions.iter().enumerate() {
            if !known
                .get(&stamp.id)
                .is_some_and(|local| local.supersedes(stamp))
            {
                waves
                    .entry((stamp.id.kind, priority))
                    .or_default()
                    .push(stamp);
            }
        }
    }
    let total = waves.values().map(Vec::len).sum::<usize>();
    let mut processed = 0;
    for (_, mut wave) in waves {
        let count = wave.len();
        wave.retain(|stamp| {
            !known
                .get(&stamp.id)
                .is_some_and(|local| local.supersedes(stamp))
        });
        processed += count - wave.len();
        // Collections must still apply before clips, and a predecessor is only read
        // after its successor has been tried. Within a wave, read each pack together.
        wave.sort_by_cached_key(|stamp| match transport.fetch_source(stamp) {
            FetchSource::Ready => (0, ""),
            FetchSource::Shared(name) => (1, name),
            FetchSource::Single => (2, ""),
        });
        let mut remaining = wave.as_slice();
        while !remaining.is_empty() {
            progress(SyncRemoteProgress::Downloading {
                processed: processed as u32,
                total: total as u32,
            });
            let max_reads = transport.fetch_batch_size().max(1);
            let max_entries = max_reads.saturating_mul(256);
            let ready_only = matches!(transport.fetch_source(remaining[0]), FetchSource::Ready);
            let mut shared = HashSet::new();
            let mut reads = 0;
            let count = remaining
                .iter()
                .take(max_entries)
                .take_while(|&&stamp| {
                    let source = transport.fetch_source(stamp);
                    if ready_only {
                        return matches!(source, FetchSource::Ready);
                    }
                    let needs_read = match source {
                        FetchSource::Ready => false,
                        FetchSource::Shared(name) => shared.insert(name),
                        FetchSource::Single => true,
                    };
                    reads += usize::from(needs_read);
                    reads <= max_reads
                })
                .count();
            let (batch, rest) = remaining.split_at(count);
            let entries = transport.fetch_batch(batch);
            if entries.len() != batch.len() {
                return Err(invalid());
            }
            for (&remote, entry) in batch.iter().zip(entries) {
                let applied = match entry {
                    Ok(Some(entry)) if entry.stamp == *remote => {
                        block(core.apply_replica_entry(entry)).map(|_| {
                            known.insert(remote.id.clone(), remote.clone());
                        })
                    }
                    Ok(Some(_)) => Err(invalid()),
                    Ok(None) => {
                        pending = true;
                        continue;
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = applied {
                    match error.code {
                        ErrorCode::SyncData
                        | ErrorCode::InvalidInput
                        | ErrorCode::LimitExceeded => {
                            failed.get_or_insert(error);
                        }
                        _ => return Err(error),
                    }
                }
            }
            processed += batch.len();
            remaining = rest;
        }
    }
    let mut available = Vec::new();
    let mut local = block(core.replica_inventory())?;
    // Group records deterministically by ID so packed files stay contiguous for downloads.
    local.sort_by(|a, b| a.id.cmp(&b.id));
    let total = local.len() as u32;
    let batch_size = transport.publish_batch_size().max(1);
    let mut batch = Vec::new();
    let mut batch_bytes = 0usize;
    for (index, stamp) in local.into_iter().enumerate() {
        if inline_removals && stamp.removed {
            available.push(stamp);
            continue;
        }
        let needed = (|| {
            if keys.contains(&stamp.key()) && transport.contains_body(&stamp)? {
                return Ok((true, None));
            }
            Ok::<_, AppError>((false, block(core.replica_entry(stamp.clone()))?))
        })();
        match needed {
            Ok((true, _)) => available.push(stamp),
            Ok((false, None)) => pending = true,
            Ok((false, Some(entry))) => {
                progress(SyncRemoteProgress::Uploading {
                    processed: (index - batch.len()) as u32,
                    total,
                });
                let bytes = payload_bytes(&entry);
                if batch_bytes + bytes > UPLOAD_BATCH_BYTES {
                    if !finish_batch(transport, &mut batch, &mut available, &mut failed) {
                        break;
                    }
                    batch_bytes = 0;
                }
                batch.push(entry);
                batch_bytes += bytes;
                if batch.len() >= batch_size || batch_bytes >= UPLOAD_BATCH_BYTES {
                    if !finish_batch(transport, &mut batch, &mut available, &mut failed) {
                        break;
                    }
                    batch_bytes = 0;
                }
            }
            Err(error) if matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) => {
                failed.get_or_insert(error);
            }
            Err(error) => {
                finish_batch(transport, &mut batch, &mut available, &mut failed);
                failed = Some(error);
                break;
            }
        }
    }
    finish_batch(transport, &mut batch, &mut available, &mut failed);
    available.sort_by(|a, b| (&a.id, &a.writer, a.counter).cmp(&(&b.id, &b.writer, b.counter)));
    let complete = !pending && failed.is_none();
    if complete {
        let candidates = transport.compaction_candidates(&available);
        if !candidates.is_empty() {
            let mut entries = Vec::with_capacity(candidates.len());
            for stamp in candidates {
                if let Ok(Some(entry)) = block(core.replica_entry(stamp)) {
                    entries.push(entry);
                }
            }
            if !entries.is_empty() {
                // Compaction is best-effort; original object references remain valid if it fails.
                let _ = transport.repack(entries);
            }
        }
    }
    let records = available
        .iter()
        .filter(|stamp| stamp.id.kind == ReplicaType::Clip && !stamp.removed)
        .count() as u32;
    progress(SyncRemoteProgress::Finalizing);
    let committed = transport.commit(available, complete);
    match failed {
        Some(error) => Err(error),
        None => committed.map(|_| complete.then_some(records)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenObject {
        offered: Vec<ReplicaStamp>,
        readable: Option<ReplicaEntry>,
    }

    impl ReplicaTransport for BrokenObject {
        fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
            Ok(self.offered.clone())
        }

        fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
            if self
                .readable
                .as_ref()
                .is_some_and(|entry| &entry.stamp == stamp)
            {
                return Ok(self.readable.take());
            }
            Err(invalid())
        }

        fn publish(&mut self, _: ReplicaEntry) -> Result<(), AppError> {
            Ok(())
        }

        fn commit(&mut self, _: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError> {
            assert!(!complete, "a round with a missing body cannot be confirmed");
            Ok(())
        }
    }

    #[test]
    fn damaged_record_does_not_block_an_independent_item_or_confirm_the_round() {
        let directory = tempfile::tempdir().unwrap();
        let source = block(CoreService::open(directory.path().join("source"))).unwrap();
        let local = block(CoreService::open(directory.path().join("local"))).unwrap();
        block(source.enroll_replica()).unwrap();
        block(local.enroll_replica()).unwrap();
        let missing = block(source.create_text("missing body".into())).unwrap();
        let readable = block(source.create_text("independent item".into())).unwrap();
        let offered = block(source.replica_inventory()).unwrap();
        let good = offered
            .iter()
            .find(|stamp| stamp.id.id == readable)
            .unwrap()
            .clone();
        let mut transport = BrokenObject {
            offered,
            readable: block(source.replica_entry(good)).unwrap(),
        };
        assert!(
            matches!(reconcile(&local, &mut transport), Err(error) if error.code == ErrorCode::SyncData)
        );
        assert!(block(local.read_clip(readable)).is_ok());
        assert!(block(local.read_clip(missing)).is_err());
        block(source.shutdown()).unwrap();
        block(local.shutdown()).unwrap();
    }

    struct BatchedTransport {
        offered: Vec<ReplicaStamp>,
        entries: BTreeMap<String, Vec<u8>>,
        batch_size: usize,
        fetched: Vec<Vec<String>>,
    }

    impl ReplicaTransport for BatchedTransport {
        fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
            Ok(self.offered.clone())
        }

        fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
            self.fetched.push(vec![stamp.key()]);
            self.entries
                .get(&stamp.key())
                .ok_or_else(invalid)
                .and_then(|data| decode(data).map(Some))
        }

        fn fetch_batch_size(&self) -> usize {
            self.batch_size
        }

        fn fetch_batch(
            &mut self,
            stamps: &[&ReplicaStamp],
        ) -> Vec<Result<Option<ReplicaEntry>, AppError>> {
            self.fetched
                .push(stamps.iter().map(|stamp| stamp.key()).collect());
            stamps
                .iter()
                .map(|stamp| {
                    self.entries
                        .get(&stamp.key())
                        .ok_or_else(invalid)
                        .and_then(|data| decode(data).map(Some))
                })
                .collect()
        }

        fn publish(&mut self, _: ReplicaEntry) -> Result<(), AppError> {
            Ok(())
        }

        fn commit(&mut self, _: Vec<ReplicaStamp>, _: bool) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[test]
    fn batched_pull_separates_versions_of_the_same_record_and_handles_zero_batch_size() {
        let directory = tempfile::tempdir().unwrap();
        let source = block(CoreService::open(directory.path().join("source"))).unwrap();
        let local = block(CoreService::open(directory.path().join("local"))).unwrap();
        block(source.enroll_replica()).unwrap();
        block(local.enroll_replica()).unwrap();

        let id = block(source.create_text("version 1".into())).unwrap();
        let v1_stamp = block(source.replica_inventory()).unwrap().remove(0);
        let v1_entry = block(source.replica_entry(v1_stamp.clone()))
            .unwrap()
            .unwrap();
        block(source.change_clip_pin(uclip_core::ChangeClipPin {
            id: id.clone(),
            expected_revision: "0".into(),
            pinned: true,
        }))
        .unwrap();
        let v2_stamp = block(source.replica_inventory()).unwrap().remove(0);
        let v2_entry = block(source.replica_entry(v2_stamp.clone()))
            .unwrap()
            .unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(v1_stamp.key(), encode(&v1_entry, MAX_RECORD).unwrap());
        entries.insert(v2_stamp.key(), encode(&v2_entry, MAX_RECORD).unwrap());
        let mut transport = BatchedTransport {
            offered: vec![v1_stamp.clone(), v2_stamp.clone()],
            entries,
            batch_size: 4,
            fetched: Vec::new(),
        };

        assert!(reconcile(&local, &mut transport).unwrap());
        // Only the latest version should be fetched; its predecessor is skipped once v2 applies.
        assert_eq!(transport.fetched, vec![vec![v2_stamp.key()]]);

        // Zero batch size must fall back to 1 instead of looping forever.
        let other = block(CoreService::open(directory.path().join("other"))).unwrap();
        block(other.enroll_replica()).unwrap();
        transport.batch_size = 0;
        transport.fetched.clear();
        assert!(reconcile(&other, &mut transport).unwrap());
        assert_eq!(transport.fetched, vec![vec![v2_stamp.key()]]);

        for core in [source, local, other] {
            block(core.shutdown()).unwrap();
        }
    }

    struct NonInlineRemovalTransport {
        published: Vec<ReplicaEntry>,
    }

    impl ReplicaTransport for NonInlineRemovalTransport {
        fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
            Ok(Vec::new())
        }

        fn fetch(&mut self, _: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
            Ok(None)
        }

        fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError> {
            self.published.push(entry);
            Ok(())
        }

        fn commit(&mut self, _: Vec<ReplicaStamp>, _: bool) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[test]
    fn non_inline_transports_publish_permanent_removals_to_peers() {
        let directory = tempfile::tempdir().unwrap();
        let client = block(CoreService::open(directory.path().join("client"))).unwrap();
        block(client.enroll_replica()).unwrap();
        let clip = block(client.create_text("delete me over LAN".into())).unwrap();
        block(client.change_clip_deletion(uclip_core::ChangeClipDeletion {
            id: clip,
            expected_revision: "0".into(),
            deleted: true,
        }))
        .unwrap();
        block(
            client.purge_trash(uclip_core::PurgeTrash::All(uclip_core::EmptyTrash {
                expected_revision: block(client.history_revision()).unwrap(),
            })),
        )
        .unwrap();

        let mut transport = NonInlineRemovalTransport {
            published: Vec::new(),
        };
        assert!(reconcile(&client, &mut transport).unwrap());
        assert_eq!(transport.published.len(), 1);
        assert!(transport.published[0].stamp.removed);
        assert!(transport.published[0].body.is_none());
        block(client.shutdown()).unwrap();
    }
}
