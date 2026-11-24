use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
};

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use uclip_core::{AppError, ErrorCode, ReplicaEntry, ReplicaStamp};
use uuid::Uuid;

use super::transport::{
    decode, encode, invalid, limit, unavailable, ReplicaTransport, MAX_INVENTORY, MAX_RECORD,
    MAX_STAMPS,
};

const MAGIC: &[u8] = b"UClip encrypted sync v1\0";
const MANIFEST_VERSION: u8 = 4;
const MAX_MANIFESTS: usize = 256;
const OVERHEAD: usize = MAGIC.len() + 24 + 16;

#[derive(Clone, Copy)]
pub(super) enum Area {
    Manifests,
    Objects,
}

impl Area {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Manifests => ".idx",
            Self::Objects => ".clip",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Manifests => "manifests",
            Self::Objects => "objects",
        }
    }
}

pub(super) struct ObjectUpload {
    pub name: String,
    pub data: Vec<u8>,
}

/// The final name must only become visible once the complete object is durable on the backend.
pub(super) trait ArchiveStore {
    fn list(&mut self, area: Area) -> Result<Vec<String>, AppError>;
    fn object_exists(&mut self, name: &str) -> Result<bool, AppError>;
    fn read(&mut self, area: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError>;
    fn create(&mut self, area: Area, name: &str, data: Vec<u8>) -> Result<bool, AppError>;
    fn object_batch_size(&self) -> usize {
        1
    }
    fn create_objects(
        &mut self,
        objects: Vec<ObjectUpload>,
        stopped: Option<&AtomicBool>,
    ) -> Vec<Result<bool, AppError>> {
        objects
            .into_iter()
            .map(|object| {
                if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err(unavailable());
                }
                self.create(Area::Objects, &object.name, object.data)
            })
            .collect()
    }
    fn remove_manifest(&mut self, name: &str);
}

enum ObjectLookup {
    Unchecked,
    Listed(HashSet<String>),
    Probe,
}

type Manifests = BTreeMap<String, (u64, String)>;

pub(super) struct Archive<S> {
    store: S,
    key: [u8; 32],
    device: String,
    generation: Option<u64>,
    own: Vec<ReplicaStamp>,
    objects: ObjectLookup,
    previous: Vec<String>,
}

impl<S: ArchiveStore> Archive<S> {
    pub fn new(store: S, key: [u8; 32], device: String) -> Self {
        Self {
            store,
            device,
            key,
            generation: None,
            own: Vec::new(),
            objects: ObjectLookup::Unchecked,
            previous: Vec::new(),
        }
    }

    fn seal(&self, name: &str, data: &[u8]) -> Result<Vec<u8>, AppError> {
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(|_| unavailable())?;
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let data = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: data,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| invalid())?;
        let mut result = Vec::with_capacity(MAGIC.len() + nonce.len() + data.len());
        result.extend_from_slice(MAGIC);
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&data);
        Ok(result)
    }

    fn open(&self, name: &str, data: &[u8]) -> Result<Vec<u8>, AppError> {
        let data = data
            .strip_prefix(MAGIC)
            .filter(|data| data.len() >= 24 + 16)
            .ok_or_else(invalid)?;
        let (nonce, ciphertext) = data.split_at(24);
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| AppError::new(ErrorCode::SyncAuthentication))
    }

    fn latest(&mut self) -> Result<Manifests, AppError> {
        let mut latest = BTreeMap::<String, (u64, String)>::new();
        self.previous.clear();
        for name in self.store.list(Area::Manifests)? {
            let Some((device, generation)) = manifest_parts(&name) else {
                continue;
            };
            if device == self.device {
                self.previous.push(name.clone());
            }
            let item = latest
                .entry(device.into())
                .or_insert_with(|| (generation, name.clone()));
            if item.0 < generation {
                *item = (generation, name);
            }
        }
        if latest.len() > MAX_MANIFESTS {
            return Err(limit());
        }
        Ok(latest)
    }

    fn verify_object(&mut self, entry: &ReplicaEntry) -> Result<(), AppError> {
        let existing = self.fetch(&entry.stamp)?.ok_or_else(invalid)?;
        if existing.stamp != entry.stamp || existing.body != entry.body {
            return Err(invalid());
        }
        Ok(())
    }
}

