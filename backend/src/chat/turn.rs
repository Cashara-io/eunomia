//! A database lease fences every history append, including requests left in flight on disconnect.

use serde_json::Value;
use surrealdb::types::RecordId;
use crate::{error::{AppError, AppResult, ErrorCode}, pool::OrgDb, store, tx};

const LEASE: &str = "30s";
pub(super) const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) struct Turn {
    pub(super) thread: RecordId,
    owner: RecordId,
    token: String,
}

impl Turn {
    pub(super) fn new(thread: &RecordId, owner: &RecordId) -> Self {
        Self { thread: thread.clone(), owner: owner.clone(), token: uuid::Uuid::new_v4().to_string() }
    }

    pub(super) async fn acquire(&self, db: &OrgDb) -> AppResult<()> {
        // New code must not send against N-1 while the tenant migration is pending.
        if db.schema_version() < 43 {
            return Err(AppError::coded(ErrorCode::TenantSchemaBehind, "Chat is waiting for a database upgrade. Please try again shortly."));
        }
        loop {
            let claimed = tx::with_retry(|| async {
                let mut res = store::app::CHAT_TURN_CLAIM.on(db)
                    .bind(("thread_id", self.thread.clone())).bind(("owner", self.owner.clone()))
                    .bind(("turn_key", self.token.clone())).bind(("lease", LEASE)).await?;
                res.take::<Vec<RecordId>>(0)
            }).await?;
            if !claimed.is_empty() { return Ok(()); }
            #[cfg(any(test, feature = "test-support"))]
            crate::sources::mock::chat_write_checkpoint(&self.thread, "turn_waiting").await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn keep_alive(&self, db: &OrgDb) -> AppResult<()> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let renewed = tx::with_retry(|| async {
                let mut res = store::app::CHAT_TURN_RENEW.on(db)
                    .bind(("thread_id", self.thread.clone())).bind(("owner", self.owner.clone()))
                    .bind(("turn_key", self.token.clone())).bind(("lease", LEASE)).await?;
                res.take::<Vec<RecordId>>(0)
            }).await?;
            if renewed.is_empty() {
                return Err(AppError::coded(ErrorCode::DbConflict, "This reply lost its turn. Please send your message again."));
            }
        }
    }

    pub(super) async fn append(&self, db: &OrgDb, messages: Vec<Value>) -> AppResult<()> {
        tx::with_retry(|| async {
            store::app::CHAT_TURN_APPEND.on(db)
                .bind(("thread_id", self.thread.clone())).bind(("owner", self.owner.clone()))
                .bind(("turn_key", self.token.clone())).bind(("lease", LEASE))
                .bind(("messages", messages.clone())).await?.check()?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub(super) async fn release(&self, db: &OrgDb) {
        // This UPDATE conflicts with an abandoned append's thread write. That append either
        // commits before release or loses its token, even if the SDK future was already dropped.
        let result = tx::with_retry(|| async {
            store::app::CHAT_TURN_RELEASE.on(db)
                .bind(("thread_id", self.thread.clone())).bind(("owner", self.owner.clone()))
                .bind(("turn_key", self.token.clone())).await?.check()?;
            Ok(())
        }).await;
        if let Err(error) = result { tracing::warn!(%error, "chat turn release failed; lease will expire"); }
    }
}
