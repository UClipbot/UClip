mod archive;
mod dav;
mod files;
mod lan;
mod sftp;
mod transport;
mod wire;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use uclip_core::{
    AppError, CoreService, ErrorCode, FileTicket, SyncRemoteInput, SyncRemoteSettings, SyncRole,
    SyncStatus, UpdateSync,
};
use uuid::Uuid;

use self::{
    archive::{Archive, ArchiveStore},
    lan::{derive, Lan},
    transport::{block, limit, reconcile, unavailable},
};

fn storage() -> AppError {
    AppError::new(ErrorCode::Storage)
}

fn invalid() -> AppError {
    AppError::new(ErrorCode::InvalidInput)
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    code: Option<String>,
    lan: bool,
    remote: Option<SyncRemoteInput>,
}

const CODE_PREFIX: &str = "uclip1-";

fn format_code(key: &[u8; 32]) -> String {
    format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(key))
}

fn decode_code(code: &str) -> Result<[u8; 32], AppError> {
    let payload = code
        .trim()
        .strip_prefix(CODE_PREFIX)
        .filter(|code| code.len() == 43)
        .ok_or_else(invalid)?;
    URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid())?
        .try_into()
        .map_err(|_| invalid())
}

pub fn is_pairing_code(text: &str) -> bool {
    decode_code(text).is_ok()
}

fn validate_remote(remote: &SyncRemoteInput) -> Result<(), AppError> {
    let valid =
        |text: &str, limit: usize| text.len() <= limit && !text.chars().any(char::is_control);
    match remote {
        SyncRemoteInput::Webdav {
            url,
            username,
            password,
        } => {
            if !valid(url, 2048)
                || !valid(username, 256)
                || !valid(password, 2048)
                || (username.is_empty() && !password.is_empty())
            {
                return Err(invalid());
            }
            dav::endpoint(url)?;
        }
        SyncRemoteInput::Sftp {
            host,
            port,
            directory,
            username,
            password,
            host_key,
        } => {
            if host.is_empty()
                || !valid(host, 253)
                || host.chars().any(char::is_whitespace)
                || host.contains(['/', '@', '\\'])
                || *port == 0
                || directory.is_empty()
                || !valid(directory, 2048)
                || !directory.starts_with('/')
                || directory.split('/').any(|part| matches!(part, "." | ".."))
                || username.is_empty()
                || !valid(username, 256)
                || password.is_empty()
                || !valid(password, 2048)
            {
                return Err(invalid());
            }
            sftp::fingerprint(host_key)?;
        }
    }
    Ok(())
}

impl Profile {
    fn load(core: &CoreService) -> Result<Self, AppError> {
        let Some(data) = block(core.replica_profile())? else {
            return Ok(Self::default());
        };
        let profile: Self = serde_json::from_str(&data).map_err(|_| storage())?;
        if profile.key()?.is_none() {
            return Err(storage());
        }
        if let Some(remote) = &profile.remote {
            validate_remote(remote)?;
        }
        Ok(profile)
    }

    fn key(&self) -> Result<Option<[u8; 32]>, AppError> {
        self.code.as_deref().map(decode_code).transpose()
    }

    fn view(&self) -> Option<SyncRemoteSettings> {
        self.remote.as_ref().map(|remote| match remote {
            SyncRemoteInput::Webdav { url, username, .. } => SyncRemoteSettings::Webdav {
                url: url.clone(),
                username: username.clone(),
            },
            SyncRemoteInput::Sftp {
                host,
                port,
                directory,
                username,
                host_key,
                ..
            } => SyncRemoteSettings::Sftp {
                host: host.clone(),
                port: *port,
                directory: directory.clone(),
                username: username.clone(),
                host_key: host_key.clone(),
            },
        })
    }
}

#[derive(Default)]
struct RemoteReport {
    syncing: bool,
    success: Option<u64>,
    error: Option<AppError>,
}

struct RemoteWorker {
    stop: Arc<AtomicBool>,
    request: SyncSender<()>,
    report: Arc<Mutex<RemoteReport>>,
}

