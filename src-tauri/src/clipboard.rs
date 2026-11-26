//! All clipboard reads/writes live on a bounded actor, independently of the WebView.
use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard_rs::{common::RustImage, RustImageData};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::Duration,
};
use uclip_core::{
    AppError, CapturePolicy, ClipDetail, ClipPayload, ClipboardStatus, CopyResult, CoreService,
    ErrorCode, UpdateCapturePolicy,
};

use tokio::sync::oneshot;
pub use uclip_runtime::clipboard::normalize_files;
#[cfg(target_os = "linux")]
use uclip_runtime::clipboard::sensitive;
use uclip_runtime::clipboard::{write, Captured, Context as ClipboardContext, MARKER};
type Reply<T> = oneshot::Sender<Result<T, AppError>>;
enum Request {
    Policy(UpdateCapturePolicy, Reply<CapturePolicy>),
    Copy(
        CopyRequest,
        Option<crate::sync::StagedFiles>,
        Reply<CopyResult>,
    ),
    Deletion(
        uclip_core::ChangeClipDeletion,
        Reply<uclip_core::ClipDeletionSnapshot>,
    ),
    Stop(Reply<()>),
}
enum CopyRequest {
    History {
        ids: Vec<String>,
        history_revision: Option<String>,
        paste: bool,
        fallback: bool,
    },
    Quick {
        id: String,
        revision: String,
        target: Option<crate::paste::InvocationTarget>,
        active: Arc<AtomicBool>,
    },
}

#[derive(Default)]
struct State {
    available: bool,
    error: Option<AppError>,
}
#[derive(Clone)]
pub struct ClipboardService {
    sender: SyncSender<Request>,
    state: Arc<Mutex<State>>,
    core: CoreService,
    paste: Arc<crate::paste::PasteService>,
    sync: Option<crate::sync::SyncService>,
}
fn error() -> AppError {
    AppError::new(ErrorCode::Clipboard)
}
fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

