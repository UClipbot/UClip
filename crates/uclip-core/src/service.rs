use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use crate::{AppError, ErrorCode, SettingsSnapshot, Storage, UpdateSettings};

const QUEUE_CAPACITY: usize = 64;
type Reply<T> = oneshot::Sender<Result<T, AppError>>;

enum Request {
    Settings(Reply<SettingsSnapshot>),
    Update(UpdateSettings, Reply<SettingsSnapshot>),
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