impl RemoteWorker {
    fn start(
        core: CoreService,
        key: [u8; 32],
        device: String,
        settings: SyncRemoteInput,
    ) -> Result<Self, AppError> {
        let name = format!(
            "uclip-v1-{}",
            URL_SAFE_NO_PAD.encode(&derive(&key, b"uclip/archive/namespace/v1")?[..18])
        );
        let key = derive(&key, b"uclip/archive/encryption/v1")?;
        let stop = Arc::new(AtomicBool::new(false));
        let report = Arc::new(Mutex::new(RemoteReport::default()));
        let (request, wake) = mpsc::sync_channel(1);
        let signal = stop.clone();
        let status = report.clone();
        thread::Builder::new()
            .name("uclip-sync-storage".into())
            .spawn(move || {
                while !signal.load(Ordering::Acquire) {
                    if let Ok(mut status) = status.lock() {
                        status.syncing = true;
                    }
                    let result = match &settings {
                        SyncRemoteInput::Webdav {
                            url,
                            username,
                            password,
                        } => dav::Dav::connect(url, username, password, &name)
                            .and_then(|store| archive_round(&core, store, key, &device, &signal)),
                        SyncRemoteInput::Sftp {
                            host,
                            port,
                            directory,
                            username,
                            password,
                            host_key,
                        } => sftp::Ssh::connect(
                            host, *port, directory, username, password, host_key, &name,
                        )
                        .and_then(|store| archive_round(&core, store, key, &device, &signal)),
                    };
                    if let Ok(mut status) = status.lock() {
                        status.syncing = false;
                        if matches!(result, Ok(true)) {
                            status.success = Some(lan::now());
                        }
                        status.error = result.err();
                    }
                    if wake.recv_timeout(Duration::from_secs(6))
                        == Err(mpsc::RecvTimeoutError::Disconnected)
                    {
                        break;
                    }
                }
            })
            .map_err(|_| unavailable())?;
        Ok(Self {
            stop,
            request,
            report,
        })
    }

    fn sync_now(&self) -> Result<(), AppError> {
        match self.request.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => Err(AppError::new(ErrorCode::Unavailable)),
        }
    }
}

fn archive_round<S: ArchiveStore>(
    core: &CoreService,
    store: S,
    key: [u8; 32],
    device: &str,
    stop: &AtomicBool,
) -> Result<bool, AppError> {
    let mut remote = Archive::new(ActiveStore(store, stop), key, device.into());
    reconcile(core, &mut remote)
}

struct ActiveStore<'a, S>(S, &'a AtomicBool);

impl<S> ActiveStore<'_, S> {
    fn active(&self) -> Result<(), AppError> {
        if self.1.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        Ok(())
    }
}

impl<S: ArchiveStore> ArchiveStore for ActiveStore<'_, S> {
    fn list(&mut self, area: archive::Area) -> Result<Vec<String>, AppError> {
        self.active()?;
        let result = self.0.list(area)?;
        self.active()?;
        Ok(result)
    }
    fn object_exists(&mut self, name: &str) -> Result<bool, AppError> {
        self.active()?;
        let exists = self.0.object_exists(name)?;
        self.active()?;
        Ok(exists)
    }

    fn read(
        &mut self,
        area: archive::Area,
        name: &str,
        bound: usize,
    ) -> Result<Option<Vec<u8>>, AppError> {
        self.active()?;
        let data = self.0.read(area, name, bound)?;
        self.active()?;
        Ok(data)
    }
    fn create(&mut self, area: archive::Area, name: &str, data: Vec<u8>) -> Result<bool, AppError> {
        self.active()?;
        self.0.create(area, name, data)
    }
    fn remove_manifest(&mut self, name: &str) {
        if self.active().is_ok() {
            self.0.remove_manifest(name);
        }
    }
}

impl Drop for RemoteWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct Runtime {
    profile: Profile,
    running: bool,
    lan: Option<Result<Arc<Lan>, AppError>>,
    remote: Option<Result<RemoteWorker, AppError>>,
}

impl Runtime {
    fn new(profile: Profile, running: bool) -> Self {
        Self {
            profile,
            running,
            lan: None,
            remote: None,
        }
    }

    fn start_lan(&mut self, core: &CoreService, device: Uuid) {
        self.lan = if self.profile.lan {
            Some(self.profile.key().and_then(|key| {
                Lan::start(core.clone(), device, key.ok_or_else(invalid)?).map(Arc::new)
            }))
        } else {
            None
        };
    }

