//! In-process extension contract. A trusted host assigns capabilities when it opens a session.
//! Requests cannot nominate an identity, another record, or grant their own capabilities.

use crate::{
    history::{allowed_by_capture_policy, mutation_revision, read_capture_policy},
    AppError, ClipPayload, ClipVersion, CoreService, ErrorCode, Storage,
};

pub const VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("Unsupported extension contract version. The host supports version {supported}.")]
pub struct UnsupportedVersion {
    pub supported: u32,
}

/// Grants are carried by the host session. An empty grant denies every operation.
#[derive(Default)]
pub struct ExtensionGrants {
    pub selected_text: Option<ClipVersion>,
    pub create_text: bool,
    pub observe_history: bool,
}

pub struct ExtensionSession {
    core: CoreService,
    grants: ExtensionGrants,
}

impl ExtensionSession {
    pub fn open(
        core: CoreService,
        version: u32,
        grants: ExtensionGrants,
    ) -> Result<Self, UnsupportedVersion> {
        if version != VERSION {
            return Err(UnsupportedVersion { supported: VERSION });
        }
        Ok(Self { core, grants })
    }

    pub async fn read_selected_text(&self) -> Result<String, AppError> {
        let clip = self.grants.selected_text.clone().ok_or_else(restricted)?;
        self.core
            .operation(move |storage| {
                let snapshot = storage.connection.unchecked_transaction()?;
                let revision = mutation_revision(&clip.expected_revision)?;
                let payload = Storage::read_active_clip(&snapshot, &clip.id, Some(revision))?;
                if !allowed_by_capture_policy(&payload, &read_capture_policy(&snapshot)?)? {
                    return Err(restricted());
                }
                match payload {
                    ClipPayload::Text(text) => Ok(text),
                    _ => Err(restricted()),
                }
            })
            .await
    }

    /// Uses the same admission rules as a manually created record.
    pub async fn create_text(&self, text: String) -> Result<ClipVersion, AppError> {
        if !self.grants.create_text {
            return Err(restricted());
        }
        self.core
            .create_clip_version(ClipPayload::Text(text), None)
            .await
    }

    pub async fn subscribe_history(&self) -> Result<HistorySubscription<'_>, AppError> {
        if !self.grants.observe_history {
            return Err(restricted());
        }
        Ok(HistorySubscription {
            core: &self.core,
            revision: self.core.history_revision().await?,
        })
    }
}

/// Reads coalesced invalidation hints. Dropping this handle ends the subscription.
pub struct HistorySubscription<'a> {
    core: &'a CoreService,
    revision: String,
}

impl HistorySubscription<'_> {
    pub async fn poll_change(&mut self) -> Result<Option<String>, AppError> {
        let current = self.core.history_revision().await?;
        if self.revision == current {
            return Ok(None);
        }
        self.revision = current;
        Ok(Some(self.revision.clone()))
    }
}

fn restricted() -> AppError {
    AppError::new(ErrorCode::Restricted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UpdateCapturePolicy;

    #[tokio::test]
    async fn host_grants_are_scoped_to_a_session_and_a_record_version() {
        let dir = tempfile::tempdir().unwrap();
        let core = CoreService::open(dir.path().join("extension.sqlite3"))
            .await
            .unwrap();
        let denied =
            ExtensionSession::open(core.clone(), VERSION, ExtensionGrants::default()).unwrap();
        assert!(matches!(
            ExtensionSession::open(core.clone(), VERSION + 1, ExtensionGrants::default()),
            Err(UnsupportedVersion { supported: VERSION })
        ));
        assert_eq!(
            denied.read_selected_text().await.unwrap_err().code,
            ErrorCode::Restricted
        );
        assert_eq!(
            denied.create_text("denied".into()).await.unwrap_err().code,
            ErrorCode::Restricted
        );
        assert!(
            matches!(denied.subscribe_history().await, Err(error) if error.code == ErrorCode::Restricted)
        );

        let writer = ExtensionSession::open(
            core.clone(),
            VERSION,
            ExtensionGrants {
                create_text: true,
                observe_history: true,
                ..ExtensionGrants::default()
            },
        )
        .unwrap();
        let mut changes = writer.subscribe_history().await.unwrap();
        assert_eq!(changes.poll_change().await.unwrap(), None);
        let clip = writer.create_text("allowed text".into()).await.unwrap();
        assert_eq!(
            changes.poll_change().await.unwrap(),
            Some(core.history_revision().await.unwrap())
        );
        assert_eq!(changes.poll_change().await.unwrap(), None);
        assert_eq!(
            writer.read_selected_text().await.unwrap_err().code,
            ErrorCode::Restricted
        );
        drop(changes);

        let reader = ExtensionSession::open(
            core.clone(),
            VERSION,
            ExtensionGrants {
                selected_text: Some(clip.clone()),
                ..ExtensionGrants::default()
            },
        )
        .unwrap();
        assert_eq!(reader.read_selected_text().await.unwrap(), "allowed text");
        core.update_capture_policy(UpdateCapturePolicy {
            enabled: true,
            exclude_pattern: "allowed|blocked".into(),
            max_items: 500,
            expected_revision: "0".into(),
        })
        .await
        .unwrap();
        assert_eq!(
            reader.read_selected_text().await.unwrap_err().code,
            ErrorCode::Restricted
        );
        assert_eq!(
            writer.create_text("blocked".into()).await.unwrap_err().code,
            ErrorCode::Excluded
        );
        core.mark_used(clip.id).await.unwrap();
        assert_eq!(
            reader.read_selected_text().await.unwrap_err().code,
            ErrorCode::Conflict
        );
        core.shutdown().await.unwrap();
    }
}
