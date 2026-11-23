use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
};

use serde::{de::DeserializeOwned, Serialize};
use uclip_core::{AppError, CoreService, ErrorCode, ReplicaEntry, ReplicaId, ReplicaStamp};

pub(super) const MAX_RECORD: usize = 52 * 1024 * 1024;
pub(super) const MAX_INVENTORY: usize = 48 * 1024 * 1024;
pub(super) const MAX_STAMPS: usize = 25_000;

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

pub(super) trait ReplicaTransport {
    fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError>;
    fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError>;
    fn contains_body(&mut self, _stamp: &ReplicaStamp) -> Result<bool, AppError> {
        Ok(true)
    }
    fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError>;
    /// Archive transports can publish available versions before the round is complete.
    fn commit(&mut self, available: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError>;
}

/// All backends run the same pull, causal merge and push through the SQLite owner.
pub(super) fn reconcile(
    core: &CoreService,
    transport: &mut impl ReplicaTransport,
) -> Result<bool, AppError> {
    let mut pending = false;
    let mut failed = None;
    let mut remote = transport.inventory()?;
    // Try the latest first. A missing body cannot suppress an available predecessor.
    remote.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| b.compare_version(a)));
    let mut remote_by_id: BTreeMap<&ReplicaId, Vec<&ReplicaStamp>> = BTreeMap::new();
    let mut keys = HashSet::new();
    for stamp in &remote {
        stamp.validate().map_err(|_| invalid())?;
        if keys.insert(stamp.key()) {
            remote_by_id.entry(&stamp.id).or_default().push(stamp);
        }
    }
    let mut known = block(core.replica_inventory())?
        .into_iter()
        .map(|stamp| (stamp.id.clone(), stamp))
        .collect::<BTreeMap<_, _>>();
    for (id, stamps) in &remote_by_id {
        for remote in stamps {
            if known.get(*id).is_some_and(|local| local.supersedes(remote)) {
                continue;
            }
            let applied = match transport.fetch(remote) {
                Ok(Some(entry)) if entry.stamp == **remote => {
                    block(core.apply_replica_entry(entry)).map(|_| {
                        known.insert((*id).clone(), (*remote).clone());
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
                    ErrorCode::SyncData | ErrorCode::InvalidInput | ErrorCode::LimitExceeded => {
                        failed.get_or_insert(error);
                    }
                    _ => return Err(error),
                }
            }
        }
    }
    let mut available = Vec::new();
    for stamp in block(core.replica_inventory())? {
        if keys.contains(&stamp.key()) && transport.contains_body(&stamp)? {
            available.push(stamp);
            continue;
        }
        match block(core.replica_entry(stamp.clone()))? {
            Some(entry) => {
                transport.publish(entry)?;
                available.push(stamp);
            }
            None => pending = true,
        }
    }
    let complete = !pending && failed.is_none();
    transport.commit(available, complete)?;
    match failed {
        Some(error) => Err(error),
        None => Ok(complete),
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
}