    fn start_remote(&mut self, core: &CoreService, device: Uuid) {
        self.remote = self.profile.remote.as_ref().map(|remote| {
            RemoteWorker::start(
                core.clone(),
                self.profile.key()?.ok_or_else(invalid)?,
                device.to_string(),
                remote.clone(),
            )
        });
    }
}

#[derive(Clone)]
pub struct SyncService {
    core: CoreService,
    device: Uuid,
    incoming: PathBuf,
    runtime: Arc<Mutex<Runtime>>,
    transfer: Arc<AtomicBool>,
    _lock: Arc<fs::File>,
}

pub struct StagedFiles {
    files: Vec<(String, FileTicket, files::Received)>,
    transfer: Arc<AtomicBool>,
}

impl Drop for StagedFiles {
    fn drop(&mut self) {
        self.transfer.store(false, Ordering::Release);
    }
}

impl StagedFiles {
    pub fn replacements(&self) -> Vec<uclip_core::FileReplacement> {
        self.files
            .iter()
            .map(|(id, ticket, file)| uclip_core::FileReplacement {
                id: id.clone(),
                ticket: ticket.clone(),
                paths: file.paths.clone(),
            })
            .collect()
    }

    pub fn preserve(&mut self) {
        for (_, _, file) in self.files.drain(..) {
            let _ = file.directory.keep();
        }
    }
}

impl SyncService {
    pub fn start(core: CoreService, data_dir: &Path) -> Result<Self, AppError> {
        Self::open(core, data_dir, true)
    }

    /// Edit a stopped profile without briefly announcing a LAN peer or contacting remote storage.
    pub fn configure(core: CoreService, data_dir: &Path) -> Result<Self, AppError> {
        Self::open(core, data_dir, false)
    }

    pub fn configured_status(core: &CoreService) -> Result<SyncStatus, AppError> {
        let profile = Profile::load(core)?;
        Ok(SyncStatus {
            paired: profile.code.is_some(),
            lan_enabled: profile.lan,
            role: SyncRole::Off,
            peers: 0,
            lan_last_sync: None,
            lan_error: None,
            remote: profile.view(),
            remote_syncing: false,
            remote_last_sync: None,
            remote_error: None,
        })
    }

    pub fn configured_code(core: &CoreService) -> Result<String, AppError> {
        Profile::load(core)?.code.ok_or_else(invalid)
    }

    fn open(core: CoreService, data_dir: &Path, running: bool) -> Result<Self, AppError> {
        let lock = lock_profile(data_dir)?;
        let incoming = data_dir.join("received-files");
        if running && fs::symlink_metadata(&incoming).is_ok() && prepare_received(&incoming).is_ok()
        {
            let _ = clean_received(&incoming);
        }
        let profile = Profile::load(&core)?;
        let device = block(core.replica_device())?
            .parse()
            .map_err(|_| storage())?;
        let mut runtime = Runtime::new(profile, running);
        if running && runtime.profile.code.is_some() {
            block(core.enroll_replica())?;
            runtime.start_lan(&core, device);
            runtime.start_remote(&core, device);
        }
        Ok(Self {
            core,
            device,
            incoming,
            runtime: Arc::new(Mutex::new(runtime)),
            transfer: Arc::new(AtomicBool::new(false)),
            _lock: lock.into(),
        })
    }

    pub fn status(&self) -> Result<SyncStatus, AppError> {
        let state = self.runtime.lock().map_err(|_| storage())?;
        let (role, peers, last_sync, lan_error) = match &state.lan {
            Some(Ok(lan)) => {
                let status = lan.status()?;
                (
                    status.role,
                    status.peers,
                    status.last_sync.map(|time| time.to_string()),
                    status.error,
                )
            }
            Some(Err(error)) => (SyncRole::Discovering, 0, None, Some(error.clone())),
            None => (SyncRole::Off, 0, None, None),
        };
        let (remote_syncing, remote_last_sync, remote_error) = match &state.remote {
            Some(Ok(remote)) => {
                let report = remote.report.lock().map_err(|_| storage())?;
                (
                    report.syncing,
                    report.success.map(|time| time.to_string()),
                    report.error.clone(),
                )
            }
            Some(Err(error)) => (false, None, Some(error.clone())),
            None => (false, None, None),
        };
        Ok(SyncStatus {
            paired: state.profile.code.is_some(),
            lan_enabled: state.profile.lan,
            role,
            peers,
            lan_last_sync: last_sync,
            lan_error,
            remote: state.profile.view(),
            remote_syncing,
            remote_last_sync,
            remote_error,
        })
    }