fn manifest_parts(name: &str) -> Option<(&str, u64)> {
    let name = name.strip_suffix(".idx")?;
    let (device, generation) = name.rsplit_once('-')?;
    if generation.len() != 16 || Uuid::parse_str(device).ok()?.to_string() != device {
        return None;
    }
    Some((device, u64::from_str_radix(generation, 16).ok()?))
}

fn object_name(stamp: &ReplicaStamp) -> String {
    format!("{}.clip", stamp.key())
}

impl<S: ArchiveStore> ReplicaTransport for Archive<S> {
    fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
        let mut unique = HashSet::new();
        let mut result = Vec::new();
        self.objects = ObjectLookup::Unchecked;
        for (device, (generation, name)) in self.latest()? {
            let data = self
                .store
                .read(Area::Manifests, &name, MAX_INVENTORY + OVERHEAD)?
                .ok_or_else(unavailable)?;
            let plaintext = self.open(&name, &data)?;
            let (version, stamps): (u8, Vec<ReplicaStamp>) = decode(&plaintext)?;
            if version != MANIFEST_VERSION {
                return Err(invalid());
            }
            if stamps.len() > MAX_STAMPS {
                return Err(limit());
            }
            for stamp in &stamps {
                stamp.validate().map_err(|_| invalid())?;
                if unique.insert(stamp.key()) {
                    result.push(stamp.clone());
                    if result.len() > MAX_STAMPS * 2 {
                        return Err(limit());
                    }
                }
            }
            if device == self.device {
                self.own = stamps;
                self.generation = Some(generation);
            }
        }
        Ok(result)
    }

    fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
        let name = object_name(stamp);
        let Some(data) = self
            .store
            .read(Area::Objects, &name, MAX_RECORD + OVERHEAD)?
        else {
            return Err(invalid());
        };
        let entry = decode(&self.open(&name, &data).map_err(|_| invalid())?)?;
        Ok(Some(entry))
    }

    fn contains_body(&mut self, stamp: &ReplicaStamp) -> Result<bool, AppError> {
        if matches!(self.objects, ObjectLookup::Unchecked) {
            self.objects = match self.store.list(Area::Objects) {
                Ok(names) => ObjectLookup::Listed(names.into_iter().collect()),
                Err(error) if error.code == ErrorCode::LimitExceeded => ObjectLookup::Probe,
                Err(error) => return Err(error),
            };
        }
        let name = object_name(stamp);
        match &self.objects {
            ObjectLookup::Listed(names) => Ok(names.contains(&name)),
            _ => self.store.object_exists(&name),
        }
    }

    fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError> {
        let name = object_name(&entry.stamp);
        if !self.contains_body(&entry.stamp)? {
            let body = self.seal(&name, &encode(&entry, MAX_RECORD)?)?;
            if self.store.create(Area::Objects, &name, body)? {
                return Ok(());
            }
        }
        // An earlier round may have uploaded the body without committing its manifest.
        self.verify_object(&entry)
    }

    fn publish_batch_size(&self) -> usize {
        self.store.object_batch_size()
    }

    fn publish_batch(&mut self, entries: Vec<ReplicaEntry>) -> Vec<Result<(), AppError>> {
        if entries.len() <= 1 {
            return entries
                .into_iter()
                .map(|entry| self.publish(entry))
                .collect();
        }
        let mut results = vec![None; entries.len()];
        let mut indices = Vec::new();
        let mut objects = Vec::new();
        let mut failure: Option<AppError> = None;
        for (index, entry) in entries.iter().enumerate() {
            if let Some(error) = &failure {
                results[index] = Some(Err(error.clone()));
                continue;
            }
            let stage = (|| -> Result<Option<ObjectUpload>, AppError> {
                if self.contains_body(&entry.stamp)? {
                    self.verify_object(entry)?;
                    return Ok(None);
                }
                let name = object_name(&entry.stamp);
                let data = self.seal(&name, &encode(entry, MAX_RECORD)?)?;
                Ok(Some(ObjectUpload { name, data }))
            })();
            match stage {
                Ok(Some(object)) => {
                    indices.push(index);
                    objects.push(object);
                }
                Ok(None) => results[index] = Some(Ok(())),
                Err(error) => {
                    if !matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) {
                        failure = Some(error.clone());
                    }
                    results[index] = Some(Err(error));
                }
            }
        }
        for (index, result) in indices
            .into_iter()
            .zip(self.store.create_objects(objects, None))
        {
            results[index] = Some(result.and_then(|created| {
                if created {
                    Ok(())
                } else {
                    self.verify_object(&entries[index])
                }
            }));
        }
        results
            .into_iter()
            .map(|result| result.unwrap_or_else(|| Err(invalid())))
            .collect()
    }

    fn commit(&mut self, mut available: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError> {
        if !complete {
            // Keep existing references until a published successor or a complete round replaces them.
            let mut published = BTreeMap::<_, Vec<_>>::new();
            for stamp in &available {
                published.entry(&stamp.id).or_default().push(stamp);
            }
            let retained = self.own.iter().filter(|stamp| {
                !published.get(&stamp.id).is_some_and(|versions| {
                    versions.iter().any(|version| version.supersedes(stamp))
                })
            });
            let retained = retained.cloned().collect::<Vec<_>>();
            available.extend(retained);
            available
                .sort_by(|a, b| (&a.id, &a.writer, a.counter).cmp(&(&b.id, &b.writer, b.counter)));
        }
        if self.own == available && (self.generation.is_some() || !complete) {
            return Ok(());
        }
        if available.len() > MAX_STAMPS {
            return Err(limit());
        }
        let generation = self
            .generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(limit)?;
        let name = format!("{}-{generation:016x}.idx", self.device);
        let body = self.seal(
            &name,
            &encode(&(MANIFEST_VERSION, &available), MAX_INVENTORY)?,
        )?;
        if !self.store.create(Area::Manifests, &name, body)? {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        for previous in &self.previous {
            if previous != &name {
                self.store.remove_manifest(previous);
            }
        }
        self.own = available;
        self.generation = Some(generation);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::btree_map::Entry;

    use uclip_core::{ChangeClipPin, ClipPayload, CoreService, SyncRemoteProgress};

    use super::*;
    use crate::sync::transport::{block, reconcile, reconcile_with_progress};

    #[derive(Default)]
    struct MemoryStore {
        entries: BTreeMap<String, Vec<u8>>,
        rejected: Option<String>,
        writes: Vec<String>,
    }

    impl ArchiveStore for &mut MemoryStore {
        fn list(&mut self, area: Area) -> Result<Vec<String>, AppError> {
            Ok(self
                .entries
                .keys()
                .filter(|name| name.ends_with(area.suffix()))
                .cloned()
                .collect())
        }

        fn object_exists(&mut self, name: &str) -> Result<bool, AppError> {
            Ok(self.entries.contains_key(name))
        }

        fn read(&mut self, _: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError> {
            let data = self.entries.get(name);
            if data.is_some_and(|data| data.len() > bound) {
                return Err(limit());
            }
            Ok(data.cloned())
        }

        fn create(&mut self, _: Area, name: &str, data: Vec<u8>) -> Result<bool, AppError> {
            self.writes.push(name.into());
            if self.rejected.as_deref() == Some(name) {
                return Err(limit());
            }
            match self.entries.entry(name.into()) {
                Entry::Occupied(_) => Ok(false),
                Entry::Vacant(slot) => {
                    slot.insert(data);
                    Ok(true)
                }
            }
        }

        fn remove_manifest(&mut self, name: &str) {
            self.entries.remove(name);
        }
    }

    #[test]
    fn resumes_unindexed_objects_and_commits_independent_uploads_when_a_record_is_rejected() {
        use SyncRemoteProgress::*;

        let directory = tempfile::tempdir().unwrap();
        let core = block(CoreService::open(directory.path().join("local"))).unwrap();
        block(core.enroll_replica()).unwrap();
        for text in ["first", "second", "third"] {
            block(core.create_text(text.into())).unwrap();
        }
        let device = block(core.replica_device()).unwrap();
        let stamps = block(core.replica_inventory()).unwrap();
        let rejected = object_name(&stamps[0]);
        let existing = object_name(&stamps[1]);
        let mut store = MemoryStore::default();
        let mut archive = Archive::new(&mut store, [23; 32], device.clone());
        let unindexed = block(core.replica_entry(stamps[1].clone()))
            .unwrap()
            .unwrap();
        archive.publish(unindexed).unwrap();
        store.writes.clear();
        store.rejected = Some(rejected.clone());

        let mut phases = Vec::new();
        let mut archive = Archive::new(&mut store, [23; 32], device.clone());
        let result = reconcile_with_progress(&core, &mut archive, |phase| phases.push(phase));
        assert!(matches!(result, Err(error) if error.code == ErrorCode::LimitExceeded));
        assert_eq!(
            phases,
            [
                Checking,
                Uploading {
                    processed: 0,
                    total: 3
                },
                Uploading {
                    processed: 1,
                    total: 3
                },
                Uploading {
                    processed: 2,
                    total: 3
                },
                Finalizing,
            ],
        );
        assert!(archive.inventory().unwrap() == stamps[1..]);
        assert!(!store.writes.contains(&existing));
        assert!(store.writes.contains(&object_name(&stamps[2])));

        store.writes.clear();
        store.rejected = None;
        let mut archive = Archive::new(&mut store, [23; 32], device);
        assert_eq!(
            reconcile_with_progress(&core, &mut archive, |_| {}).unwrap(),
            Some(3)
        );
        assert_eq!(
            store
                .writes
                .iter()
                .filter(|name| name.ends_with(".clip"))
                .collect::<Vec<_>>(),
            [&rejected],
        );
        block(core.shutdown()).unwrap();
    }

    #[test]
    fn rejects_manifest_versions_other_than_the_current_format() {
        let device = Uuid::new_v4().to_string();
        for version in [MANIFEST_VERSION - 1, MANIFEST_VERSION + 1] {
            let mut store = MemoryStore::default();
            let mut archive = Archive::new(&mut store, [23; 32], device.clone());
            let name = format!("{device}-{:016x}.idx", 1);
            let plaintext = encode(&(version, Vec::<ReplicaStamp>::new()), MAX_INVENTORY).unwrap();
            let body = archive.seal(&name, &plaintext).unwrap();
            assert!(archive.store.create(Area::Manifests, &name, body).unwrap());
            assert!(matches!(archive.inventory(), Err(error) if error.code == ErrorCode::SyncData));
        }
    }

    #[test]
    fn missing_successor_does_not_hide_a_predecessor_or_independent_updates() {
        let directory = tempfile::tempdir().unwrap();
        let open = |name| {
            let core = block(CoreService::open(directory.path().join(name))).unwrap();
            block(core.enroll_replica()).unwrap();
            core
        };
        let (a, b, c) = (open("a"), open("b"), open("c"));
        let round = |core: &CoreService, store: &mut MemoryStore| {
            let device = block(core.replica_device()).unwrap();
            reconcile(core, &mut Archive::new(store, [23; 32], device))
        };
        let mut store = MemoryStore::default();
        let clip = block(a.create_text("version shared by A and B".into())).unwrap();
        let predecessor = block(a.replica_inventory()).unwrap().remove(0);
        assert!(round(&a, &mut store).unwrap());
        let entry = block(a.replica_entry(predecessor.clone()))
            .unwrap()
            .unwrap();
        block(b.apply_replica_entry(entry)).unwrap();
        block(a.change_clip_pin(ChangeClipPin {
            id: clip.clone(),
            expected_revision: "0".into(),
            pinned: true,
        }))
        .unwrap();
        let successor = block(a.replica_inventory()).unwrap().remove(0);
        assert!(round(&a, &mut store).unwrap());
        assert!(store.entries.remove(&object_name(&predecessor)).is_some());
        assert!(store.entries.remove(&object_name(&successor)).is_some());
        let independent = block(b.create_text("independent update".into())).unwrap();

        assert!(matches!(round(&b, &mut store), Err(error) if error.code == ErrorCode::SyncData));
        assert!(matches!(round(&c, &mut store), Err(error) if error.code == ErrorCode::SyncData));
        assert_eq!(
            block(c.read_clip(clip.clone())).unwrap(),
            ClipPayload::Text("version shared by A and B".into())
        );
        assert_eq!(
            block(c.read_clip(independent)).unwrap(),
            ClipPayload::Text("independent update".into())
        );

        assert!(round(&a, &mut store).unwrap());
        assert!(store.entries.contains_key(&object_name(&successor)));
        for peer in [b, c] {
            assert!(round(&peer, &mut store).unwrap());
            let inventory = block(peer.replica_inventory()).unwrap();
            assert_eq!(inventory.len(), 2);
            assert!(inventory.contains(&successor));
            block(peer.shutdown()).unwrap();
        }
        block(a.shutdown()).unwrap();
    }
}
