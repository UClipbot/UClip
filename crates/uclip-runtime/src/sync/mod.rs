mod archive;
mod dav;
mod files;
mod lan;
mod pairing;
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
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use uclip_core::{
    AppError, CoreService, ErrorCode, FileTicket, SyncJoinMode, SyncJoinPreview, SyncRemoteInput,
    SyncRemoteProgress, SyncRemoteSettings, SyncRole, SyncStatus, UpdateSync,
};
use uuid::Uuid;

use self::{
    archive::{Archive, ArchiveStore},
    lan::{derive, Lan},
    transport::{block, limit, reconcile_with_progress, unavailable},
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
    progress: Option<SyncRemoteProgress>,
    success: Option<(u64, u32)>,
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
        let report = Arc::new(Mutex::new(RemoteReport {
            progress: Some(SyncRemoteProgress::Queued),
            ..RemoteReport::default()
        }));
        let (request, wake) = mpsc::sync_channel(1);
        let signal = stop.clone();
        let status = report.clone();
        thread::Builder::new()
            .name("uclip-sync-storage".into())
            .spawn(move || {
                while !signal.load(Ordering::Acquire) {
                    let progress = |stage| {
                        if let Ok(mut report) = status.lock() {
                            report.progress = Some(stage);
                        }
                    };
                    progress(SyncRemoteProgress::Connecting);
                    // An explicit request may arrive just as the automatic timer wakes.
                    let _ = wake.try_recv();
                    let result = match &settings {
                        SyncRemoteInput::Webdav {
                            url,
                            username,
                            password,
                        } => dav::Dav::connect(url, username, password, &name).and_then(|store| {
                            archive_round(&core, store, key, &device, &signal, progress)
                        }),
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
                        .and_then(|store| {
                            archive_round(&core, store, key, &device, &signal, progress)
                        }),
                    };
                    if let Ok(mut status) = status.lock() {
                        status.progress = None;
                        match result {
                            Ok(Some(records)) => {
                                status.success = Some((lan::now(), records));
                                status.error = None;
                            }
                            Ok(None) => {
                                status.error = Some(AppError::new(ErrorCode::SyncData));
                            }
                            Err(error) => status.error = Some(error),
                        }
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
        let mut report = self.report.lock().map_err(|_| storage())?;
        match self.request.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {
                if report.progress.is_none() {
                    report.progress = Some(SyncRemoteProgress::Queued);
                }
                Ok(())
            }
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
    progress: impl FnMut(SyncRemoteProgress),
) -> Result<Option<u32>, AppError> {
    let mut remote = Archive::new(ActiveStore(store, stop), key, device.into());
    reconcile_with_progress(core, &mut remote, progress)
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
    fn object_batch_size(&self) -> usize {
        self.0.object_batch_size()
    }

    fn create_objects(
        &mut self,
        objects: Vec<archive::ObjectUpload>,
        _: Option<&AtomicBool>,
    ) -> Vec<Result<bool, AppError>> {
        self.0.create_objects(objects, Some(self.1))
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
    pending_join: Option<PendingJoin>,
    history_backup_folder: Option<String>,
}

struct PendingJoin {
    id: String,
    profile: Profile,
    expires: Instant,
    verified_on_lan: bool,
}

impl Runtime {
    fn new(profile: Profile, running: bool) -> Self {
        Self {
            profile,
            running,
            lan: None,
            remote: None,
            pending_join: None,
            history_backup_folder: None,
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
    pairing_offer: Arc<Mutex<Option<pairing::Offer>>>,
    preparing_join: Arc<Mutex<()>>,
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
        let paired = profile.code.is_some();
        let (capacity, local_only_records) = Self::usage(core, paired)?;
        Ok(SyncStatus {
            paired,
            pairing_active: false,
            history_backup_folder: None,
            lan_enabled: profile.lan,
            role: SyncRole::Off,
            peers: 0,
            lan_last_sync: None,
            lan_error: None,
            remote: profile.view(),
            remote_syncing: false,
            remote_progress: None,
            remote_last_sync: None,
            remote_synced_records: None,
            local_only_records,
            capacity,
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
            pairing_offer: Arc::new(Mutex::new(None)),
            preparing_join: Arc::new(Mutex::new(())),
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
        let (remote_progress, remote_success, remote_error) = match &state.remote {
            Some(Ok(remote)) => {
                let report = remote.report.lock().map_err(|_| storage())?;
                (report.progress, report.success, report.error.clone())
            }
            Some(Err(error)) => (None, None, Some(error.clone())),
            None => (None, None, None),
        };
        let paired = state.profile.code.is_some();
        let lan_enabled = state.profile.lan;
        let remote = state.profile.view();
        let history_backup_folder = state.history_backup_folder.clone();
        let pairing_active = self
            .pairing_offer
            .lock()
            .is_ok_and(|offer| offer.as_ref().is_some_and(pairing::Offer::active));
        drop(state);
        let (capacity, local_only_records) = Self::usage(&self.core, paired)?;
        Ok(SyncStatus {
            paired,
            pairing_active,
            history_backup_folder,
            lan_enabled,
            role,
            peers,
            lan_last_sync: last_sync,
            lan_error,
            remote,
            remote_syncing: remote_progress.is_some(),
            remote_progress,
            remote_last_sync: remote_success.map(|(time, _)| time.to_string()),
            remote_synced_records: remote_success.map(|(_, records)| records),
            local_only_records,
            capacity,
            remote_error,
        })
    }

    fn usage(
        core: &CoreService,
        paired: bool,
    ) -> Result<(Option<uclip_core::SyncCapacity>, Option<u32>), AppError> {
        if paired {
            let (capacity, local_only_records) = block(core.replica_usage())?;
            Ok((Some(capacity), Some(local_only_records)))
        } else {
            Ok((None, None))
        }
    }

    pub fn update(&self, action: UpdateSync) -> Result<SyncStatus, AppError> {
        let mut state = self.runtime.lock().map_err(|_| storage())?;
        let mut next = state.profile.clone();
        let from_now = matches!(
            action,
            UpdateSync::CreateFromNow | UpdateSync::JoinFromNow { .. }
        );
        let mut join_mode = None;
        let (lan, remote) = match action {
            UpdateSync::Create | UpdateSync::CreateFromNow => {
                if next.code.is_some() {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                let mut key = [0; 32];
                getrandom::fill(&mut key).map_err(|_| unavailable())?;
                next.code = Some(format_code(&key));
                next.lan = true;
                (true, false)
            }
            UpdateSync::Join { code } | UpdateSync::JoinFromNow { code } => {
                let code = format_code(&decode_code(&code)?);
                // A profile remains attached to its replica history and encryption key.
                if next.code.as_ref().is_some_and(|current| current != &code) {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                next.code = Some(code);
                next.lan = true;
                (true, false)
            }
            UpdateSync::FinishJoin { id, mode } => {
                if next.code.is_some() {
                    return Err(AppError::new(ErrorCode::Conflict));
                }
                let pending = state
                    .pending_join
                    .as_ref()
                    .filter(|pending| pending.id == id && pending.expires > Instant::now())
                    .ok_or_else(|| AppError::new(ErrorCode::SyncAuthentication))?;
                if matches!(mode, SyncJoinMode::Replace) && !pending.verified_on_lan {
                    return Err(AppError::new(ErrorCode::Restricted));
                }
                next = pending.profile.clone();
                join_mode = Some(mode);
                (true, true)
            }
            UpdateSync::CancelJoin { id } => {
                if state
                    .pending_join
                    .as_ref()
                    .is_some_and(|pending| pending.id == id)
                {
                    state.pending_join = None;
                }
                drop(state);
                return self.status();
            }
            UpdateSync::StopPairing => {
                self.end_pairing();
                drop(state);
                return self.status();
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
        if let Some(mode) = join_mode {
            let backup = block(self.core.join_replica_profile(encoded, mode))?;
            state.history_backup_folder = backup
                .as_deref()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .map(|folder| folder.to_string_lossy().into_owned());
        } else {
            block(self.core.save_replica_profile(encoded, from_now))?;
        }
        state.profile = next;
        state.pending_join = None;
        self.end_pairing();
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

    pub fn start_pairing(&self) -> Result<String, AppError> {
        let state = self.runtime.lock().map_err(|_| storage())?;
        if !state.running || !state.profile.lan || state.profile.key()?.is_none() {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        let (offer, code) = pairing::start(state.profile.clone())?;
        *self.pairing_offer.lock().map_err(|_| storage())? = Some(offer);
        Ok(code)
    }

    fn end_pairing(&self) {
        if let Ok(mut offer) = self.pairing_offer.lock() {
            offer.take();
        }
    }

    pub fn is_local_pairing_pin(&self, text: &str) -> bool {
        let text = text.trim();
        text.len() == 8
            && text.bytes().all(|byte| byte.is_ascii_digit())
            && self
                .pairing_offer
                .lock()
                .is_ok_and(|offer| offer.as_ref().is_some_and(|offer| offer.matches(text)))
    }

    pub fn prepare_join(&self, code: &str) -> Result<SyncJoinPreview, AppError> {
        let _preparing = self
            .preparing_join
            .try_lock()
            .map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => AppError::new(ErrorCode::Busy),
                std::sync::TryLockError::Poisoned(_) => storage(),
            })?;
        {
            let mut state = self.runtime.lock().map_err(|_| storage())?;
            if state.profile.code.is_some() {
                return Err(AppError::new(ErrorCode::Conflict));
            }
            state.pending_join = None;
        }
        let (mut profile, verified_on_lan) = if let Ok(key) = decode_code(code) {
            // Existing groups remain joinable with their long key, including offline setups.
            (
                Profile {
                    code: Some(format_code(&key)),
                    lan: true,
                    remote: None,
                },
                false,
            )
        } else {
            (pairing::receive(code.trim())?, true)
        };
        profile.code = Some(format_code(&profile.key()?.ok_or_else(invalid)?));
        if let Some(remote) = &profile.remote {
            validate_remote(remote)?;
        }
        profile.lan = true;
        let id = Uuid::new_v4().to_string();
        let remote = profile.view();
        let mut state = self.runtime.lock().map_err(|_| storage())?;
        if state.profile.code.is_some() {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        state.pending_join = Some(PendingJoin {
            id: id.clone(),
            profile,
            expires: Instant::now() + Duration::from_secs(300),
            verified_on_lan,
        });
        Ok(SyncJoinPreview {
            id,
            remote,
            verified_on_lan,
        })
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
            self.end_pairing();
            state.pending_join.take();
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

    fn archive_round<S: ArchiveStore>(
        core: &CoreService,
        store: S,
        key: [u8; 32],
        device: &str,
        stop: &AtomicBool,
    ) -> Result<bool, AppError> {
        super::archive_round(core, store, key, device, stop, |_| {})
            .map(|records| records.is_some())
    }

    #[test]
    fn offline_group_keys_cannot_replace_local_history() {
        let directory = tempfile::tempdir().unwrap();
        let core = block(CoreService::open(directory.path().join("db.sqlite3"))).unwrap();
        let record = block(core.create_text("Keep this clip".into())).unwrap();
        let sync = SyncService::configure(core.clone(), directory.path()).unwrap();
        let code = format_code(&[7; 32]);
        let verified = sync.prepare_join(&code).unwrap();
        assert!(!verified.verified_on_lan);
        assert!(matches!(
            sync.update(UpdateSync::FinishJoin { id: verified.id.clone(), mode: SyncJoinMode::Replace }),
            Err(error) if error.code == ErrorCode::Restricted
        ));
        assert!(!sync.status().unwrap().paired);
        assert!(block(core.read_clip(record.clone())).is_ok());
        assert!(
            sync.update(UpdateSync::FinishJoin {
                id: verified.id,
                mode: SyncJoinMode::Merge
            })
            .unwrap()
            .paired
        );
        assert!(block(core.read_clip(record)).is_ok());
    }

    #[test]
    fn a_verified_replacement_keeps_a_recoverable_library_when_the_source_is_offline() {
        let directory = tempfile::tempdir().unwrap();
        let core = block(CoreService::open(directory.path().join("db.sqlite3"))).unwrap();
        let record = block(core.create_text("Keep a recovery copy".into())).unwrap();
        let sync = SyncService::configure(core.clone(), directory.path()).unwrap();
        sync.runtime.lock().unwrap().pending_join = Some(PendingJoin {
            id: "verified-join".into(),
            profile: Profile {
                code: Some(format_code(&[7; 32])),
                lan: true,
                remote: None,
            },
            expires: Instant::now() + Duration::from_secs(60),
            verified_on_lan: true,
        });
        let status = sync
            .update(UpdateSync::FinishJoin {
                id: "verified-join".into(),
                mode: SyncJoinMode::Replace,
            })
            .unwrap();
        assert!(status.paired && status.peers == 0);
        assert_eq!(
            block(core.read_clip(record.clone())).unwrap_err().code,
            ErrorCode::NotFound
        );
        let backup = directory
            .path()
            .join(status.history_backup_folder.unwrap())
            .join("foundation.sqlite3");
        let backup = uclip_core::Storage::open(&backup).unwrap();
        assert_eq!(
            backup.read_clip(&record).unwrap(),
            uclip_core::ClipPayload::Text("Keep a recovery copy".into())
        );
        assert!(backup.replica_profile().unwrap().is_none());
    }

    #[test]
    #[ignore = "requires local IPv4 multicast"]
    fn short_lan_pairing_copies_credentials_and_applies_the_selected_history_mode() {
        use uclip_core::ClipPayload;

        let directory = tempfile::tempdir().unwrap();
        let pair = |name: &str| {
            let path = directory.path().join(name);
            fs::create_dir(&path).unwrap();
            let core = block(CoreService::open(path.join("db.sqlite3"))).unwrap();
            let sync = SyncService::start(core.clone(), &path).unwrap();
            (path, core, sync)
        };
        for (backend, remote, mode) in [
            (
                "webdav",
                SyncRemoteInput::Webdav {
                    url: "https://127.0.0.1:1/uclip/".into(),
                    username: "pairing-fixture".into(),
                    password: "fixture-webdav-password".into(),
                },
                SyncJoinMode::Merge,
            ),
            (
                "sftp",
                SyncRemoteInput::Sftp {
                    host: "127.0.0.1".into(),
                    port: 1,
                    directory: "/pairing-fixture".into(),
                    username: "pairing-fixture".into(),
                    password: "fixture-sftp-password".into(),
                    host_key: format!("SHA256:{}", "A".repeat(43)),
                },
                SyncJoinMode::Replace,
            ),
        ] {
            let (_, host_core, host) = pair(&format!("{backend}-host"));
            let (path, client_core, client) = pair(&format!("{backend}-client"));
            let shared = block(host_core.create_text("from the host".into())).unwrap();
            let local = block(client_core.create_text("from the client".into())).unwrap();
            let remote_data = serde_json::to_value(&remote).unwrap();
            host.update(UpdateSync::Create).unwrap();
            host.update(UpdateSync::Remote {
                remote: Some(remote),
            })
            .unwrap();
            let code = host.start_pairing().unwrap();
            assert_eq!(code.len(), 8);
            assert!(host.is_local_pairing_pin(&code));
            let wrong_code = format!(
                "{}{}",
                if &code[..1] == "0" { "1" } else { "0" },
                &code[1..]
            );
            assert!(
                matches!(client.prepare_join(&wrong_code), Err(error) if error.code == ErrorCode::SyncAuthentication)
            );
            let verified = client.prepare_join(&code).unwrap();
            assert!(pairing::receive(&code).is_err());
            assert!(!client.status().unwrap().paired);
            assert_eq!(
                block(client_core.read_clip(local.clone())).unwrap(),
                ClipPayload::Text("from the client".into())
            );
            let preview = serde_json::to_value(&verified).unwrap();
            assert!(preview["remote"].get("password").is_none());
            assert_eq!(preview["remote"]["kind"], backend);
            let client_status = client
                .update(UpdateSync::FinishJoin {
                    id: verified.id,
                    mode,
                })
                .unwrap();
            assert!(client_status.paired && client_status.lan_enabled);
            assert_eq!(client.pairing_code().unwrap(), host.pairing_code().unwrap());
            let saved = Profile::load(&client_core).unwrap();
            assert_eq!(
                serde_json::to_value(saved.remote.unwrap()).unwrap(),
                remote_data
            );
            let keep_local = matches!(mode, SyncJoinMode::Merge);
            let deadline = Instant::now() + Duration::from_secs(10);
            while block(client_core.read_clip(shared.clone())).is_err()
                || (keep_local && block(host_core.read_clip(local.clone())).is_err())
            {
                assert!(
                    Instant::now() < deadline,
                    "initial LAN reconciliation timed out"
                );
                thread::sleep(Duration::from_millis(50));
            }
            assert_eq!(
                block(client_core.read_clip(local.clone())).is_ok(),
                keep_local
            );
            let updated = block(host_core.create_text("a live LAN update".into())).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while block(client_core.read_clip(updated.clone())).is_err() {
                assert!(
                    Instant::now() < deadline,
                    "LAN update did not arrive independently of remote storage"
                );
                thread::sleep(Duration::from_millis(50));
            }
            assert_eq!(block(host_core.read_clip(local)).is_ok(), keep_local);
            host.stop();
            client.stop();
            drop(client);
            let reopened = SyncService::configure(client_core, &path).unwrap();
            assert!(reopened.status().unwrap().lan_enabled);
            assert_eq!(
                reopened.pairing_code().unwrap(),
                host.pairing_code().unwrap()
            );
        }
    }

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
        assert_eq!(
            worker.report.lock().unwrap().progress,
            Some(SyncRemoteProgress::Queued)
        );
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
            block(core.save_replica_profile(serde_json::to_string(&profile).unwrap(), false))
                .unwrap();
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