impl ClipboardService {
    pub(crate) fn start(
        core: CoreService,
        app: Option<tauri::AppHandle>,
        sync: Option<crate::sync::SyncService>,
    ) -> Result<Self, AppError> {
        #[cfg(target_os = "linux")]
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        let (sender, receiver) = mpsc::sync_channel(32);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(State::default()));
        let paste = Arc::new(crate::paste::PasteService::start(app));
        let service = Self {
            sender,
            state: state.clone(),
            core: core.clone(),
            paste: paste.clone(),
            sync: sync.clone(),
        };
        std::thread::Builder::new()
            .name("uclip-clipboard".into())
            .spawn(move || {
                let context = match ClipboardContext::new() {
                    Ok(c) => c,
                    Err(_) => {
                        let _ = ready_tx.send(Err(error()));
                        return;
                    }
                };
                let mut watch = match crate::clipboard_watch::Watch::start() {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let marker = uuid::Uuid::new_v4().to_string().into_bytes();
                let mut enabled = block(core.capture_policy()).is_ok_and(|p| p.enabled);
                let mut seen = watch.generation().unwrap_or(0);
                let mut failed_attempts = 0u32;
                let mut attempted_generation = seen;
                let mut rejected_generation = None;
                let mut pending_payload: Option<(u64, Captured)> = None;
                if let Ok(mut s) = state.lock() {
                    s.available = true;
                }
                let _ = ready_tx.send(Ok(()));
                loop {
                    #[cfg(target_os = "windows")]
                    context.pump();
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(Request::Policy(input, reply)) => {
                            let response = block(core.update_capture_policy(input));
                            if let Ok(policy) = &response {
                                let was_enabled = enabled;
                                enabled = policy.enabled;
                                if !was_enabled || !enabled {
                                    seen = watch.generation().unwrap_or(seen);
                                    pending_payload = None;
                                }
                                failed_attempts = 0;
                                rejected_generation = None;
                                if let Ok(mut s) = state.lock() {
                                    s.error = None;
                                }
                            }
                            let _ = reply.send(response);
                        }
                        Ok(Request::Copy(request, mut staged, reply)) => {
                            let replacements = staged
                                .as_ref()
                                .map(crate::sync::StagedFiles::replacements)
                                .unwrap_or_default();
                            let response = (|| {
                                let (ids, payload, prepared, active) = match request {
                                    CopyRequest::History {
                                        ids,
                                        history_revision,
                                        paste: auto_paste,
                                        fallback,
                                    } => {
                                        let payload = block(core.clipboard_payload(
                                            ids.clone(),
                                            history_revision,
                                            None,
                                            replacements,
                                        ))?;
                                        let prepared =
                                            match auto_paste.then(|| paste.prepare()).transpose() {
                                                Err(error)
                                                    if fallback
                                                        && matches!(
                                                            error.code,
                                                            ErrorCode::TargetLost
                                                                | ErrorCode::Restricted
                                                        ) =>
                                                {
                                                    None
                                                }
                                                result => result?,
                                            };
                                        (ids, payload, prepared, None)
                                    }
                                    CopyRequest::Quick {
                                        id,
                                        revision,
                                        target,
                                        active,
                                    } => {
                                        let payload = block(core.clipboard_payload(
                                            vec![id.clone()],
                                            None,
                                            Some(revision),
                                            replacements,
                                        ))?;
                                        let prepared = target
                                            .map(|target| paste.prepare_invocation(target, &active))
                                            .transpose()?;
                                        (vec![id], payload, prepared, Some(active))
                                    }
                                };
                                let valid = || {
                                    active
                                        .as_ref()
                                        .is_none_or(|active| active.load(Ordering::Acquire))
                                };
                                if !valid() {
                                    return Err(AppError::new(ErrorCode::TargetLost));
                                }
                                #[cfg(target_os = "windows")]
                                let publication = write(&context, payload, MARKER, &marker)?;
                                #[cfg(not(target_os = "windows"))]
                                write(&context, payload, MARKER, &marker)?;
                                if let Some(staged) = &mut staged {
                                    staged.preserve();
                                }
                                #[cfg(target_os = "linux")]
                                let selection_owner = if active.is_some() && prepared.is_some() {
                                    quick_selection_owner(&marker).map(Some)
                                } else {
                                    Ok(None)
                                };
                                let usage =
                                    ids.into_iter().try_for_each(|id| block(core.mark_used(id)));
                                let mut result = CopyResult {
                                    copied: true,
                                    usage_saved: usage.is_ok(),
                                    paste_requested: false,
                                    warning: usage.err(),
                                };
                                if let Some(prepared) = prepared {
                                    let dispatched = (|| {
                                        if !valid() {
                                            return Err(AppError::new(ErrorCode::TargetLost));
                                        }
                                        paste.paste(
                                            prepared,
                                            #[cfg(target_os = "windows")]
                                            publication,
                                            #[cfg(target_os = "linux")]
                                            selection_owner?,
                                        )
                                    })();
                                    match dispatched {
                                        Ok(()) => result.paste_requested = true,
                                        Err(e) => result.warning = Some(e),
                                    }
                                }
                                Ok(result)
                            })();
                            let _ = reply.send(response);
                        }
                        Ok(Request::Deletion(request, reply)) => {
                            let _ = reply.send(block(core.change_clip_deletion(request)));
                        }
                        Ok(Request::Stop(reply)) => {
                            let stopped = watch.stop();
                            paste.stop();
                            let _ = reply.send(stopped);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = watch.stop();
                            paste.stop();
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let observed = match watch.generation() {
                        Ok(g) => {
                            if let Ok(mut s) = state.lock() {
                                s.available = true;
                            }
                            g
                        }
                        Err(e) => {
                            if let Ok(mut s) = state.lock() {
                                s.available = false;
                                s.error = Some(e);
                            }
                            continue;
                        }
                    };
                    if !enabled {
                        seen = observed;
                        continue;
                    }
                    if pending_payload.is_none()
                        && (observed == seen || rejected_generation == Some(observed))
                    {
                        continue;
                    }
                    let attempt_generation = pending_payload
                        .as_ref()
                        .map_or(observed, |(generation, _)| *generation);
                    if attempt_generation != attempted_generation {
                        rejected_generation = None;
                        failed_attempts = 0;
                        attempted_generation = attempt_generation;
                    }
                    // Failure does not advance the cursor; retry is bounded per wake, with backoff.
                    if failed_attempts > 0 && !failed_attempts.is_multiple_of(25) {
                        failed_attempts += 1;
                        continue;
                    }
                    // Retain one bounded complete snapshot on write failure. A later OS
                    // clipboard change cannot replace an already-read, uncommitted payload.
                    let captured = (|| {
                        if pending_payload.is_none() {
                            if let Some(payload) =
                                stable_read(&context, &marker, &mut watch, observed)?
                            {
                                pending_payload = Some((observed, payload));
                            } else {
                                return Ok(false);
                            }
                        }
                        let snapshot = &pending_payload.as_ref().expect("snapshot ready").1;
                        if matches!(&snapshot.payload, ClipPayload::Text(text)
                            if crate::sync::is_pairing_code(text)
                                || sync.as_ref().is_some_and(|service| service.is_local_pairing_pin(text))) {
                            return Ok(false);
                        }
                        block(core.capture_with_source(
                            snapshot.payload.clone(),
                            snapshot.source_app.clone(),
                        ))
                        .map(|id| id.is_some())
                    })();
                    match captured {
                        Ok(saved) => {
                            seen = attempt_generation;
                            pending_payload = None;
                            failed_attempts = 0;
                            // A skipped/empty clipboard must not erase a rejection before the UI polls it.
                            if saved {
                                if let Ok(mut s) = state.lock() {
                                    s.error = None;
                                }
                            }
                        }
                        Err(e) => {
                            if matches!(
                                e.code,
                                ErrorCode::LimitExceeded
                                    | ErrorCode::InvalidInput
                                    | ErrorCode::InvalidPattern
                            ) {
                                rejected_generation = Some(attempt_generation);
                                pending_payload = None;
                            }
                            failed_attempts = failed_attempts.saturating_add(1);
                            if let Ok(mut s) = state.lock() {
                                s.error = Some(e);
                            }
                        }
                    }
                }
            })
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        ready_rx.recv().map_err(|_| error())??;
        Ok(service)
    }
    fn submit(&self, request: Request) -> Result<(), AppError> {
        self.sender.try_send(request).map_err(|e| {
            AppError::new(match e {
                mpsc::TrySendError::Full(_) => ErrorCode::Busy,
                mpsc::TrySendError::Disconnected(_) => ErrorCode::Unavailable,
            })
        })
    }
    pub async fn status(&self) -> Result<ClipboardStatus, AppError> {
        let policy = self.core.capture_policy().await?;
        let history_revision = self.core.history_revision().await?;
        let s = self.state.lock().map_err(|_| error())?;
        Ok(ClipboardStatus {
            policy,
            history_revision,
            available: s.available,
            error: s.error.clone(),
            paste_available: self.paste.available(),
            paste_target: self.paste.has_target(),
            shortcut_available: self.paste.shortcut_available(),
        })
    }
    pub async fn update_policy(
        &self,
        request: UpdateCapturePolicy,
    ) -> Result<CapturePolicy, AppError> {
        let (tx, rx) = oneshot::channel();
        self.submit(Request::Policy(request, tx))?;
        rx.await.map_err(|_| error())?
    }
    pub fn capture_target(&self) {
        self.paste.capture_target();
    }
    async fn stage_files(
        &self,
        ids: Vec<String>,
    ) -> Result<Option<crate::sync::StagedFiles>, AppError> {
        let tickets = self.core.remote_file_tickets(ids).await?;
        if tickets.is_empty() {
            return Ok(None);
        }
        let sync = self
            .sync
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::FilePeerOffline))?;
        tauri::async_runtime::spawn_blocking(move || sync.take_files(tickets))
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
            .map(Some)
    }

    pub async fn copy(
        &self,
        ids: Vec<String>,
        paste: bool,
        fallback_to_copy: bool,
        history_revision: Option<String>,
    ) -> Result<CopyResult, AppError> {
        let staged = self.stage_files(ids.clone()).await?;
        let (tx, rx) = oneshot::channel();
        self.submit(Request::Copy(
            CopyRequest::History {
                ids,
                history_revision,
                paste,
                fallback: fallback_to_copy,
            },
            staged,
            tx,
        ))?;
        rx.await.map_err(|_| error())?
    }
    pub fn invocation_target(&self) -> Option<crate::paste::InvocationTarget> {
        self.paste.invocation_target()
    }

    pub async fn quick_paste(
        &self,
        id: String,
        revision: String,
        target: Option<crate::paste::InvocationTarget>,
        active: Arc<AtomicBool>,
    ) -> Result<CopyResult, AppError> {
        let staged = self.stage_files(vec![id.clone()]).await?;
        if !active.load(Ordering::Acquire) {
            return Err(AppError::new(ErrorCode::TargetLost));
        }
        let (tx, rx) = oneshot::channel();
        self.submit(Request::Copy(
            CopyRequest::Quick {
                id,
                revision,
                target,
                active,
            },
            staged,
            tx,
        ))?;
        rx.await.map_err(|_| error())?
    }

    pub async fn change_deletion(
        &self,
        request: uclip_core::ChangeClipDeletion,
    ) -> Result<uclip_core::ClipDeletionSnapshot, AppError> {
        let (tx, rx) = oneshot::channel();
        self.submit(Request::Deletion(request, tx))?;
        rx.await.map_err(|_| error())?
    }
    pub async fn stop(&self) -> Result<(), AppError> {
        let (tx, rx) = oneshot::channel();
        let sender = self.sender.clone();
        tauri::async_runtime::spawn_blocking(move || sender.send(Request::Stop(tx)))
            .await
            .map_err(|_| error())?
            .map_err(|_| error())?;
        rx.await.map_err(|_| error())?
    }
}

