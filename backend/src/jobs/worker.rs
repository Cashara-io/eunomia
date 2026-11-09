//! The worker loop: claim due jobs, run each under its lease, heartbeat, finish or fail.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tracing::{field::Empty, Instrument};

use super::{Job, JobError, WorkerConfig};
use crate::error::ErrorCode;
use crate::rid::RecordIdExt;
use crate::state::AppState;

type Handler = Arc<dyn Fn(AppState, Job) -> BoxFuture<'static, Result<(), JobError>> + Send + Sync>;

/// Job kind to handler. A handler must be safe to run twice (a lease can expire mid-run).
#[derive(Default, Clone)]
pub struct Registry(HashMap<String, Handler>);

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F, Fut>(mut self, kind: &str, f: F) -> Self
    where
        F: Fn(AppState, Job) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        self.0.insert(kind.to_string(), Arc::new(move |s, j| f(s, j).boxed()));
        self
    }
}

/// Run until `shutdown` flips to true, then finish or release what is in flight. Aborting the
/// returned future (a crash, in tests) leaves leases to expire.
pub async fn run(state: AppState, registry: Registry, cfg: WorkerConfig, mut shutdown: watch::Receiver<bool>) {
    let wake = Arc::new(Notify::new());
    let _live = AbortOnDrop(tokio::spawn(crate::store::jobs::live_wake(state.control.clone(), wake.clone(), shutdown.clone())));
    let slots = Arc::new(Semaphore::new(cfg.concurrency));
    let mut running = JoinSet::new();
    tracing::info!(name: "diagnostics.worker_started", worker = %cfg.id, concurrency = cfg.concurrency, "job worker started");

    while !*shutdown.borrow() {
        while running.try_join_next().is_some() {}
        let free = slots.available_permits();
        let mut full_batch = false;
        if free > 0 {
            match super::claim(&state.control, &cfg.id, free, cfg.lease, cfg.owner_cap).await {
                Ok(jobs) => {
                    full_batch = jobs.len() == free;
                    for job in jobs {
                        let permit = slots.clone().acquire_owned().await.expect("semaphore is never closed");
                        running.spawn(run_one(state.clone(), registry.clone(), cfg.clone(), job, wake.clone(), permit));
                    }
                }
                Err(e) => tracing::warn!(worker = %cfg.id, error = %e.message, "job claim failed"),
            }
        }
        if full_batch {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(cfg.poll) => {}
            _ = wake.notified() => {}
            _ = shutdown.changed() => {}
        }
    }

    let drained = tokio::time::timeout(cfg.grace, async { while running.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        running.abort_all();
        while running.join_next().await.is_some() {}
    }
    match super::release_worker(&state.control, &cfg.id).await {
        Ok(n) => tracing::info!(name: "diagnostics.worker_stopped", worker = %cfg.id, released = n, "job worker stopped"),
        Err(e) => tracing::warn!(worker = %cfg.id, error = %e.message, "releasing leases failed; they will expire"),
    }
}

/// Stops the LIVE listener when `run` ends, including when `run` itself is aborted.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run_one(state: AppState, registry: Registry, cfg: WorkerConfig, job: Job, wake: Arc<Notify>, _slot: OwnedSemaphorePermit) {
    let span = tracing::info_span!(
        "job.attempt",
        job.id = %job.id.to_string(), job.kind = %job.kind, job.attempt = job.attempts, job.owner = %job.owner.to_string(), trace_id = Empty
    );
    let trace_id = job.traceparent.as_deref().and_then(crate::telemetry::trace_id_of);
    if let Some(tp) = &job.traceparent {
        crate::telemetry::link_span(&span, tp);
    }
    let trace_id = trace_id.unwrap_or_else(crate::telemetry::current_trace_id);
    span.record("trace_id", trace_id.as_str());
    crate::telemetry::with_trace_id(trace_id, attempt(&state, &registry, &cfg, &job).instrument(span)).await;
    wake.notify_one(); // a slot is free: look for more work now
}

async fn attempt(state: &AppState, registry: &Registry, cfg: &WorkerConfig, job: &Job) {
    let Some(handler) = registry.0.get(&job.kind).cloned() else {
        let err = JobError::permanent(ErrorCode::JobNoHandler, format!("no handler for job kind {:?}", job.kind));
        return finish(state, cfg, job, Err(err)).await;
    };
    let work = AssertUnwindSafe(crate::authz::as_system(async { handler(state.clone(), job.clone()).await })).catch_unwind();
    let outcome = tokio::select! {
        r = work => Some(r),
        _ = keep_lease(state, cfg, job) => None,
    };
    match outcome {
        None => tracing::warn!("job lease lost; abandoning this attempt"), // another worker owns it now
        Some(Ok(r)) => finish(state, cfg, job, r).await,
        Some(Err(_)) => {
            let err = JobError::retryable(ErrorCode::JobPanicked, "job handler panicked");
            finish(state, cfg, job, Err(err)).await
        }
    }
}

/// Heartbeats the lease; returns only when it is lost, which cancels the handler.
async fn keep_lease(state: &AppState, cfg: &WorkerConfig, job: &Job) {
    loop {
        tokio::time::sleep(cfg.lease / 3).await;
        match super::heartbeat(&state.control, job, &cfg.id, cfg.lease).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => tracing::warn!(error = %e.message, "job heartbeat failed"), // the lease may still hold; try again
        }
    }
}

async fn finish(state: &AppState, cfg: &WorkerConfig, job: &Job, result: Result<(), JobError>) {
    match result {
        Ok(()) => match super::complete(&state.control, job, &cfg.id).await {
            Ok(true) => tracing::info!(name: "diagnostics.job_done", "job done"),
            Ok(false) => tracing::warn!("job finished after losing its lease"),
            Err(e) => tracing::warn!(error = %e.message, "recording job completion failed; the lease will expire and it will rerun"),
        },
        Err(err) => match super::fail(&state.control, job, &cfg.id, &err, cfg.retry_base).await {
            Ok(status) => tracing::warn!(code = err.code.as_str(), error = %err.message, status = status.as_deref().unwrap_or("lost"), "job failed"),
            Err(e) => tracing::warn!(error = %e.message, "recording job failure failed; the lease will expire and it will rerun"),
        },
    }
}
