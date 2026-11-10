//! Scope-bound transport limits and a size and transfer-rate budget for upload bodies.

use axum::{body::{Body, Bytes}, http::StatusCode, response::{IntoResponse, Response}};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::ratelimit::{LargeBodies, LargeBody};
use http_body_util::BodyExt;
use std::{collections::VecDeque, time::Duration};
use tokio::time::Instant;

// Keep these aligned with frontend/src/lib/upload-budget.ts.
const GRACE: Duration = Duration::from_secs(15);
const WINDOW: Duration = Duration::from_secs(10);
const MIN_RATE: usize = 64 * 1024;
const MAX_READ: Duration = Duration::from_secs(15 * 60);
const RESOLUTION: Duration = Duration::from_secs(1);

struct UploadBudget {
    start: Instant,
    ceiling: Duration,
    buckets: VecDeque<(Instant, usize)>,
}

impl UploadBudget {
    fn new(limit: usize) -> Self {
        Self { start: Instant::now(), ceiling: MAX_READ.min(GRACE + Duration::from_secs(limit.div_ceil(MIN_RATE) as u64)), buckets: VecDeque::new() }
    }

    fn prune(&mut self, now: Instant) {
        // One-second buckets bound memory even for tiny, frequent frames. Include the
        // boundary bucket so scheduling jitter cannot penalise a sender at the minimum.
        while self.buckets.front().is_some_and(|(at, _)| now.duration_since(*at) >= WINDOW + RESOLUTION) {
            self.buckets.pop_front();
        }
    }

    fn record(&mut self, bytes: usize) {
        let now = Instant::now();
        self.prune(now);
        if let Some((at, size)) = self.buckets.back_mut()
            && now.duration_since(*at) < RESOLUTION {
            *size = size.saturating_add(bytes);
        } else {
            self.buckets.push_back((now, bytes));
        }
    }

    fn expired(&mut self) -> bool {
        let now = Instant::now();
        self.prune(now);
        now.duration_since(self.start) >= self.ceiling ||
            (now.duration_since(self.start) >= GRACE + WINDOW && self.buckets.iter().map(|(_, size)| size).sum::<usize>() < MIN_RATE * WINDOW.as_secs() as usize)
    }
}

pub(super) fn allowed() -> bool {
    crate::authz::current().is_some_and(|c| c.allows(crate::scopes::for_tool("document_upload", false)))
}

pub(super) fn preflight(limit: usize, upload: bool) -> Response {
    ([("x-eunomia-body-limit", limit.to_string()), ("x-eunomia-upload-allowed", if upload { "1" } else { "0" }.to_string())], StatusCode::OK).into_response()
}

pub(super) fn scope_denied() -> AppError {
    AppError::coded(ErrorCode::AuthScope, "document upload requires memory:write")
}

fn timed_out() -> AppError {
    AppError::new(StatusCode::REQUEST_TIMEOUT, "upload body exceeded its size or minimum transfer-rate budget")
}

/// Read the small budget without a slot. Acquire it before retaining any excess bytes;
/// an unscoped MCP caller is passed only the small limit and can never escalate.
pub(super) async fn read(body: Body, limit: usize, bodies: &LargeBodies, user: String) -> AppResult<(Bytes, Option<LargeBody<'_>>)> {
    let mut budget = UploadBudget::new(limit);
    let deadline = budget.start + budget.ceiling;
    tokio::time::timeout_at(deadline, async move {
        let mut body = body;
        let mut bytes = Vec::new();
        let mut slot = None;
        let normal_limit = crate::max_body_bytes();
        let mut tick = tokio::time::interval_at(budget.start + GRACE + WINDOW, RESOLUTION);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let frame = tokio::select! {
                biased;
                _ = tick.tick() => {
                    if budget.expired() { return Err(timed_out()); }
                    continue;
                }
                frame = body.frame() => frame,
            };
            let Some(frame) = frame else { break };
            let frame = frame.map_err(|_| AppError::bad_request("could not read upload body"))?;
            let Ok(chunk) = frame.into_data() else { continue };
            if budget.expired() { return Err(timed_out()); }
            let size = bytes.len().saturating_add(chunk.len());
            if size > limit {
                return Err(AppError::coded(ErrorCode::DocumentTooLarge, format!("request body exceeds the {limit} byte transport limit")));
            }
            if size > normal_limit && slot.is_none() {
                slot = Some(bodies.acquire(user.clone()).ok_or_else(|| AppError::new(StatusCode::TOO_MANY_REQUESTS, "another large upload is in progress"))?);
            }
            budget.record(chunk.len());
            bytes.extend_from_slice(&chunk);
        }
        Ok((Bytes::from(bytes), slot))
    }).await.map_err(|_| timed_out())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn upload_budget_allows_25_mib_past_two_minutes_and_scales_ceiling() {
        let mut budget = UploadBudget::new(25 << 20);
        assert_eq!(budget.ceiling, Duration::from_secs(415));
        for _ in 0..200 {
            budget.record(128 * 1024);
            assert!(!budget.expired());
            tokio::time::advance(RESOLUTION).await;
        }
        assert_eq!(UploadBudget::new(100 << 20).ceiling, MAX_READ);
        assert_eq!(UploadBudget::new(2 << 20).ceiling, Duration::from_secs(47));
    }
}