#[cfg(not(target_os = "linux"))]
fn stable_read(
    context: &ClipboardContext,
    marker: &[u8],
    watch: &mut crate::clipboard_watch::Watch,
    expected_generation: u64,
) -> Result<Option<Captured>, AppError> {
    let before = watch.generation()?;
    if before != expected_generation {
        return Err(AppError::new(ErrorCode::Busy));
    }
    #[cfg(target_os = "windows")]
    let captured = context.read(marker)?;
    #[cfg(target_os = "macos")]
    let captured = {
        let source = context.source_application();
        let payload = context.read(marker)?;
        let source_app = payload.as_ref().and_then(|_| source.finish());
        payload.map(|payload| Captured {
            payload,
            source_app,
        })
    };
    if before != watch.generation()? {
        return Err(AppError::new(ErrorCode::Busy));
    }
    if let Some(snapshot) = &captured {
        snapshot.payload.validate()?;
    }
    Ok(captured)
}

#[cfg(target_os = "linux")]
fn quick_selection_owner(marker: &[u8]) -> Result<u32, AppError> {
    let reader = uclip_runtime::clipboard_x11::Reader::new()?;
    let owner = reader.owner()?;
    if owner == 0 || reader.buffer(MARKER, 64)? != marker || reader.owner()? != owner {
        return Err(AppError::new(ErrorCode::TargetLost));
    }
    Ok(owner)
}

