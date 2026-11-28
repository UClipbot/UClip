use std::{
    collections::{BTreeMap, HashMap, HashSet},
    iter,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uclip_core::{AppError, ErrorCode, ReplicaEntry, ReplicaStamp};
use uuid::Uuid;

use super::transport::{
    decode, encode, invalid, limit, unavailable, FetchSource, ReplicaTransport, MAX_INVENTORY,
    MAX_RECORD, MAX_STAMPS,
};

const MAGIC: &[u8] = b"UClip encrypted sync v1\0";
const MANIFEST_VERSION: u8 = 5;
const PACK_VERSION: u8 = 1;
const MAX_MANIFESTS: usize = 256;
const OVERHEAD: usize = MAGIC.len() + 24 + 16;
// Keep the number of concurrently buffered objects bounded even for large records.
pub(super) const PARALLEL_READS: usize = 4;
pub(super) const PACK_ENTRY_MAX_BYTES: usize = 64 * 1024;
pub(super) const PACK_MAX_ENTRIES: usize = 256;
const PACK_MAX_BYTES: usize = 2 * 1024 * 1024;
const PACK_READ_LIMIT: usize = PACK_MAX_BYTES + 64 * 1024 + OVERHEAD;
const LOOSE_COMPACT_THRESHOLD: usize = 64;
const PACK_RECHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) type ObjectRead = Result<Option<Vec<u8>>, AppError>;

#[derive(Clone, Copy)]
pub(super) enum Area {
    Manifests,
    Objects,
}

