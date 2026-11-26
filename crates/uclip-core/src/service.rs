use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use crate::{AppError, ErrorCode, SettingsSnapshot, Storage, SyncCapacity, UpdateSettings};

const QUEUE_CAPACITY: usize = 64;
type Operation = Box<dyn FnOnce(Result<&mut Storage, AppError>) + Send>;
type Reply<T> = oneshot::Sender<Result<T, AppError>>;

enum Request {
    Settings(Reply<SettingsSnapshot>),
    Update(UpdateSettings, Reply<SettingsSnapshot>),
    Operation(Operation),
    Shutdown(Reply<()>),
}

/// Bounded admission, one SQLite owner, and replies only after transaction completion.
#[derive(Clone)]
pub struct CoreService {
    sender: mpsc::Sender<Request>,
}

impl CoreService {
    pub async fn open(path: PathBuf) -> Result<Self, AppError> {
        let (sender, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel();
        std::thread::Builder::new()
            .name("uclip-storage".into())
            .spawn(move || {
                let mut storage = match Storage::open(&path) {
                    Ok(storage) => {
                        let _ = ready_tx.send(Ok(()));
                        storage
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                while let Some(request) = receiver.blocking_recv() {
                    match request {
                        Request::Operation(run) => run(Ok(&mut storage)),
                        Request::Settings(reply) => {
                            let _ = reply.send(storage.settings());
                        }
                        Request::Update(input, reply) => {
                            let _ = reply.send(storage.update_settings(input));
                        }
                        Request::Shutdown(reply) => {
                            receiver.close();
                            // Requests admitted behind shutdown are rejected explicitly.
                            while let Some(pending) = receiver.blocking_recv() {
                                match pending {
                                    Request::Operation(run) => {
                                        run(Err(AppError::new(ErrorCode::Unavailable)))
                                    }
                                    Request::Settings(r) | Request::Update(_, r) => {
                                        let _ = r.send(Err(AppError::new(ErrorCode::Unavailable)));
                                    }
                                    Request::Shutdown(r) => {
                                        let _ = r.send(Err(AppError::new(ErrorCode::Unavailable)));
                                    }
                                }
                            }
                            let result = storage.checkpoint();
                            drop(storage);
                            let _ = reply.send(result);
                            break;
                        }
                    }
                }
            })
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        ready_rx
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))??;
        Ok(Self { sender })
    }

    fn submit(&self, request: Request) -> Result<(), AppError> {
        self.sender.try_send(request).map_err(|error| {
            AppError::new(match error {
                mpsc::error::TrySendError::Full(_) => ErrorCode::Busy,
                mpsc::error::TrySendError::Closed(_) => ErrorCode::Unavailable,
            })
        })
    }

    pub async fn settings(&self) -> Result<SettingsSnapshot, AppError> {
        let (reply, response) = oneshot::channel();
        self.submit(Request::Settings(reply))?;
        response
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
    }

    pub async fn update_settings(
        &self,
        input: UpdateSettings,
    ) -> Result<SettingsSnapshot, AppError> {
        let (reply, response) = oneshot::channel();
        self.submit(Request::Update(input, reply))?;
        // No mutation timeout: cancellation of a wait must not imply rollback of an accepted write.
        response
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
    }

    pub(crate) async fn operation<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Storage) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let (reply, response) = oneshot::channel();
        self.submit(Request::Operation(Box::new(move |storage| {
            let _ = reply.send(storage.and_then(operation));
        })))?;
        response
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
    }
    pub async fn replica_device(&self) -> Result<String, AppError> {
        self.operation(|s| s.replica_device()).await
    }

    pub async fn enroll_replica(&self) -> Result<(), AppError> {
        self.operation(|s| s.enroll_replica()).await
    }

    pub async fn replica_profile(&self) -> Result<Option<String>, AppError> {
        self.operation(|s| s.replica_profile()).await
    }

    pub async fn save_replica_profile(
        &self,
        profile: String,
        from_now: bool,
    ) -> Result<(), AppError> {
        self.operation(move |s| s.save_replica_profile(profile, from_now))
            .await
    }

    pub async fn join_replica_profile(
        &self,
        profile: String,
        mode: crate::SyncJoinMode,
    ) -> Result<(), AppError> {
        self.operation(move |s| s.join_replica_profile(profile, mode))
            .await
    }

    pub async fn replica_inventory(&self) -> Result<Vec<crate::ReplicaStamp>, AppError> {
        self.operation(|s| s.replica_inventory()).await
    }

    pub async fn replica_usage(&self) -> Result<(SyncCapacity, u32), AppError> {
        self.operation(|s| Ok((s.replica_capacity()?, s.replica_local_only_count()?)))
            .await
    }

    pub async fn replica_entry(
        &self,
        stamp: crate::ReplicaStamp,
    ) -> Result<Option<crate::ReplicaEntry>, AppError> {
        self.operation(move |s| s.replica_entry(&stamp)).await
    }

    pub async fn apply_replica_entry(&self, entry: crate::ReplicaEntry) -> Result<bool, AppError> {
        self.operation(move |s| s.apply_replica_entry(entry)).await
    }

    pub async fn remote_file_tickets(
        &self,
        ids: Vec<String>,
    ) -> Result<Vec<(String, crate::FileTicket)>, AppError> {
        self.operation(move |s| s.remote_file_tickets(ids)).await
    }

    pub async fn clipboard_payload(
        &self,
        ids: Vec<String>,
        history_revision: Option<String>,
        slot_revision: Option<String>,
        staged: Vec<crate::FileReplacement>,
    ) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| {
            s.clipboard_payload(
                &ids,
                history_revision.as_deref(),
                slot_revision.as_deref(),
                staged,
            )
        })
        .await
    }

    pub async fn replica_grant(&self, token: String) -> Result<Vec<String>, AppError> {
        self.operation(move |s| s.replica_grant(&token)).await
    }

    pub async fn capture_policy(&self) -> Result<crate::CapturePolicy, AppError> {
        self.operation(|s| s.capture_policy()).await
    }
    pub async fn history_revision(&self) -> Result<String, AppError> {
        self.operation(|s| s.history_revision()).await
    }
    pub async fn update_capture_policy(
        &self,
        request: crate::UpdateCapturePolicy,
    ) -> Result<crate::CapturePolicy, AppError> {
        self.operation(move |s| s.update_capture_policy(request))
            .await
    }
    pub async fn create_text(&self, text: String) -> Result<String, AppError> {
        self.operation(move |s| s.create_text(text)).await
    }
    pub async fn create_text_in_collection(
        &self,
        text: String,
        collection_id: Option<String>,
    ) -> Result<String, AppError> {
        self.operation(move |s| s.create_text_in_collection(text, collection_id))
            .await
    }

    pub async fn create_clip_version(
        &self,
        payload: crate::ClipPayload,
        collection_id: Option<String>,
    ) -> Result<crate::ClipVersion, AppError> {
        self.operation(move |s| s.create_clip_version(payload, collection_id))
            .await
    }
    pub async fn collections(&self) -> Result<crate::CollectionsSnapshot, AppError> {
        self.operation(|s| s.collections()).await
    }
    pub async fn create_collection(
        &self,
        request: crate::CreateCollection,
    ) -> Result<crate::CollectionItem, AppError> {
        self.operation(move |s| s.create_collection(request)).await
    }
    pub async fn rename_collection(
        &self,
        request: crate::RenameCollection,
    ) -> Result<crate::CollectionItem, AppError> {
        self.operation(move |s| s.rename_collection(request)).await
    }
    pub async fn transfer_clip(
        &self,
        request: crate::TransferClip,
    ) -> Result<crate::ClipTransferSnapshot, AppError> {
        self.operation(move |s| s.transfer_clip(request)).await
    }

    pub async fn delete_collection(
        &self,
        request: crate::DeleteCollection,
    ) -> Result<u32, AppError> {
        self.operation(move |s| s.delete_collection(request)).await
    }

    pub async fn transfer_clips(
        &self,
        request: crate::TransferClips,
    ) -> Result<crate::ClipTransferBatch, AppError> {
        self.operation(move |s| s.transfer_clips(request)).await
    }

    pub async fn editable_content(
        &self,
        clip: crate::ClipVersion,
    ) -> Result<crate::EditableContent, AppError> {
        self.operation(move |s| s.editable_content(clip)).await
    }

    pub async fn edit_clip(&self, request: crate::EditClip) -> Result<(), AppError> {
        self.operation(move |s| s.edit_clip(request)).await
    }
    pub async fn capture(&self, payload: crate::ClipPayload) -> Result<Option<String>, AppError> {
        self.operation(move |s| s.capture(payload)).await
    }
    pub async fn capture_with_source(
        &self,
        payload: crate::ClipPayload,
        source_app: Option<String>,
    ) -> Result<Option<String>, AppError> {
        self.operation(move |s| s.capture_with_source(payload, source_app))
            .await
    }
    pub async fn history(
        &self,
        request: crate::HistoryQuery,
    ) -> Result<crate::HistoryPage, AppError> {
        self.operation(move |s| s.history(request)).await
    }
    pub async fn history_window(
        &self,
        request: crate::HistoryQuery,
        trash: bool,
        count: Option<u32>,
    ) -> Result<crate::HistoryPage, AppError> {
        self.operation(move |s| s.history_window(request, trash, count))
            .await
    }
    pub async fn history_content(
        &self,
        request: crate::HistoryQuery,
        count: u32,
    ) -> Result<Vec<(crate::HistoryItem, crate::ClipPayload)>, AppError> {
        self.operation(move |s| s.history_content(request, count))
            .await
    }
    pub async fn search_history(
        &self,
        request: crate::HistoryQuery,
        trash: bool,
        count: Option<u32>,
        on_first_page: Option<impl FnOnce(crate::HistoryPage) + Send + 'static>,
        is_current: impl Fn() -> bool + Send + 'static,
    ) -> Result<crate::HistoryPage, AppError> {
        self.operation(move |s| s.search_history(request, trash, count, on_first_page, is_current))
            .await
    }
    pub async fn quick_paste_slots(
        &self,
        include_pinned: bool,
    ) -> Result<Vec<crate::QuickPasteSlot>, AppError> {
        self.operation(move |s| s.quick_paste_slots(include_pinned))
            .await
    }
    pub async fn read_quick_paste_slot(
        &self,
        id: String,
        revision: String,
    ) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| s.read_quick_paste_slot(&id, &revision))
            .await
    }
    pub async fn auxiliary_window_size(
        &self,
        window: crate::AuxiliaryWindow,
    ) -> Result<crate::AuxiliaryWindowSize, AppError> {
        self.operation(move |s| s.auxiliary_window_size(window))
            .await
    }
    pub async fn save_auxiliary_window_size(
        &self,
        window: crate::AuxiliaryWindow,
        size: crate::AuxiliaryWindowSize,
    ) -> Result<(), AppError> {
        self.operation(move |s| s.save_auxiliary_window_size(window, size))
            .await
    }
    pub async fn quick_paste_preferences(&self) -> Result<crate::QuickPastePreferences, AppError> {
        self.operation(|s| s.quick_paste_preferences()).await
    }
    pub async fn update_quick_paste_preferences(
        &self,
        request: crate::UpdateQuickPastePreferences,
    ) -> Result<crate::QuickPastePreferences, AppError> {
        self.operation(move |s| s.update_quick_paste_preferences(request))
            .await
    }
    pub async fn read_clip(&self, id: String) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| s.read_clip(&id)).await
    }
    pub async fn read_clip_in_history(
        &self,
        id: String,
        expected_revision: Option<String>,
    ) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| s.read_clip_in_history(&id, expected_revision.as_deref()))
            .await
    }
    pub async fn read_clips(&self, ids: Vec<String>) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| s.read_clips(&ids)).await
    }
    pub async fn read_clips_at_revision(
        &self,
        ids: Vec<String>,
        revision: String,
    ) -> Result<crate::ClipPayload, AppError> {
        self.clipboard_payload(ids, Some(revision), None, Vec::new())
            .await
    }
    pub async fn change_clip_deletion(
        &self,
        request: crate::ChangeClipDeletion,
    ) -> Result<crate::ClipDeletionSnapshot, AppError> {
        self.operation(move |s| s.change_clip_deletion(request))
            .await
    }
    pub async fn change_clip_deletions(
        &self,
        requests: Vec<crate::ChangeClipDeletion>,
    ) -> Result<crate::ClipDeletionBatch, AppError> {
        self.operation(move |s| s.change_clip_deletions(requests))
            .await
    }
    pub async fn purge_trash(&self, request: crate::PurgeTrash) -> Result<u32, AppError> {
        self.operation(move |s| s.purge_trash(request)).await
    }
    pub async fn mark_used(&self, id: String) -> Result<(), AppError> {
        self.operation(move |s| s.mark_used(&id)).await
    }
    pub async fn change_clip_pin(
        &self,
        request: crate::ChangeClipPin,
    ) -> Result<crate::ClipPinSnapshot, AppError> {
        self.operation(move |s| s.change_clip_pin(request)).await
    }
    pub async fn deduplicate_pinned(
        &self,
        request: crate::DeduplicatePinned,
    ) -> Result<crate::DeduplicationSnapshot, AppError> {
        self.operation(move |s| s.deduplicate_pinned(request)).await
    }
    pub async fn keyboard_bindings(&self) -> Result<crate::KeyboardSnapshot, AppError> {
        self.operation(|s| s.keyboard_bindings()).await
    }
    pub async fn update_key_bindings(
        &self,
        request: crate::UpdateKeyBindings,
    ) -> Result<crate::KeyboardSnapshot, AppError> {
        self.operation(move |s| s.update_key_bindings(request))
            .await
    }
    pub async fn desktop_preferences(&self) -> Result<crate::DesktopPreferences, AppError> {
        self.operation(|s| s.desktop_preferences()).await
    }
    pub async fn take_autostart_default(&self) -> Result<bool, AppError> {
        self.operation(|s| s.take_autostart_default()).await
    }
    pub async fn update_desktop_preferences(
        &self,
        request: crate::UpdateDesktopPreferences,
    ) -> Result<crate::DesktopPreferences, AppError> {
        self.operation(move |s| s.update_desktop_preferences(request))
            .await
    }
    pub async fn window_placement(&self) -> Result<Option<crate::WindowPlacement>, AppError> {
        self.operation(|s| s.window_placement()).await
    }
    pub async fn save_window_placement(
        &self,
        placement: crate::WindowPlacement,
    ) -> Result<(), AppError> {
        self.operation(move |s| s.save_window_placement(placement))
            .await
    }

    pub async fn shutdown(&self) -> Result<(), AppError> {
        let (reply, response) = oneshot::channel();
        // Shutdown waits for admission; ordinary calls never accumulate an unbounded wait queue.
        self.sender
            .send(Request::Shutdown(reply))
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
        response
            .await
            .map_err(|_| AppError::new(ErrorCode::Unavailable))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn full_queue_returns_busy_before_accepting_work() {
        // Keep receiver alive without a consumer to test admission deterministically.
        let (sender, _receiver) = mpsc::channel(1);
        let service = CoreService { sender };
        let (first, _) = oneshot::channel();
        service.submit(Request::Settings(first)).unwrap();
        let (second, _) = oneshot::channel();
        assert_eq!(
            service.submit(Request::Settings(second)).unwrap_err().code,
            ErrorCode::Busy
        );
    }
}