pub fn detail(id: String, payload: ClipPayload, thumbnail: bool) -> Result<ClipDetail, AppError> {
    let (text, image_data_url) = match payload {
        ClipPayload::Text(text) => (text, None),
        ClipPayload::Files(files) => (files.join("\n"), None),
        ClipPayload::Image {
            width,
            height,
            rgba,
        } => {
            let image = RustImageData::from_dynamic_image(image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, rgba).ok_or_else(error)?,
            ));
            let scaled = image
                .thumbnail(
                    width.min(if thumbnail { 160 } else { 1280 }),
                    height.min(if thumbnail { 160 } else { 1280 }),
                )
                .map_err(|_| error())?;
            (
                format!("{width} × {height}"),
                Some(format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(scaled.to_png().map_err(|_| error())?.get_bytes())
                )),
            )
        }
    };
    Ok(ClipDetail {
        id,
        text,
        image_data_url,
    })
}

#[cfg(target_os = "linux")]
fn stable_read(
    _context: &ClipboardContext,
    marker: &[u8],
    watch: &mut crate::clipboard_watch::Watch,
    expected_generation: u64,
) -> Result<Option<Captured>, AppError> {
    let before = watch.generation()?;
    if before != expected_generation {
        return Err(AppError::new(ErrorCode::Busy));
    }
    let reader = uclip_runtime::clipboard_x11::Reader::new()?;
    let owner = reader.owner()?;
    let formats = reader.formats()?;
    if sensitive(&formats, |f| reader.buffer(f, 16)) {
        return Ok(None);
    }
    if formats.iter().any(|f| f == MARKER) && reader.buffer(MARKER, 64).is_ok_and(|v| v == marker) {
        return Ok(None);
    }
    let payload = reader.payload(&formats)?;
    let source_app = payload.as_ref().and_then(|_| reader.source_app(owner));
    if before != watch.generation()? || owner != reader.owner()? {
        return Err(AppError::new(ErrorCode::Busy));
    }
    if let Some(payload) = &payload {
        payload.validate()?;
    }
    Ok(payload.map(|payload| Captured {
        payload,
        source_app,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_uris_decode_once_and_remote_or_ambiguous_uris_are_refused() {
        let dir = std::env::temp_dir();
        let path = dir.join("clipboard 你好#%.txt");
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        assert_eq!(
            normalize_files(vec![uri]).unwrap(),
            vec![path.to_str().unwrap().to_string()]
        );
        for uri in [
            "file://remote.invalid/share/file",
            "https://example.invalid/file",
            "file:///tmp/file?query",
            "file:///tmp/file#fragment",
        ] {
            assert!(normalize_files(vec![uri.into()]).is_err());
        }
    }
    #[test]
    fn preview_is_a_real_png_and_preserves_transparency() {
        let rgba = vec![12, 34, 56, 0, 100, 120, 140, 255];
        let result = detail(
            "id".into(),
            ClipPayload::Image {
                width: 2,
                height: 1,
                rgba: rgba.clone(),
            },
            true,
        )
        .unwrap();
        let bytes = STANDARD
            .decode(
                result
                    .image_data_url
                    .unwrap()
                    .strip_prefix("data:image/png;base64,")
                    .unwrap(),
            )
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.into_raw(), rgba);
    }
}