    pub fn update(&self, action: UpdateSync) -> Result<SyncStatus, AppError> {
        let mut state = self.runtime.lock().map_err(|_| storage())?;
        let mut next = state.profile.clone();
        let (lan, remote) = match action {
            UpdateSync::Create => {
                if next.code.is_some() {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                let mut key = [0; 32];
                getrandom::fill(&mut key).map_err(|_| unavailable())?;
                next.code = Some(format_code(&key));
                next.lan = true;
                (true, false)
            }
            UpdateSync::Join { code } => {
                let code = format_code(&decode_code(&code)?);
                // A profile remains attached to its replica history and encryption key.
                if next.code.as_ref().is_some_and(|current| current != &code) {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                next.code = Some(code);
                next.lan = true;
                (true, false)
            }
            UpdateSync::Lan { enabled } => {
                if next.code.is_none() {
                    return Err(invalid());
                }
                next.lan = enabled;
                (true, false)
            }
            UpdateSync::Remote { remote } => {
                if next.code.is_none() {
                    return Err(invalid());
                }
                if let Some(remote) = &remote {
                    validate_remote(remote)?;
                }
                next.remote = remote;
                (false, true)
            }
            UpdateSync::SyncRemoteNow => {
                state
                    .remote
                    .as_ref()
                    .ok_or_else(|| AppError::new(ErrorCode::Unavailable))?
                    .as_ref()
                    .map_err(Clone::clone)?
                    .sync_now()?;
                drop(state);
                return self.status();
            }
        };
        let encoded = serde_json::to_string(&next).map_err(|_| storage())?;
        block(self.core.save_replica_profile(encoded))?;
        state.profile = next;
        if state.running && lan {
            if let Some(Ok(old)) = &state.lan {
                old.stop();
            }
            state.start_lan(&self.core, self.device);
        }
        if state.running && remote {
            state.remote.take();
            state.start_remote(&self.core, self.device);
        }
        drop(state);
        self.status()
    }

    pub fn pairing_code(&self) -> Result<String, AppError> {
        self.runtime
            .lock()
            .map_err(|_| storage())?
            .profile
            .code
            .clone()
            .ok_or_else(invalid)
    }

    pub fn stop(&self) {
        if let Ok(mut state) = self.runtime.lock() {
            if let Some(Ok(lan)) = state.lan.take() {
                lan.stop();
            }
            state.remote.take();
        }
    }

    pub fn take_files(&self, tickets: Vec<(String, FileTicket)>) -> Result<StagedFiles, AppError> {
        let lan = {
            let state = self.runtime.lock().map_err(|_| storage())?;
            state
                .lan
                .as_ref()
                .and_then(|lan| lan.as_ref().ok())
                .cloned()
                .ok_or_else(|| AppError::new(ErrorCode::FilePeerOffline))?
        };
        if self.transfer.swap(true, Ordering::AcqRel) {
            return Err(AppError::new(ErrorCode::Busy));
        }
        let mut staged = StagedFiles {
            files: Vec::new(),
            transfer: self.transfer.clone(),
        };
        prepare_received(&self.incoming)?;
        let mut budget = clean_received(&self.incoming)?.min(files::MAX_BYTES);
        for (id, ticket) in tickets {
            let file = lan.file(&ticket, &self.incoming, budget)?;
            budget = budget.checked_sub(file.bytes).ok_or_else(limit)?;
            staged.files.push((id, ticket, file));
        }
        Ok(staged)
    }
}

fn lock_profile(data_dir: &Path) -> Result<fs::File, AppError> {
    let path = data_dir.join("sync.lock");
    if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(storage());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| storage())?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => AppError::new(ErrorCode::Busy),
        std::fs::TryLockError::Error(_) => storage(),
    })?;
    Ok(file)
}