impl Area {
    pub fn matches(self, name: &str) -> bool {
        match self {
            Self::Manifests => name.ends_with(".idx"),
            Self::Objects => name.ends_with(".clip") || name.ends_with(".pack"),
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

pub(super) struct ArchiveFile {
    pub name: String,
    pub revision: FileRevision,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct FileRevision {
    pub size: Option<u64>,
    pub validator: Option<String>,
}

pub(super) fn read_parallel<T: Send>(
    jobs: impl IntoIterator<Item = T>,
    stopped: Option<&AtomicBool>,
    read: impl Fn(T) -> ObjectRead + Sync,
) -> Vec<ObjectRead> {
    if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return jobs.into_iter().map(|_| Err(unavailable())).collect();
    }
    let read = |job| {
        if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(unavailable());
        }
        read(job)
    };
    let mut jobs = jobs.into_iter().peekable();
    let Some(first) = jobs.next() else {
        return Vec::new();
    };
    if jobs.peek().is_none() {
        return vec![read(first)];
    }
    thread::scope(|scope| {
        let read = &read;
        let pending: Vec<_> = iter::once(first)
            .chain(jobs)
            .map(|job| thread::Builder::new().spawn_scoped(scope, move || read(job)))
            .collect();
        pending
            .into_iter()
            .map(|job| {
                job.map_err(|_| unavailable())?
                    .join()
                    .map_err(|_| unavailable())?
            })
            .collect()
    })
}

/// The final name must only become visible once the complete object is durable on the backend.
pub(super) trait ArchiveStore {
    fn list(&mut self, area: Area) -> Result<Vec<ArchiveFile>, AppError>;
    fn object_revision(&mut self, name: &str) -> Result<Option<FileRevision>, AppError>;
    fn read(&mut self, area: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError>;
    fn read_batch_size(&self) -> usize {
        1
    }
    fn read_objects(
        &mut self,
        names: &[String],
        bound: usize,
        stopped: Option<&AtomicBool>,
    ) -> Vec<ObjectRead> {
        names
            .iter()
            .map(|name| {
                if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err(unavailable());
                }
                self.read(Area::Objects, name, bound)
            })
            .collect()
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct ManifestPack {
    name: String,
    total_entries: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum RecordStorage {
    Inline,
    LooseClip,
    LargeClip,
    Pack(u32),
}

#[derive(Clone, Deserialize, Serialize)]
struct ManifestV5 {
    version: u8,
    packs: Vec<ManifestPack>,
    records: Vec<(ReplicaStamp, RecordStorage)>,
}

#[derive(Deserialize, Serialize)]
struct PackFile {
    version: u8,
    entries: Vec<ReplicaEntry>,
}

#[derive(Serialize)]
struct RepairedPack<'a> {
    #[serde(flatten)]
    pack: &'a PackFile,
    nonce: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StoredLocation {
    Inline,
    LooseClip,
    LargeClip,
    Pack { name: String, total_entries: u32 },
}

impl StoredLocation {
    fn object_name(&self, stamp: &ReplicaStamp) -> Option<String> {
        match self {
            Self::Inline => None,
            Self::LooseClip | Self::LargeClip => Some(object_name(stamp)),
            Self::Pack { name, .. } => Some(name.clone()),
        }
    }

    fn sort_key(&self, is_own: bool) -> (u8, u8, u32, &str) {
        let own_rank = if is_own { 0 } else { 1 };
        match self {
            Self::Pack {
                name,
                total_entries,
            } => (own_rank, 0, *total_entries, name.as_str()),
            Self::LooseClip => (own_rank, 1, 0, ""),
            Self::LargeClip => (own_rank, 2, 0, ""),
            Self::Inline => (own_rank, 3, 0, ""),
        }
    }
}

#[derive(Default)]
pub(super) struct ArchiveCache {
    manifests: HashMap<String, ManifestV5>,
    packs: HashMap<String, VerifiedPack>,
}

struct VerifiedPack {
    revision: FileRevision,
    verified_at: Instant,
    members: Arc<HashSet<String>>,
}

impl ArchiveCache {
    pub fn new() -> Self {
        Self::default()
    }
}

enum ObjectLookup {
    Unchecked,
    Listed(HashMap<String, FileRevision>),
    Probe,
}

type Manifests = BTreeMap<String, (u64, String)>;

pub(super) struct Archive<'a, S> {
    store: S,
    key: [u8; 32],
    device: String,
    generation: Option<u64>,
    own: Vec<(ReplicaStamp, StoredLocation)>,
    own_locations: HashMap<String, StoredLocation>,
    candidates: HashMap<String, Vec<StoredLocation>>,
    pack_members: HashMap<String, Result<Arc<HashSet<String>>, AppError>>,
    audited_pack: bool,
    objects: ObjectLookup,
    previous: Vec<String>,
    cache: Option<&'a mut ArchiveCache>,
}

impl<S: ArchiveStore> Archive<'static, S> {
    #[cfg(test)]
    pub fn new(store: S, key: [u8; 32], device: String) -> Self {
        Self {
            store,
            device,
            key,
            generation: None,
            own: Vec::new(),
            own_locations: HashMap::new(),
            candidates: HashMap::new(),
            pack_members: HashMap::new(),
            audited_pack: false,
            objects: ObjectLookup::Unchecked,
            previous: Vec::new(),
            cache: None,
        }
    }
}

impl<'a, S: ArchiveStore> Archive<'a, S> {
    pub fn with_cache(
        store: S,
        key: [u8; 32],
        device: String,
        cache: &'a mut ArchiveCache,
    ) -> Self {
        Self {
            store,
            device,
            key,
            generation: None,
            own: Vec::new(),
            own_locations: HashMap::new(),
            candidates: HashMap::new(),
            pack_members: HashMap::new(),
            audited_pack: false,
            objects: ObjectLookup::Unchecked,
            previous: Vec::new(),
            cache: Some(cache),
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
        for ArchiveFile { name, .. } in self.store.list(Area::Manifests)? {
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

    fn ensure_objects_listed(&mut self) -> Result<(), AppError> {
        if matches!(self.objects, ObjectLookup::Unchecked) {
            self.objects = match self.store.list(Area::Objects) {
                Ok(files) => ObjectLookup::Listed(
                    files
                        .into_iter()
                        .map(|file| (file.name, file.revision))
                        .collect(),
                ),
                Err(error) if error.code == ErrorCode::LimitExceeded => ObjectLookup::Probe,
                Err(error) => return Err(error),
            };
        }
        Ok(())
    }

    fn object_present(&mut self, name: &str) -> Result<bool, AppError> {
        Ok(self.object_revision(name)?.is_some())
    }

    fn object_revision(&mut self, name: &str) -> Result<Option<FileRevision>, AppError> {
        self.ensure_objects_listed()?;
        match &self.objects {
            ObjectLookup::Listed(files) => Ok(files.get(name).cloned()),
            _ => self.store.object_revision(name),
        }
    }

    fn record_created_object(&mut self, name: &str) {
        if let ObjectLookup::Listed(files) = &mut self.objects {
            files.insert(name.to_string(), FileRevision::default());
        }
    }

    fn listed_revision(&self, name: &str) -> Option<FileRevision> {
        match &self.objects {
            ObjectLookup::Listed(files) => files.get(name).cloned(),
            _ => None,
        }
    }

    fn verify_clip(&mut self, entry: &ReplicaEntry) -> Result<(), AppError> {
        let name = object_name(&entry.stamp);
        let data = self.store.read(Area::Objects, &name, MAX_RECORD + OVERHEAD);
        let existing = self.decode_clip(&name, data)?.ok_or_else(invalid)?;
        if existing.stamp != entry.stamp || existing.body != entry.body {
            return Err(invalid());
        }
        Ok(())
    }

    fn verify_pack(&mut self, name: &str, expected_plaintext: &[u8]) -> Result<(), AppError> {
        let Some(data) = self.store.read(Area::Objects, name, PACK_READ_LIMIT)? else {
            return Err(invalid());
        };
        let plaintext = self.open(name, &data).map_err(|_| invalid())?;
        if plaintext != expected_plaintext {
            return Err(invalid());
        }
        Ok(())
    }

    fn decode_clip(&self, name: &str, data: ObjectRead) -> Result<Option<ReplicaEntry>, AppError> {
        let data = data?.ok_or_else(invalid)?;
        let entry = decode(&self.open(name, &data).map_err(|_| invalid())?)?;
        Ok(Some(entry))
    }

    fn decode_pack(
        &mut self,
        name: &str,
        data: ObjectRead,
        revision: Option<FileRevision>,
    ) -> Result<HashMap<String, ReplicaEntry>, AppError> {
        let entries = self.open_pack(name, data);
        let members = match &entries {
            Ok(entries) => Ok(Arc::new(entries.keys().cloned().collect::<HashSet<_>>())),
            Err(error) => Err(error.clone()),
        };
        if let (Some(revision), Ok(members)) = (revision, &members) {
            if let Some(cache) = self.cache.as_deref_mut() {
                cache.packs.insert(
                    name.into(),
                    VerifiedPack {
                        revision,
                        verified_at: Instant::now(),
                        members: members.clone(),
                    },
                );
            }
        } else if entries.is_err() {
            if let Some(cache) = self.cache.as_deref_mut() {
                cache.packs.remove(name);
            }
        }
        self.pack_members.insert(name.to_owned(), members);
        entries
    }

    fn open_pack(
        &self,
        name: &str,
        data: ObjectRead,
    ) -> Result<HashMap<String, ReplicaEntry>, AppError> {
        let data = data?.ok_or_else(invalid)?;
        if data.len() > PACK_READ_LIMIT {
            return Err(limit());
        }
        let plaintext = self.open(name, &data).map_err(|_| invalid())?;
        if pack_name(&plaintext) != name {
            return Err(invalid());
        }
        let pack: PackFile = decode(&plaintext)?;
        if pack.version != PACK_VERSION
            || pack.entries.is_empty()
            || pack.entries.len() > PACK_MAX_ENTRIES
        {
            return Err(invalid());
        }
        let mut entries = HashMap::with_capacity(pack.entries.len());
        for entry in pack.entries {
            entry.stamp.validate().map_err(|_| invalid())?;
            if entry.stamp.removed
                || entry.body.is_none()
                || entries.insert(entry.stamp.key(), entry).is_some()
            {
                return Err(invalid());
            }
        }
        Ok(entries)
    }

    fn pack_contains(&mut self, name: &str, key: &str) -> Result<bool, AppError> {
        if let Some(result) = self.pack_members.get(name) {
            return result
                .as_ref()
                .map(|members| members.contains(key))
                .map_err(Clone::clone);
        }
        let Some(revision) = self.object_revision(name)? else {
            self.pack_members.insert(name.to_owned(), Err(invalid()));
            return Ok(false);
        };
        if let Some(verified) = self
            .cache
            .as_deref()
            .and_then(|cache| cache.packs.get(name))
        {
            if verified.revision == revision {
                if verified.verified_at.elapsed() < PACK_RECHECK_INTERVAL || self.audited_pack {
                    let present = verified.members.contains(key);
                    self.pack_members
                        .insert(name.to_owned(), Ok(verified.members.clone()));
                    return Ok(present);
                }
                self.audited_pack = true;
            }
        }
        let data = self.store.read(Area::Objects, name, PACK_READ_LIMIT);
        let entries = self.decode_pack(name, data, Some(revision))?;
        let present = entries.contains_key(key);
        Ok(present)
    }

    fn add_candidate(&mut self, key: String, location: StoredLocation) {
        let list = self.candidates.entry(key).or_default();
        if !list.contains(&location) {
            list.push(location);
        }
    }

    fn promote_location(&mut self, key: String, location: StoredLocation) {
        self.own_locations.insert(key.clone(), location.clone());
        let list = self.candidates.entry(key).or_default();
        list.retain(|existing| existing != &location);
        list.insert(0, location);
    }

    fn preferred_location(&self, key: &str) -> Option<&StoredLocation> {
        self.candidates.get(key).and_then(|list| list.first())
    }

    fn sort_candidates(&mut self) {
        for (key, list) in &mut self.candidates {
            let own = self.own_locations.get(key);
            list.sort_by(|left, right| {
                left.sort_key(own == Some(left))
                    .cmp(&right.sort_key(own == Some(right)))
            });
        }
    }

    fn upload_objects_batched(
        &mut self,
        objects: Vec<ObjectUpload>,
    ) -> Vec<Result<bool, AppError>> {
        let batch_size = self.store.object_batch_size().max(1);
        let mut results = Vec::with_capacity(objects.len());
        let mut remaining = objects.into_iter();
        loop {
            let chunk: Vec<_> = remaining.by_ref().take(batch_size).collect();
            if chunk.is_empty() {
                break;
            }
            let chunk_len = chunk.len();
            let batch_results = self.store.create_objects(chunk, None);
            if batch_results.len() != chunk_len {
                results.extend((0..chunk_len).map(|_| Err(invalid())));
                results.extend(remaining.map(|_| Err(invalid())));
                break;
            }
            let fatal = batch_results.iter().find_map(|result| match result {
                Err(error)
                    if !matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) =>
                {
                    Some(error.clone())
                }
                _ => None,
            });
            results.extend(batch_results);
            if let Some(error) = fatal {
                results.extend(remaining.map(|_| Err(error.clone())));
                break;
            }
        }
        results
    }

    fn publish_packed_entries(
        &mut self,
        entries: Vec<ReplicaEntry>,
        verify_existing_clips: bool,
    ) -> Vec<Result<(), AppError>> {
        enum PendingItem {
            Clip {
                index: usize,
                name: String,
                large: bool,
                data: Vec<u8>,
            },
            Pack {
                indices: Vec<usize>,
                keys: Vec<String>,
                name: String,
                total_entries: u32,
                plaintext: Vec<u8>,
                data: Vec<u8>,
            },
        }

        let mut results: Vec<Option<Result<(), AppError>>> = vec![None; entries.len()];
        let mut small: Vec<(usize, usize)> = Vec::new();
        let mut pending: Vec<PendingItem> = Vec::new();
        let mut failure: Option<AppError> = None;

        for (index, entry) in entries.iter().enumerate() {
            if let Some(error) = &failure {
                results[index] = Some(Err(error.clone()));
                continue;
            }
            if entry.stamp.removed {
                self.promote_location(entry.stamp.key(), StoredLocation::Inline);
                results[index] = Some(Ok(()));
                continue;
            }
            let stage = (|| -> Result<Option<PendingItem>, AppError> {
                let name = object_name(&entry.stamp);
                if verify_existing_clips && self.object_present(&name)? {
                    self.verify_clip(entry)?;
                    let encoded = encode(entry, MAX_RECORD)?;
                    let location = if encoded.len() <= PACK_ENTRY_MAX_BYTES {
                        StoredLocation::LooseClip
                    } else {
                        StoredLocation::LargeClip
                    };
                    self.promote_location(entry.stamp.key(), location);
                    return Ok(None);
                }
                let encoded = encode(entry, MAX_RECORD)?;
                if encoded.len() <= PACK_ENTRY_MAX_BYTES {
                    small.push((index, encoded.len()));
                    return Ok(None);
                }
                let data = self.seal(&name, &encoded)?;
                Ok(Some(PendingItem::Clip {
                    index,
                    name,
                    large: true,
                    data,
                }))
            })();
            match stage {
                Ok(Some(item)) => pending.push(item),
                Ok(None) => {
                    if small.last().is_none_or(|&(idx, _)| idx != index) {
                        results[index] = Some(Ok(()));
                    }
                }
                Err(error) => {
                    if !matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) {
                        failure = Some(error.clone());
                    }
                    results[index] = Some(Err(error));
                }
            }
        }

        // Partition small entries into immutable packs (>= 2 entries), leaving singletons as loose clips.
        let mut small_entries: Vec<Option<ReplicaEntry>> = entries.into_iter().map(Some).collect();
        let mut offset = 0;
        while offset < small.len() {
            if let Some(error) = &failure {
                for &(index, _) in &small[offset..] {
                    results[index] = Some(Err(error.clone()));
                }
                break;
            }
            let mut count = 0usize;
            let mut bytes = 0usize;
            while offset + count < small.len() && count < PACK_MAX_ENTRIES {
                let entry_bytes = small[offset + count].1;
                if count > 0 && bytes + entry_bytes > PACK_MAX_BYTES {
                    break;
                }
                bytes += entry_bytes;
                count += 1;
            }
            let slice = &small[offset..offset + count];
            offset += count;
            if slice.len() == 1 {
                let (index, _) = slice[0];
                let entry = small_entries[index].as_ref().expect("entry present");
                let name = object_name(&entry.stamp);
                match encode(entry, MAX_RECORD).and_then(|encoded| self.seal(&name, &encoded)) {
                    Ok(data) => pending.push(PendingItem::Clip {
                        index,
                        name,
                        large: false,
                        data,
                    }),
                    Err(error) => results[index] = Some(Err(error)),
                }
            } else {
                let mut indices: Vec<usize> = slice.iter().map(|&(idx, _)| idx).collect();
                indices.sort_by(|&a, &b| {
                    let left = small_entries[a].as_ref().expect("entry present");
                    let right = small_entries[b].as_ref().expect("entry present");
                    left.stamp.key().cmp(&right.stamp.key())
                });
                let keys: Vec<String> = indices
                    .iter()
                    .map(|&idx| {
                        small_entries[idx]
                            .as_ref()
                            .expect("entry present")
                            .stamp
                            .key()
                    })
                    .collect();
                let pack_entries: Vec<ReplicaEntry> = indices
                    .iter()
                    .map(|&idx| small_entries[idx].take().expect("entry present"))
                    .collect();
                let total_entries = pack_entries.len() as u32;
                let pack = PackFile {
                    version: PACK_VERSION,
                    entries: pack_entries,
                };
                let staged = (|| {
                    let mut plaintext = encode(&pack, PACK_MAX_BYTES + 64 * 1024)?;
                    let mut name = pack_name(&plaintext);
                    if self.object_present(&name)? {
                        match self.verify_pack(&name, &plaintext) {
                            Ok(()) => return Ok((name, plaintext, None)),
                            Err(error) if damaged(&error) => {
                                // Preserve the immutable object and offer a healthy alternate.
                                plaintext = encode(
                                    &RepairedPack {
                                        pack: &pack,
                                        nonce: Uuid::new_v4(),
                                    },
                                    PACK_MAX_BYTES + 64 * 1024,
                                )?;
                                name = pack_name(&plaintext);
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    let data = self.seal(&name, &plaintext)?;
                    Ok((name, plaintext, Some(data)))
                })();
                match staged {
                    Ok((name, _plaintext, None)) => {
                        for key in keys {
                            self.promote_location(
                                key,
                                StoredLocation::Pack {
                                    name: name.clone(),
                                    total_entries,
                                },
                            );
                        }
                        for index in indices {
                            results[index] = Some(Ok(()));
                        }
                    }
                    Ok((name, plaintext, Some(data))) => pending.push(PendingItem::Pack {
                        indices,
                        keys,
                        name,
                        total_entries,
                        plaintext,
                        data,
                    }),
                    Err(error) => {
                        if !matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) {
                            failure = Some(error.clone());
                        }
                        for index in indices {
                            results[index] = Some(Err(error.clone()));
                        }
                    }
                }
            }
        }

        if !pending.is_empty() {
            let uploads: Vec<ObjectUpload> = pending
                .iter()
                .map(|item| match item {
                    PendingItem::Clip { name, data, .. } | PendingItem::Pack { name, data, .. } => {
                        ObjectUpload {
                            name: name.clone(),
                            data: data.clone(),
                        }
                    }
                })
                .collect();
            let upload_results = self.upload_objects_batched(uploads);
            for (item, outcome) in pending.into_iter().zip(upload_results) {
                match item {
                    PendingItem::Clip {
                        index, name, large, ..
                    } => {
                        let entry = small_entries[index].as_ref().expect("entry present");
                        let done = outcome.and_then(|created| {
                            if created {
                                self.record_created_object(&name);
                                Ok(())
                            } else {
                                self.verify_clip(entry)
                            }
                        });
                        if done.is_ok() {
                            let location = if large {
                                StoredLocation::LargeClip
                            } else {
                                StoredLocation::LooseClip
                            };
                            self.promote_location(entry.stamp.key(), location);
                        }
                        results[index] = Some(done);
                    }
                    PendingItem::Pack {
                        indices,
                        keys,
                        name,
                        total_entries,
                        plaintext,
                        ..
                    } => {
                        let done = outcome.and_then(|created| {
                            if created {
                                self.record_created_object(&name);
                                Ok(())
                            } else {
                                self.verify_pack(&name, &plaintext)
                            }
                        });
                        if done.is_ok() {
                            for key in keys {
                                self.promote_location(
                                    key,
                                    StoredLocation::Pack {
                                        name: name.clone(),
                                        total_entries,
                                    },
                                );
                            }
                        }
                        for index in indices {
                            results[index] = Some(done.clone());
                        }
                    }
                }
            }
        }

        results
            .into_iter()
            .map(|result| result.unwrap_or_else(|| Err(invalid())))
            .collect()
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

fn pack_name(plaintext: &[u8]) -> String {
    format!("p-{:x}.pack", Sha256::digest(plaintext))
}

fn is_valid_pack_name(name: &str) -> bool {
    name.strip_prefix("p-")
        .and_then(|rest| rest.strip_suffix(".pack"))
        .is_some_and(|hex| {
            hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

fn damaged(error: &AppError) -> bool {
    matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded)
}

fn validate_manifest(manifest: &ManifestV5) -> Result<(), AppError> {
    if manifest.version != MANIFEST_VERSION {
        return Err(invalid());
    }
    if manifest.records.len() > MAX_STAMPS || manifest.packs.len() > MAX_STAMPS {
        return Err(limit());
    }
    for pack in &manifest.packs {
        if !is_valid_pack_name(&pack.name)
            || pack.total_entries == 0
            || pack.total_entries as usize > PACK_MAX_ENTRIES
        {
            return Err(invalid());
        }
    }
    for (stamp, storage) in &manifest.records {
        stamp.validate().map_err(|_| invalid())?;
        match storage {
            RecordStorage::Inline if !stamp.removed => return Err(invalid()),
            RecordStorage::LooseClip | RecordStorage::LargeClip if stamp.removed => {
                return Err(invalid());
            }
            RecordStorage::Pack(index) => {
                if stamp.removed || (*index as usize) >= manifest.packs.len() {
                    return Err(invalid());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn resolve_storage(
    packs: &[ManifestPack],
    storage: RecordStorage,
) -> Result<StoredLocation, AppError> {
    match storage {
        RecordStorage::Inline => Ok(StoredLocation::Inline),
        RecordStorage::LooseClip => Ok(StoredLocation::LooseClip),
        RecordStorage::LargeClip => Ok(StoredLocation::LargeClip),
        RecordStorage::Pack(index) => {
            let pack = packs.get(index as usize).ok_or_else(invalid)?;
            Ok(StoredLocation::Pack {
                name: pack.name.clone(),
                total_entries: pack.total_entries,
            })
        }
    }
}

fn build_manifest_v5(records: &[(ReplicaStamp, StoredLocation)]) -> ManifestV5 {
    let mut pack_indices = HashMap::<String, u32>::new();
    let mut packs = Vec::new();
    let mut encoded = Vec::with_capacity(records.len());
    for (stamp, location) in records {
        let storage = match location {
            StoredLocation::Inline => RecordStorage::Inline,
            StoredLocation::LooseClip => RecordStorage::LooseClip,
            StoredLocation::LargeClip => RecordStorage::LargeClip,
            StoredLocation::Pack {
                name,
                total_entries,
            } => {
                let index = *pack_indices.entry(name.clone()).or_insert_with(|| {
                    let next = packs.len() as u32;
                    packs.push(ManifestPack {
                        name: name.clone(),
                        total_entries: *total_entries,
                    });
                    next
                });
                RecordStorage::Pack(index)
            }
        };
        encoded.push((stamp.clone(), storage));
    }
    ManifestV5 {
        version: MANIFEST_VERSION,
        packs,
        records: encoded,
    }
}

impl<S: ArchiveStore> ReplicaTransport for Archive<'_, S> {
    fn inline_removals(&self) -> bool {
        true
    }

    fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
        let mut unique = HashSet::new();
        let mut result = Vec::new();
        self.objects = ObjectLookup::Unchecked;
        self.own_locations.clear();
        self.candidates.clear();
        self.pack_members.clear();
        self.audited_pack = false;
        self.own.clear();
        self.generation = None;

        let latest = self.latest()?;
        if let Some(cache) = self.cache.as_deref_mut() {
            let live_names: HashSet<&str> =
                latest.values().map(|(_, name)| name.as_str()).collect();
            cache
                .manifests
                .retain(|name, _| live_names.contains(name.as_str()));
        }

        for (device, (generation, name)) in latest {
            let cached = self
                .cache
                .as_deref()
                .and_then(|cache| cache.manifests.get(&name).cloned());
            let manifest = match cached {
                Some(manifest) => manifest,
                None => {
                    let data = self
                        .store
                        .read(Area::Manifests, &name, MAX_INVENTORY + OVERHEAD)?
                        .ok_or_else(unavailable)?;
                    let plaintext = self.open(&name, &data)?;
                    let manifest: ManifestV5 = decode(&plaintext)?;
                    validate_manifest(&manifest)?;
                    if let Some(cache) = self.cache.as_deref_mut() {
                        cache.manifests.insert(name.clone(), manifest.clone());
                    }
                    manifest
                }
            };

            let mut device_records = if device == self.device {
                Vec::with_capacity(manifest.records.len())
            } else {
                Vec::new()
            };
            for (stamp, storage) in manifest.records {
                let location = resolve_storage(&manifest.packs, storage)?;
                let key = stamp.key();
                if device == self.device {
                    device_records.push((stamp.clone(), location.clone()));
                    self.own_locations.insert(key.clone(), location.clone());
                }
                self.add_candidate(key.clone(), location);
                if unique.insert(key) {
                    result.push(stamp);
                    if result.len() > MAX_STAMPS * 2 {
                        return Err(limit());
                    }
                }
            }
            if device == self.device {
                self.own = device_records;
                self.generation = Some(generation);
            }
        }
        self.sort_candidates();
        if let Some(cache) = self.cache.as_deref_mut() {
            let live_packs: HashSet<_> = self
                .candidates
                .values()
                .flatten()
                .filter_map(|location| match location {
                    StoredLocation::Pack { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect();
            cache
                .packs
                .retain(|name, _| live_packs.contains(name.as_str()));
        }
        Ok(result)
    }

    fn fetch_source(&self, stamp: &ReplicaStamp) -> FetchSource<'_> {
        if stamp.removed {
            return FetchSource::Ready;
        }
        let key = stamp.key();
        match self.preferred_location(&key) {
            Some(StoredLocation::Inline) => FetchSource::Ready,
            Some(StoredLocation::Pack { name, .. }) => FetchSource::Shared(name.as_str()),
            _ => FetchSource::Single,
        }
    }

    fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
        self.fetch_batch(&[stamp])
            .into_iter()
            .next()
            .unwrap_or_else(|| Err(invalid()))
    }

    fn fetch_batch_size(&self) -> usize {
        self.store.read_batch_size()
    }

    fn fetch_batch(
        &mut self,
        stamps: &[&ReplicaStamp],
    ) -> Vec<Result<Option<ReplicaEntry>, AppError>> {
        struct ReadGroup {
            name: String,
            bound: usize,
            slots: Vec<(usize, StoredLocation)>,
        }

        let keys: Vec<_> = stamps.iter().map(|stamp| stamp.key()).collect();
        let mut locations: Vec<_> = keys
            .iter()
            .map(|key| {
                self.candidates
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| vec![StoredLocation::LooseClip])
                    .into_iter()
            })
            .collect();
        let mut pending = Vec::new();
        let mut results: Vec<_> = stamps
            .iter()
            .enumerate()
            .map(|(index, stamp)| {
                if stamp.removed {
                    return Ok(Some(ReplicaEntry {
                        stamp: (*stamp).clone(),
                        body: None,
                    }));
                }
                pending.push(index);
                Err(invalid())
            })
            .collect();

        while !pending.is_empty() {
            let mut groups = Vec::<ReadGroup>::new();
            let mut by_name = HashMap::new();
            for index in pending.drain(..) {
                for location in locations[index].by_ref() {
                    let Some(name) = location.object_name(stamps[index]) else {
                        continue;
                    };
                    let is_pack = matches!(location, StoredLocation::Pack { .. });
                    if is_pack
                        && self
                            .pack_members
                            .get(&name)
                            .is_some_and(|members| match members {
                                Ok(members) => !members.contains(&keys[index]),
                                Err(error) => damaged(error),
                            })
                    {
                        continue;
                    }
                    let group = *by_name.entry(name.clone()).or_insert_with(|| {
                        let bound = if is_pack {
                            PACK_READ_LIMIT
                        } else {
                            MAX_RECORD + OVERHEAD
                        };
                        groups.push(ReadGroup {
                            name,
                            bound,
                            slots: Vec::new(),
                        });
                        groups.len() - 1
                    });
                    groups[group].slots.push((index, location));
                    break;
                }
            }

            let mut remaining = groups.into_iter().peekable();
            while let Some(first) = remaining.next() {
                let bound = first.bound;
                if bound == PACK_READ_LIMIT && self.cache.is_some() {
                    // Snapshot revisions before reading so cached integrity can be reused.
                    let _ = self.ensure_objects_listed();
                }
                let mut batch = vec![first];
                while batch.len() < self.store.read_batch_size().max(1)
                    && remaining.peek().is_some_and(|group| group.bound == bound)
                {
                    batch.extend(remaining.next());
                }
                let names: Vec<_> = batch.iter().map(|group| group.name.clone()).collect();
                let data = if let [name] = names.as_slice() {
                    vec![self.store.read(Area::Objects, name, bound)]
                } else {
                    self.store.read_objects(&names, bound, None)
                };
                if data.len() != batch.len() {
                    return stamps.iter().map(|_| Err(invalid())).collect();
                }
                let mut fatal = false;
                for (group, data) in batch.into_iter().zip(data) {
                    let mut entries = if group.bound == PACK_READ_LIMIT {
                        let revision = self.listed_revision(&group.name);
                        self.decode_pack(&group.name, data, revision)
                    } else {
                        self.decode_clip(&group.name, data).map(|entry| {
                            entry
                                .into_iter()
                                .map(|entry| (entry.stamp.key(), entry))
                                .collect()
                        })
                    };
                    for (index, location) in group.slots {
                        let key = &keys[index];
                        let result = match &mut entries {
                            Ok(entries) => entries
                                .remove(key)
                                .filter(|entry| entry.stamp == *stamps[index])
                                .map(Some)
                                .ok_or_else(invalid),
                            Err(error) => Err(error.clone()),
                        };
                        match &result {
                            Ok(Some(_)) => self.promote_location(key.clone(), location),
                            Err(error) if damaged(error) => pending.push(index),
                            Err(_) => fatal = true,
                            _ => {}
                        }
                        results[index] = result;
                    }
                    // Drop all unrequested bodies before decoding another pack.
                }
                if fatal {
                    return results;
                }
            }
        }
        results
    }

    fn contains_body(&mut self, stamp: &ReplicaStamp) -> Result<bool, AppError> {
        if stamp.removed {
            return Ok(true);
        }
        let key = stamp.key();
        let candidates = self.candidates.get(&key).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return self.object_present(&object_name(stamp));
        }
        for candidate in candidates {
            let present = match &candidate {
                StoredLocation::Inline => continue,
                StoredLocation::Pack { name, .. } => self.pack_contains(name, &key),
                _ => self.object_present(&object_name(stamp)),
            };
            match present {
                Ok(true) => {
                    self.promote_location(key, candidate);
                    return Ok(true);
                }
                Ok(false) => {}
                Err(error) if damaged(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError> {
        self.publish_batch(vec![entry])
            .into_iter()
            .next()
            .unwrap_or_else(|| Err(invalid()))
    }

    fn publish_batch_size(&self) -> usize {
        PACK_MAX_ENTRIES
    }

    fn publish_batch(&mut self, entries: Vec<ReplicaEntry>) -> Vec<Result<(), AppError>> {
        self.publish_packed_entries(entries, true)
    }

    fn compaction_candidates(&self, available: &[ReplicaStamp]) -> Vec<ReplicaStamp> {
        let mut pack_live = HashMap::<&str, (usize, u32)>::new();
        for stamp in available {
            if stamp.removed {
                continue;
            }
            if let Some(StoredLocation::Pack {
                name,
                total_entries,
            }) = self.preferred_location(&stamp.key())
            {
                let entry = pack_live
                    .entry(name.as_str())
                    .or_insert((0, *total_entries));
                entry.0 += 1;
            }
        }
        let sparse_packs: HashSet<&str> = pack_live
            .into_iter()
            .filter_map(|(name, (live, total))| {
                (live > 0 && live * 2 < total as usize).then_some(name)
            })
            .collect();

        let mut sparse = Vec::new();
        let mut loose = Vec::new();
        for stamp in available {
            if stamp.removed {
                continue;
            }
            match self.preferred_location(&stamp.key()) {
                Some(StoredLocation::Pack { name, .. }) if sparse_packs.contains(name.as_str()) => {
                    sparse.push(stamp.clone());
                }
                Some(StoredLocation::LooseClip) => {
                    loose.push(stamp.clone());
                }
                _ => {}
            }
        }

        if sparse.is_empty() && loose.len() < LOOSE_COMPACT_THRESHOLD {
            return Vec::new();
        }
        let mut candidates = sparse;
        candidates.extend(loose);
        candidates.truncate(PACK_MAX_ENTRIES);
        candidates
    }

    fn repack(&mut self, entries: Vec<ReplicaEntry>) -> Result<(), AppError> {
        if entries.is_empty() {
            return Ok(());
        }
        let results = self.publish_packed_entries(entries, false);
        for result in results {
            result?;
        }
        Ok(())
    }

    fn commit(&mut self, mut available: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError> {
        let mut next_records = Vec::with_capacity(available.len());
        if !complete {
            // Keep existing references until a published successor or a complete round replaces them.
            let mut published = BTreeMap::<_, Vec<_>>::new();
            for stamp in &available {
                published.entry(&stamp.id).or_default().push(stamp);
            }
            let retained: Vec<_> = self
                .own
                .iter()
                .filter(|(stamp, _)| {
                    !published.get(&stamp.id).is_some_and(|versions| {
                        versions.iter().any(|version| version.supersedes(stamp))
                    })
                })
                .cloned()
                .collect();
            for (stamp, location) in retained {
                let key = stamp.key();
                if self.preferred_location(&key).is_none() {
                    self.promote_location(key, location);
                }
                available.push(stamp);
            }
            available
                .sort_by(|a, b| (&a.id, &a.writer, a.counter).cmp(&(&b.id, &b.writer, b.counter)));
        }

        for stamp in available {
            let location = if stamp.removed {
                StoredLocation::Inline
            } else {
                self.preferred_location(&stamp.key())
                    .cloned()
                    .unwrap_or(StoredLocation::LooseClip)
            };
            next_records.push((stamp, location));
        }

        if self.own == next_records && (self.generation.is_some() || !complete) {
            return Ok(());
        }
        if next_records.len() > MAX_STAMPS {
            return Err(limit());
        }
        let generation = self
            .generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(limit)?;
        let name = format!("{}-{generation:016x}.idx", self.device);
        let manifest = build_manifest_v5(&next_records);
        let body = self.seal(&name, &encode(&manifest, MAX_INVENTORY)?)?;
        if !self.store.create(Area::Manifests, &name, body)? {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        for previous in &self.previous {
            if previous != &name {
                self.store.remove_manifest(previous);
                if let Some(cache) = self.cache.as_deref_mut() {
                    cache.manifests.remove(previous);
                }
            }
        }
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.manifests.insert(name, manifest);
        }
        self.own = next_records;
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
        reads: Vec<String>,
        read_batches: Vec<Vec<String>>,
    }

    impl ArchiveStore for &mut MemoryStore {
        fn list(&mut self, area: Area) -> Result<Vec<ArchiveFile>, AppError> {
            Ok(self
                .entries
                .iter()
                .filter(|(name, _)| area.matches(name))
                .map(|(name, data)| ArchiveFile {
                    name: name.clone(),
                    revision: FileRevision {
                        size: Some(data.len() as u64),
                        validator: None,
                    },
                })
                .collect())
        }

        fn object_revision(&mut self, name: &str) -> Result<Option<FileRevision>, AppError> {
            Ok(self.entries.get(name).map(|data| FileRevision {
                size: Some(data.len() as u64),
                validator: None,
            }))
        }

        fn read(&mut self, _: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError> {
            self.reads.push(name.into());
            let data = self.entries.get(name);
            if data.is_some_and(|data| data.len() > bound) {
                return Err(limit());
            }
            Ok(data.cloned())
        }

        fn read_batch_size(&self) -> usize {
            PARALLEL_READS
        }

        fn read_objects(
            &mut self,
            names: &[String],
            bound: usize,
            _: Option<&AtomicBool>,
        ) -> Vec<ObjectRead> {
            self.read_batches.push(names.to_vec());
            names
                .iter()
                .map(|name| self.read(Area::Objects, name, bound))
                .collect()
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
        // Use large records (> 64 KiB) so each is stored as an individual `.clip` file.
        let large = |label: &str| format!("{label}-{}", "x".repeat(PACK_ENTRY_MAX_BYTES + 128));
        for text in ["first", "second", "third"] {
            block(core.create_text(large(text))).unwrap();
        }
        let device = block(core.replica_device()).unwrap();
        let mut stamps = block(core.replica_inventory()).unwrap();
        stamps.sort_by(|a, b| a.id.cmp(&b.id));
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
        assert_eq!(phases.first(), Some(&Checking));
        assert_eq!(phases.last(), Some(&Finalizing));
        assert!(archive.inventory().unwrap().len() == 2);
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
            let plaintext = encode(
                &ManifestV5 {
                    version,
                    packs: Vec::new(),
                    records: Vec::new(),
                },
                MAX_INVENTORY,
            )
            .unwrap();
            let body = archive.seal(&name, &plaintext).unwrap();
            assert!(archive.store.create(Area::Manifests, &name, body).unwrap());
            assert!(matches!(archive.inventory(), Err(error) if error.code == ErrorCode::SyncData));
        }
    }

    #[test]
    fn packs_small_batches_and_downloads_each_pack_once() {
        let directory = tempfile::tempdir().unwrap();
        let source = block(CoreService::open(directory.path().join("source"))).unwrap();
        let target = block(CoreService::open(directory.path().join("target"))).unwrap();
        for core in [&source, &target] {
            block(core.enroll_replica()).unwrap();
        }
        let count = PACK_MAX_ENTRIES * 2 + 12;
        for index in 0..count {
            block(source.create_text(format!("record {index}"))).unwrap();
        }
        let source_device = block(source.replica_device()).unwrap();
        let target_device = block(target.replica_device()).unwrap();
        let mut store = MemoryStore::default();
        assert!(reconcile(
            &source,
            &mut Archive::new(&mut store, [23; 32], source_device)
        )
        .unwrap());

        let pack_files: Vec<_> = store
            .entries
            .keys()
            .filter(|name| name.ends_with(".pack"))
            .cloned()
            .collect();
        // 524 small entries -> 3 packs (256 + 256 + 12), 0 loose `.clip` files.
        assert_eq!(pack_files.len(), 3);
        assert!(store.entries.keys().all(|name| !name.ends_with(".clip")));

        store.reads.clear();
        store.read_batches.clear();
        assert!(reconcile(
            &target,
            &mut Archive::new(&mut store, [23; 32], target_device)
        )
        .unwrap());

        let mut expected = block(source.replica_inventory()).unwrap();
        let mut actual = block(target.replica_inventory()).unwrap();
        expected.sort_by(|a, b| a.id.cmp(&b.id));
        actual.sort_by(|a, b| a.id.cmp(&b.id));
        assert!(actual == expected);
        // All 3 packs fetched in a single parallel batch (3 reads total, not 524).
        assert_eq!(store.read_batches.len(), 1);
        assert_eq!(store.read_batches[0].len(), 3);

        for core in [source, target] {
            block(core.shutdown()).unwrap();
        }
    }

    fn text_entry(part: usize, index: usize, parts: usize, bytes: usize) -> ReplicaEntry {
        let number = index * parts + part + 1;
        let writer = format!("10000000-0000-4000-8000-{part:012x}");
        ReplicaEntry {
            stamp: ReplicaStamp {
                id: uclip_core::ReplicaId {
                    kind: uclip_core::ReplicaType::Clip,
                    id: format!("20000000-0000-4000-8000-{number:012x}"),
                },
                writer: writer.clone(),
                counter: index as i64 + 1,
                clock: [(writer, index as i64 + 1)].into_iter().collect(),
                removed: false,
                changed_at: index as i64 + 1,
            },
            body: Some(uclip_core::ReplicaBody::Clip {
                kind: "text".into(),
                text: format!("{number} {}", "a".repeat(bytes)),
                data: Vec::new(),
                width: None,
                height: None,
                manual: true,
                pinned: false,
                deleted: false,
                collection: None,
                captured_at: number as i64,
                sequence: number as i64,
                source_app: None,
                file: None,
            }),
        }
    }

    #[test]
    fn downloads_interleaved_packs_once_during_the_same_version_wave() {
        let parts = 16;
        let entries_per_pack = 32;
        let mut store = MemoryStore::default();
        for part in 0..parts {
            let device = format!("10000000-0000-4000-8000-{part:012x}");
            let mut archive = Archive::new(&mut store, [23; 32], device);
            let entries: Vec<_> = (0..entries_per_pack)
                .map(|index| text_entry(part, index, parts, 60 * 1024))
                .collect();
            let stamps = entries.iter().map(|entry| entry.stamp.clone()).collect();
            for result in archive.publish_batch(entries) {
                result.unwrap();
            }
            archive.commit(stamps, true).unwrap();
        }
        let packs = store
            .entries
            .keys()
            .filter(|name| name.ends_with(".pack"))
            .count();
        assert_eq!(packs, parts);
        let directory = tempfile::tempdir().unwrap();
        let fresh = block(CoreService::open(directory.path().join("fresh"))).unwrap();
        block(fresh.enroll_replica()).unwrap();
        let fresh_device = block(fresh.replica_device()).unwrap();
        store.reads.clear();
        assert!(reconcile(
            &fresh,
            &mut Archive::new(&mut store, [23; 32], fresh_device)
        )
        .unwrap());
        assert_eq!(
            block(fresh.replica_inventory()).unwrap().len(),
            parts * entries_per_pack
        );
        block(fresh.shutdown()).unwrap();
        let pack_reads = store
            .reads
            .iter()
            .filter(|name| name.ends_with(".pack"))
            .count();
        assert_eq!(pack_reads, packs);
    }

    #[test]
    fn downloads_large_records_in_bounded_batches_and_skips_restored_objects() {
        let directory = tempfile::tempdir().unwrap();
        let source = block(CoreService::open(directory.path().join("source"))).unwrap();
        let target = block(CoreService::open(directory.path().join("target"))).unwrap();
        for core in [&source, &target] {
            block(core.enroll_replica()).unwrap();
        }
        let count = PARALLEL_READS * 2 + 1;
        for index in 0..count {
            block(source.create_text(format!(
                "large-{index}-{}",
                "y".repeat(PACK_ENTRY_MAX_BYTES + 64)
            )))
            .unwrap();
        }
        let source_device = block(source.replica_device()).unwrap();
        let target_device = block(target.replica_device()).unwrap();
        let mut expected = block(source.replica_inventory()).unwrap();
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        let mut store = MemoryStore::default();
        assert!(reconcile(
            &source,
            &mut Archive::new(&mut store, [23; 32], source_device)
        )
        .unwrap());

        // Resume with one object already committed by the previous session.
        let restored = block(source.replica_entry(expected[0].clone()))
            .unwrap()
            .unwrap();
        block(target.apply_replica_entry(restored)).unwrap();
        let mut round = || {
            reconcile(
                &target,
                &mut Archive::new(&mut store, [23; 32], target_device.clone()),
            )
        };
        assert!(round().unwrap());
        assert!(round().unwrap());
        let mut actual = block(target.replica_inventory()).unwrap();
        actual.sort_by(|left, right| left.id.cmp(&right.id));
        assert!(actual == expected);
        let expected_names: Vec<_> = expected[1..].iter().map(object_name).collect();
        assert_eq!(
            store.read_batches,
            expected_names
                .chunks(PARALLEL_READS)
                .map(<[String]>::to_vec)
                .collect::<Vec<_>>()
        );
        for core in [source, target] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn manifest_cache_avoids_rereading_unchanged_manifests_and_permanent_deletes_use_no_objects() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b"))).unwrap();
        for core in [&a, &b] {
            block(core.enroll_replica()).unwrap();
        }
        let clip = block(a.create_text("short note".into())).unwrap();
        let a_device = block(a.replica_device()).unwrap();
        let b_device = block(b.replica_device()).unwrap();
        let mut store = MemoryStore::default();
        let mut a_cache = ArchiveCache::new();
        let mut b_cache = ArchiveCache::new();

        assert!(reconcile(
            &a,
            &mut Archive::with_cache(&mut store, [23; 32], a_device.clone(), &mut a_cache)
        )
        .unwrap());
        assert!(reconcile(
            &b,
            &mut Archive::with_cache(&mut store, [23; 32], b_device.clone(), &mut b_cache)
        )
        .unwrap());

        // Idle round on B must issue zero GETs when no manifest changed.
        store.reads.clear();
        assert!(reconcile(
            &b,
            &mut Archive::with_cache(&mut store, [23; 32], b_device.clone(), &mut b_cache)
        )
        .unwrap());
        assert!(store.reads.is_empty());

        // Soft-delete then permanently purge the clip on A.
        block(a.change_clip_deletion(uclip_core::ChangeClipDeletion {
            id: clip.clone(),
            expected_revision: "0".into(),
            deleted: true,
        }))
        .unwrap();
        block(
            a.purge_trash(uclip_core::PurgeTrash::All(uclip_core::EmptyTrash {
                expected_revision: block(a.history_revision()).unwrap(),
            })),
        )
        .unwrap();

        store.writes.clear();
        assert!(reconcile(
            &a,
            &mut Archive::with_cache(&mut store, [23; 32], a_device, &mut a_cache)
        )
        .unwrap());
        // Permanent deletion writes only the new manifest, zero `.clip`/`.pack` objects.
        assert!(store.writes.iter().all(|name| name.ends_with(".idx")));

        store.reads.clear();
        assert!(reconcile(
            &b,
            &mut Archive::with_cache(&mut store, [23; 32], b_device, &mut b_cache)
        )
        .unwrap());
        // B reads only A's new manifest, zero object GETs.
        assert_eq!(store.reads.len(), 1);
        assert!(store.reads[0].ends_with(".idx"));
        assert_eq!(
            block(b.read_clip(clip)).unwrap_err().code,
            ErrorCode::NotFound
        );

        for core in [a, b] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn falls_back_to_healthy_candidate_when_preferred_pack_is_missing_or_corrupt() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b"))).unwrap();
        let c = block(CoreService::open(directory.path().join("c"))).unwrap();
        for core in [&a, &b, &c] {
            block(core.enroll_replica()).unwrap();
        }
        let shared = block(a.create_text("shared healthy record".into())).unwrap();
        let shared_stamp = block(a.replica_inventory()).unwrap().remove(0);
        let shared_entry = block(a.replica_entry(shared_stamp.clone()))
            .unwrap()
            .unwrap();
        // B starts with only `shared`, so B publishes it as an individual `.clip`.
        block(b.apply_replica_entry(shared_entry)).unwrap();
        // A has a second record so A publishes both into a `.pack`.
        let extra = block(a.create_text("only in pack".into())).unwrap();

        let a_device = block(a.replica_device()).unwrap();
        let b_device = block(b.replica_device()).unwrap();
        let c_device = block(c.replica_device()).unwrap();
        let mut store = MemoryStore::default();

        // Publish B's standalone `.clip` first, then A's `.pack` (in a separate round where A hasn't seen B yet).
        assert!(reconcile(&b, &mut Archive::new(&mut store, [23; 32], b_device)).unwrap());
        // Simulate A uploading its own pack containing `shared` + `extra` before seeing B's clip.
        let mut isolated = MemoryStore::default();
        assert!(reconcile(&a, &mut Archive::new(&mut isolated, [23; 32], a_device)).unwrap());
        let pack_name = isolated
            .entries
            .keys()
            .find(|name| name.ends_with(".pack"))
            .unwrap()
            .clone();
        // Merge A's manifest into `store`, intentionally omitting A's `.pack` (missing preferred pack!).
        for (name, data) in isolated.entries {
            if name.ends_with(".idx") {
                store.entries.insert(name, data);
            }
        }
        assert!(!store.entries.contains_key(&pack_name));
        assert!(store.entries.contains_key(&object_name(&shared_stamp)));

        // C should report SyncData for `extra` (only in missing pack) while recovering `shared`
        // from B's healthy `.clip` candidate.
        assert!(matches!(
            reconcile(&c, &mut Archive::new(&mut store, [23; 32], c_device)),
            Err(error) if error.code == ErrorCode::SyncData
        ));
        assert_eq!(
            block(c.read_clip(shared)).unwrap(),
            ClipPayload::Text("shared healthy record".into())
        );
        assert!(block(c.read_clip(extra)).is_err());

        for core in [a, b, c] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn offline_peer_stale_pack_does_not_trigger_repeated_compaction_or_uploads() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b"))).unwrap();
        for core in [&a, &b] {
            block(core.enroll_replica()).unwrap();
        }
        let mut ids = Vec::new();
        for index in 0..6 {
            ids.push(block(a.create_text(format!("item {index}"))).unwrap());
        }
        // Ensure offline device's UUID sorts BEFORE the active device's UUID so its manifest is read first.
        let offline_device = "00000000-0000-4000-8000-000000000001".to_string();
        let active_device = "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string();
        let mut store = MemoryStore::default();
        let mut active_cache = ArchiveCache::new();

        // Offline peer publishes 6 records in one pack; active peer syncs them.
        assert!(reconcile(&a, &mut Archive::new(&mut store, [23; 32], offline_device)).unwrap());
        assert!(reconcile(
            &b,
            &mut Archive::with_cache(
                &mut store,
                [23; 32],
                active_device.clone(),
                &mut active_cache
            )
        )
        .unwrap());

        // Active peer permanently deletes 4 of the 6 records -> sparse pack (2/6 < 50%) is compacted.
        for id in &ids[..4] {
            block(b.change_clip_deletion(uclip_core::ChangeClipDeletion {
                id: id.clone(),
                expected_revision: "0".into(),
                deleted: true,
            }))
            .unwrap();
        }
        block(
            b.purge_trash(uclip_core::PurgeTrash::All(uclip_core::EmptyTrash {
                expected_revision: block(b.history_revision()).unwrap(),
            })),
        )
        .unwrap();

        assert!(reconcile(
            &b,
            &mut Archive::with_cache(
                &mut store,
                [23; 32],
                active_device.clone(),
                &mut active_cache
            )
        )
        .unwrap());
        let packs: Vec<_> = store
            .entries
            .keys()
            .filter(|name| name.ends_with(".pack"))
            .cloned()
            .collect();
        assert_eq!(packs.len(), 2);

        // Next idle round on active peer (while offline peer's stale manifest still references the old pack):
        // must perform zero uploads and zero manifest writes.
        store.writes.clear();
        assert!(reconcile(
            &b,
            &mut Archive::with_cache(&mut store, [23; 32], active_device, &mut active_cache)
        )
        .unwrap());
        assert!(store.writes.is_empty());

        for core in [a, b] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn compacts_loose_small_clips_once_threshold_is_reached() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b"))).unwrap();
        for core in [&a, &b] {
            block(core.enroll_replica()).unwrap();
        }
        let a_device = block(a.replica_device()).unwrap();
        let b_device = block(b.replica_device()).unwrap();
        let mut store = MemoryStore::default();
        let mut cache = ArchiveCache::new();

        // Publish one record per round so each starts as an individual loose clip.
        for index in 0..LOOSE_COMPACT_THRESHOLD {
            block(a.create_text(format!("loose {index}"))).unwrap();
            assert!(reconcile(
                &a,
                &mut Archive::with_cache(&mut store, [23; 32], a_device.clone(), &mut cache)
            )
            .unwrap());
        }

        // At threshold, loose clips are compacted into a single pack; a new peer downloads 1 pack instead of 64 clips.
        let pack_count = store
            .entries
            .keys()
            .filter(|name| name.ends_with(".pack"))
            .count();
        assert_eq!(pack_count, 1);

        store.reads.clear();
        assert!(reconcile(&b, &mut Archive::new(&mut store, [23; 32], b_device)).unwrap());
        // B reads only 1 manifest + 1 pack = 2 reads total for all 64 records.
        assert_eq!(store.reads.len(), 2);
        assert!(store.reads.iter().any(|name| name.ends_with(".idx")));
        assert!(store.reads.iter().any(|name| name.ends_with(".pack")));

        for core in [a, b] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn missing_pack_is_repaired_by_a_healthy_replica() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b"))).unwrap();
        for core in [&a, &b] {
            block(core.enroll_replica()).unwrap();
        }
        let first = block(a.create_text("packed one".into())).unwrap();
        let second = block(a.create_text("packed two".into())).unwrap();
        let a_device = block(a.replica_device()).unwrap();
        let b_device = block(b.replica_device()).unwrap();
        let mut store = MemoryStore::default();
        let mut a_cache = ArchiveCache::new();

        assert!(reconcile(
            &a,
            &mut Archive::with_cache(&mut store, [23; 32], a_device.clone(), &mut a_cache)
        )
        .unwrap());
        let pack_file = store
            .entries
            .keys()
            .find(|name| name.ends_with(".pack"))
            .unwrap()
            .clone();

        // Remove the pack on the server; B reports SyncData, then A restores it.
        assert!(store.entries.remove(&pack_file).is_some());
        assert!(matches!(
            reconcile(&b, &mut Archive::new(&mut store, [23; 32], b_device.clone())),
            Err(error) if error.code == ErrorCode::SyncData
        ));
        assert!(reconcile(
            &a,
            &mut Archive::with_cache(&mut store, [23; 32], a_device, &mut a_cache)
        )
        .unwrap());
        assert!(store.entries.contains_key(&pack_file));
        assert!(reconcile(&b, &mut Archive::new(&mut store, [23; 32], b_device)).unwrap());
        assert_eq!(
            block(b.read_clip(first)).unwrap(),
            ClipPayload::Text("packed one".into())
        );
        assert_eq!(
            block(b.read_clip(second)).unwrap(),
            ClipPayload::Text("packed two".into())
        );

        for core in [a, b] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn corrupt_pack_is_repaired_when_the_revision_changes_or_verification_expires() {
        let directory = tempfile::tempdir().unwrap();
        let owner = block(CoreService::open(directory.path().join("owner"))).unwrap();
        let reader = block(CoreService::open(directory.path().join("reader"))).unwrap();
        for core in [&owner, &reader] {
            block(core.enroll_replica()).unwrap();
        }
        let first = block(owner.create_text("first packed record".into())).unwrap();
        let second = block(owner.create_text("second packed record".into())).unwrap();
        let owner_id = block(owner.replica_device()).unwrap();
        let reader_id = block(reader.replica_device()).unwrap();
        let mut store = MemoryStore::default();
        let mut cache = ArchiveCache::new();
        let round = |store: &mut MemoryStore, cache: &mut ArchiveCache| {
            reconcile(
                &owner,
                &mut Archive::with_cache(store, [23; 32], owner_id.clone(), cache),
            )
            .unwrap()
        };
        assert!(round(&mut store, &mut cache));
        assert!(round(&mut store, &mut cache));
        let original = cache.packs.keys().next().unwrap().clone();
        store.reads.clear();
        assert!(round(&mut store, &mut cache));
        assert!(store.reads.is_empty());

        // An in-place corruption can leave a server's metadata unchanged.
        store.entries.get_mut(&original).unwrap()[0] ^= 1;
        cache.packs.get_mut(&original).unwrap().verified_at -= PACK_RECHECK_INTERVAL;
        assert!(round(&mut store, &mut cache));
        let repaired = store
            .entries
            .keys()
            .find(|name| name.ends_with(".pack") && **name != original)
            .unwrap()
            .clone();
        // A second corruption changes the size and should be found before the audit interval.
        assert!(round(&mut store, &mut cache));
        assert!(cache.packs.contains_key(&repaired));
        store.entries.get_mut(&repaired).unwrap().truncate(1);
        assert!(round(&mut store, &mut cache));
        assert!(reconcile(&reader, &mut Archive::new(&mut store, [23; 32], reader_id)).unwrap());
        for (id, text) in [
            (first, "first packed record"),
            (second, "second packed record"),
        ] {
            assert_eq!(
                block(reader.read_clip(id)).unwrap(),
                ClipPayload::Text(text.into())
            );
        }
        for core in [owner, reader] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    fn alternate_pack_reads_are_bounded_and_return_only_requested_entries() {
        let count = PARALLEL_READS * 3;
        let mut store = MemoryStore::default();
        let mut archive = Archive::new(&mut store, [23; 32], Uuid::new_v4().to_string());
        let missing = StoredLocation::Pack {
            name: format!("p-{}.pack", "0".repeat(64)),
            total_entries: count as u32,
        };
        let mut stamps = Vec::new();
        for index in 0..count {
            let requested = text_entry(0, index, count, 10);
            let key = requested.stamp.key();
            stamps.push(requested.stamp.clone());
            let pack = PackFile {
                version: PACK_VERSION,
                entries: vec![requested, text_entry(1, index, count, 8 * 1024)],
            };
            let plaintext = encode(&pack, PACK_MAX_BYTES).unwrap();
            let name = pack_name(&plaintext);
            let data = archive.seal(&name, &plaintext).unwrap();
            assert!(archive.store.create(Area::Objects, &name, data).unwrap());
            archive.promote_location(key.clone(), missing.clone());
            archive.add_candidate(
                key,
                StoredLocation::Pack {
                    name,
                    total_entries: 2,
                },
            );
        }
        let results = archive.fetch_batch(&stamps.iter().collect::<Vec<_>>());
        assert_eq!(results.len(), count);
        for (result, stamp) in results.into_iter().zip(stamps) {
            assert!(result.unwrap().unwrap().stamp == stamp);
        }
        assert_eq!(store.reads.len(), count + 1);
        assert_eq!(
            store.read_batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![PARALLEL_READS; 3],
        );
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

    #[test]
    fn read_parallel_skips_execution_when_already_stopped() {
        let stopped = AtomicBool::new(true);
        let results = read_parallel(["a", "b", "c"], Some(&stopped), |_| {
            panic!("stopped reads must not invoke the reader closure")
        });
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|result| matches!(result, Err(error) if error.code == ErrorCode::SyncConnection)));
    }
}
