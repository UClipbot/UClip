use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use crate::{AppError, ErrorCode, SettingsSnapshot, Storage, UpdateSettings};

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

    async fn operation<T: Send + 'static>(
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
    pub async fn capture(&self, payload: crate::ClipPayload) -> Result<Option<String>, AppError> {
        self.operation(move |s| s.capture(payload)).await
    }
    pub async fn history(
        &self,
        request: crate::HistoryQuery,
    ) -> Result<crate::HistoryPage, AppError> {
        self.operation(move |s| s.history(request)).await
    }
    pub async fn read_clip(&self, id: String) -> Result<crate::ClipPayload, AppError> {
        self.operation(move |s| s.read_clip(&id)).await
    }
    pub async fn change_clip_deletion(
        &self,
        request: crate::ChangeClipDeletion,
    ) -> Result<crate::ClipDeletionSnapshot, AppError> {
        self.operation(move |s| s.change_clip_deletion(request))
            .await
    }
    pub async fn mark_used(&self, id: String) -> Result<(), AppError> {
        self.operation(move |s| s.mark_used(&id)).await
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