fn prepare_received(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || files::safe_metadata(path).is_err() {
                return Err(storage());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::paths::prepare_directory(path)
        }
        Err(_) => Err(storage()),
    }
}

fn clean_received(path: &Path) -> Result<u64, AppError> {
    const CACHE_CAPACITY: u64 = 1024 * 1024 * 1024;
    const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
    let mut remaining = CACHE_CAPACITY;
    for entry in fs::read_dir(path).map_err(|_| storage())? {
        let entry = entry.map_err(|_| storage())?;
        if !entry.file_name().to_string_lossy().starts_with("received-")
            || !entry.file_type().map_err(|_| storage())?.is_dir()
        {
            continue;
        }
        if entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age > RETENTION)
        {
            fs::remove_dir_all(entry.path()).map_err(|_| storage())?;
            continue;
        }
        remaining = remaining.saturating_sub(cache_size(&entry.path(), 0)?);
    }
    Ok(remaining)
}

fn cache_size(path: &Path, depth: usize) -> Result<u64, AppError> {
    if depth > 36 {
        return Err(limit());
    }
    let mut size = 0u64;
    for entry in fs::read_dir(path).map_err(|_| storage())? {
        let entry = entry.map_err(|_| storage())?;
        let kind = entry.file_type().map_err(|_| storage())?;
        let member = if kind.is_dir() {
            cache_size(&entry.path(), depth + 1)?
        } else if kind.is_file() {
            entry.metadata().map_err(|_| storage())?.len()
        } else {
            0
        };
        size = size.saturating_add(member);
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_sync_requests_are_coalesced_and_never_block_the_caller() {
        let (request, wake) = mpsc::sync_channel(1);
        let worker = RemoteWorker {
            stop: Arc::new(AtomicBool::new(false)),
            request,
            report: Arc::new(Mutex::new(RemoteReport::default())),
        };
        worker.sync_now().unwrap();
        worker.sync_now().unwrap();
        assert_eq!(wake.try_recv(), Ok(()));
        assert_eq!(wake.try_recv(), Err(mpsc::TryRecvError::Empty));
        drop(wake);
        assert_eq!(worker.sync_now().unwrap_err().code, ErrorCode::Unavailable);
    }

    fn fixture_objects(variable: &str, namespace: &str) -> Option<PathBuf> {
        std::env::var_os(variable).map(|root| PathBuf::from(root).join(namespace).join("objects"))
    }

    fn remote_smoke<S: ArchiveStore>(
        make_store: impl Fn() -> S + Sync,
        fixture_objects: Option<PathBuf>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a.sqlite3"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b.sqlite3"))).unwrap();
        block(a.enroll_replica()).unwrap();
        block(b.enroll_replica()).unwrap();
        let first = block(a.create_text("first device".into())).unwrap();
        let second = block(b.create_text("while the first device was away".into())).unwrap();
        let key = [23; 32];
        let a_device = block(a.replica_device()).unwrap();
        let b_device = block(b.replica_device()).unwrap();
        let stop = AtomicBool::new(false);
        archive_round(&a, make_store(), key, &a_device, &stop).unwrap();
        if let Some(objects) = fixture_objects {
            let stamp = block(a.replica_inventory()).unwrap().remove(0);
            let object = objects.join(format!("{}.clip", stamp.key()));
            // Remove a synthetic object created by this test while leaving its manifest.
            fs::remove_file(&object).unwrap();
            assert!(matches!(
                archive_round(&b, make_store(), key, &b_device, &stop),
                Err(error) if error.code == ErrorCode::SyncData
            ));
            assert!(block(b.read_clip(first.clone())).is_err());
            let observer =
                block(CoreService::open(directory.path().join("observer.sqlite3"))).unwrap();
            block(observer.enroll_replica()).unwrap();
            let observer_device = block(observer.replica_device()).unwrap();
            assert!(matches!(
                archive_round(&observer, make_store(), key, &observer_device, &stop),
                Err(error) if error.code == ErrorCode::SyncData
            ));
            assert_eq!(
                block(observer.read_clip(second.clone())).unwrap(),
                uclip_core::ClipPayload::Text("while the first device was away".into())
            );
            block(observer.shutdown()).unwrap();
            assert!(archive_round(&a, make_store(), key, &a_device, &stop).unwrap());
            assert!(
                object.is_file(),
                "the matching replica did not restore the object"
            );
        }
        archive_round(&b, make_store(), key, &b_device, &stop).unwrap();
        archive_round(&a, make_store(), key, &a_device, &stop).unwrap();
        assert_eq!(block(a.replica_inventory()).unwrap().len(), 2);
        assert_eq!(block(b.replica_inventory()).unwrap().len(), 2);
        assert_eq!(
            block(a.read_clip(second)).unwrap(),
            uclip_core::ClipPayload::Text("while the first device was away".into())
        );
        assert_eq!(
            block(b.read_clip(first.clone())).unwrap(),
            uclip_core::ClipPayload::Text("first device".into())
        );
        let edit = |core: &CoreService, text: &str| {
            let item = block(core.history(uclip_core::HistoryQuery {
                query: String::new(),
                regex: false,
                kind: None,
                pinned_only: false,
                collection_id: None,
                offset: 0,
                expected_revision: None,
            }))
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.id == first)
            .unwrap();
            block(core.edit_clip(uclip_core::EditClip {
                clip: uclip_core::ClipVersion {
                    id: item.id,
                    expected_revision: item.revision,
                },
                content: uclip_core::EditableContent::Text(text.into()),
            }))
            .unwrap();
        };
        edit(&a, "older concurrent edit");
        thread::sleep(Duration::from_millis(15));
        edit(&b, "newer concurrent edit");
        thread::scope(|scope| {
            let a = scope.spawn(|| archive_round(&a, make_store(), key, &a_device, &stop));
            let b = scope.spawn(|| archive_round(&b, make_store(), key, &b_device, &stop));
            for result in [a.join().unwrap(), b.join().unwrap()] {
                if let Err(error) = result {
                    // A peer can retire its prior manifest after the other has listed it.
                    assert_eq!(error.code, ErrorCode::SyncConnection);
                }
            }
        });
        let participants = [(&a, &a_device), (&b, &b_device)];
        // Retry both publications first. If the newer writer lost its initial
        // round, the older device may need one more pull to see it.
        for (core, device) in participants {
            assert!(archive_round(core, make_store(), key, device, &stop).unwrap());
        }
        for (core, device) in participants {
            assert!(archive_round(core, make_store(), key, device, &stop).unwrap());
            assert_eq!(
                block(core.read_clip(first.clone())).unwrap(),
                uclip_core::ClipPayload::Text("newer concurrent edit".into())
            );
            assert_eq!(block(core.replica_inventory()).unwrap().len(), 2);
        }
        let query = uclip_core::HistoryQuery {
            query: String::new(),
            regex: false,
            kind: None,
            pinned_only: false,
            collection_id: None,
            offset: 0,
            expected_revision: None,
        };
        let revision_of_first = |core: &CoreService, trash| {
            block(core.history_window(query.clone(), trash, None))
                .unwrap()
                .items
                .into_iter()
                .find(|item| item.id == first)
                .unwrap()
                .revision
        };
        block(a.change_clip_deletion(uclip_core::ChangeClipDeletion {
            id: first.clone(),
            expected_revision: revision_of_first(&a, false),
            deleted: true,
        }))
        .unwrap();
        assert!(archive_round(&a, make_store(), key, &a_device, &stop).unwrap());
        assert!(archive_round(&b, make_store(), key, &b_device, &stop).unwrap());
        block(
            a.purge_trash(uclip_core::PurgeTrash::All(uclip_core::EmptyTrash {
                expected_revision: block(a.history_revision()).unwrap(),
            })),
        )
        .unwrap();
        let purged = block(a.replica_inventory())
            .unwrap()
            .into_iter()
            .find(|stamp| stamp.id.id == first)
            .unwrap();
        thread::sleep(Duration::from_millis(15));
        block(b.change_clip_deletion(uclip_core::ChangeClipDeletion {
            id: first.clone(),
            expected_revision: revision_of_first(&b, true),
            deleted: false,
        }))
        .unwrap();
        let restored = block(b.replica_inventory())
            .unwrap()
            .into_iter()
            .find(|stamp| stamp.id.id == first)
            .unwrap();
        assert!(restored.changed_at > purged.changed_at);
        // B publishes its restore while A's purge is still local.
        assert!(archive_round(&b, make_store(), key, &b_device, &stop).unwrap());
        for (core, device) in participants {
            assert!(archive_round(core, make_store(), key, device, &stop).unwrap());
            assert_eq!(
                block(core.read_clip(first.clone())).unwrap_err().code,
                ErrorCode::NotFound
            );
            let inventory = block(core.replica_inventory()).unwrap();
            assert!(inventory.contains(&purged));
            assert!(!inventory.contains(&restored));
            assert_eq!(inventory.len(), 2);
        }
        block(a.shutdown()).unwrap();
        block(b.shutdown()).unwrap();
    }

    #[test]
    #[ignore = "requires a disposable HTTPS WebDAV server"]
    fn webdav_server_round_trip() {
        let url = std::env::var("UCLIP_TEST_DAV").unwrap();
        let namespace = format!("uclip-test-{}", Uuid::new_v4());
        let objects = fixture_objects("UCLIP_TEST_DAV_FIXTURE", &namespace);
        remote_smoke(
            || {
                dav::Dav::connect(&url, "uclip-test", "disposable-test-password", &namespace)
                    .unwrap()
            },
            objects,
        );
    }

    #[test]
    #[ignore = "requires local IPv4 multicast and a disposable HTTPS WebDAV server"]
    fn lan_and_webdav_share_one_replica_without_rebroadcast_duplicates() {
        let url = std::env::var("UCLIP_TEST_DAV").unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).unwrap();
        let code = format_code(&secret);
        let remote = SyncRemoteInput::Webdav {
            url,
            username: "uclip-test".into(),
            password: "disposable-test-password".into(),
        };
        let start = |name: &str, lan, remote| {
            let directory = root.path().join(name);
            fs::create_dir(&directory).unwrap();
            let core = block(CoreService::open(directory.join("history.sqlite3"))).unwrap();
            let item = block(core.create_text(name.into())).unwrap();
            let profile = Profile {
                code: Some(code.clone()),
                lan,
                remote,
            };
            block(core.save_replica_profile(serde_json::to_string(&profile).unwrap())).unwrap();
            let service = SyncService::start(core.clone(), &directory).unwrap();
            (core, service, item)
        };
        let (a, host, first) = start("LAN and WebDAV", true, Some(remote.clone()));
        let (b, client, second) = start("LAN only", true, None);
        let (c, storage_only, third) = start("WebDAV only", false, Some(remote));
        let start = std::time::Instant::now();
        let all = [&first, &second, &third];
        loop {
            if [&a, &b, &c].iter().all(|core| {
                block(core.replica_inventory()).is_ok_and(|entries| {
                    entries.len() == all.len()
                        && all
                            .iter()
                            .all(|id| entries.iter().any(|stamp| &stamp.id.id == *id))
                })
            }) {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(45),
                "The mixed transports did not converge"
            );
            thread::sleep(Duration::from_millis(200));
        }
        for service in [host, client, storage_only] {
            service.stop();
        }
        for core in [a, b, c] {
            block(core.shutdown()).unwrap();
        }
    }

    #[test]
    #[ignore = "requires a disposable SFTP server"]
    fn sftp_server_round_trip() {
        let port = std::env::var("UCLIP_TEST_SFTP_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let fingerprint = std::env::var("UCLIP_TEST_SFTP_FINGERPRINT").unwrap();
        let namespace = format!("uclip-test-{}", Uuid::new_v4());
        assert!(matches!(sftp::Ssh::connect(
            "127.0.0.1",port,"/","uclip-test","disposable-test-password",
            "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", &namespace,
        ), Err(error) if error.code == ErrorCode::SyncAuthentication));
        let objects = fixture_objects("UCLIP_TEST_SFTP_FIXTURE", &namespace);
        remote_smoke(
            || {
                sftp::Ssh::connect(
                    "127.0.0.1",
                    port,
                    "/",
                    "uclip-test",
                    "disposable-test-password",
                    &fingerprint,
                    &namespace,
                )
                .unwrap()
            },
            objects,
        );
    }
}
