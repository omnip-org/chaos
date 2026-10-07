use chaos_core::runtime::lifecycle::Lifecycle;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::runtime::WorkerRuntime;

pub async fn run(
    runtime: WorkerRuntime,
    lifecycle: Lifecycle,
    worker_shutdown_timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let mut workers = JoinSet::new();
    workers.spawn({
        let email_workers = runtime.email_workers.clone();
        let lifecycle = lifecycle.clone();
        async move {
            email_worker_loop(email_workers, lifecycle).await;
            "email"
        }
    });
    workers.spawn({
        let provider_webhook_worker = runtime.provider_webhook_worker.clone();
        let clock = runtime.clock.clone();
        let lifecycle = lifecycle.clone();
        async move {
            provider_webhook_worker_loop(provider_webhook_worker, clock, lifecycle).await;
            "provider-webhook"
        }
    });
    workers.spawn({
        let capi_worker = runtime.capi_worker.clone();
        let lifecycle = lifecycle.clone();
        async move {
            capi_worker_loop(capi_worker, lifecycle).await;
            "capi"
        }
    });
    workers.spawn({
        let search_indexer = runtime.search_indexer.clone();
        let clock = runtime.clock.clone();
        let lifecycle = lifecycle.clone();
        async move {
            search_worker_loop(search_indexer, clock, lifecycle).await;
            "search"
        }
    });
    workers.spawn({
        let maintenance = runtime.maintenance.clone();
        let lifecycle = lifecycle.clone();
        async move {
            maintenance_worker_loop(maintenance, lifecycle).await;
            "maintenance"
        }
    });
    tracing::info!("background worker started");
    let stop_error = tokio::select! {
        () = shutdown_signal() => None,
        result = workers.join_next() => Some(unexpected_worker_error(result)),
    };
    lifecycle.begin_draining();
    if stop_error.is_none() {
        tracing::info!("shutdown signal received; worker is draining");
    }
    let drain_error = drain_workers(&mut workers, worker_shutdown_timeout).await;
    match stop_error.or(drain_error) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn provider_webhook_worker_loop(
    worker: std::sync::Arc<chaos_core::webhooks::ProviderWebhookWorker>,
    clock: std::sync::Arc<dyn chaos_core::contracts::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let processed = match worker.run_batch(clock.now(), 10).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "provider webhook batch failed");
                0
            }
        };
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

async fn email_worker_loop(
    workers: std::sync::Arc<chaos_core::email::EmailWorkers>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let processed = match workers.run_outbox_batch(10).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "email outbox batch failed");
                0
            }
        };
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

struct PollBackoff {
    current: std::time::Duration,
}

impl PollBackoff {
    const BASE: std::time::Duration = std::time::Duration::from_millis(250);
    const MAX: std::time::Duration = std::time::Duration::from_secs(5);

    fn new() -> Self {
        Self {
            current: Self::BASE,
        }
    }

    /// Returns how long to sleep before the next poll, then updates state for
    /// the following call: any processed work resets the interval to the
    /// base so a busy queue keeps draining at full speed, while an idle poll
    /// doubles the interval up to `MAX` so an empty queue stops hammering
    /// Postgres every 250ms.
    fn observe(&mut self, processed: usize) -> std::time::Duration {
        let sleep_for = self.current;
        self.current = if processed > 0 {
            Self::BASE
        } else {
            std::cmp::min(self.current * 2, Self::MAX)
        };
        sleep_for
    }
}

async fn capi_worker_loop(
    worker: std::sync::Arc<chaos_core::analytics::MetaCapiWorker>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let processed = match worker.run_batch(10).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(%worker_id, error = ?error, "capi delivery batch failed");
                0
            }
        };
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

fn unexpected_worker_error(
    result: Option<Result<&'static str, tokio::task::JoinError>>,
) -> anyhow::Error {
    match result {
        Some(Ok(worker)) => {
            tracing::error!(worker, "worker loop stopped unexpectedly");
            anyhow::anyhow!("{worker} worker loop stopped unexpectedly")
        }
        Some(Err(error)) => {
            tracing::error!(%error, "worker task failed unexpectedly");
            anyhow::anyhow!("worker task failed unexpectedly: {error}")
        }
        None => {
            tracing::error!("all worker tasks stopped unexpectedly");
            anyhow::anyhow!("all worker tasks stopped unexpectedly")
        }
    }
}

async fn drain_workers(
    workers: &mut JoinSet<&'static str>,
    timeout: std::time::Duration,
) -> Option<anyhow::Error> {
    let drain = async {
        let mut first_error = None;
        while let Some(result) = workers.join_next().await {
            match result {
                Ok(worker) => tracing::info!(worker, "worker drained"),
                Err(error) => {
                    tracing::error!(%error, "worker task failed while draining");
                    first_error.get_or_insert_with(|| {
                        anyhow::anyhow!("worker task failed while draining: {error}")
                    });
                }
            }
        }
        first_error
    };
    match tokio::time::timeout(timeout, drain).await {
        Ok(error) => error,
        Err(_) => {
            tracing::warn!(
                ?timeout,
                remaining = workers.len(),
                "worker drain timed out; aborting remaining tasks"
            );
            workers.abort_all();
            while workers.join_next().await.is_some() {}
            None
        }
    }
}

async fn search_worker_loop(
    indexer: std::sync::Arc<chaos_core::adapters::postgres::PostgresSearchIndexer>,
    clock: std::sync::Arc<dyn chaos_core::contracts::Clock>,
    lifecycle: Lifecycle,
) {
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let processed = match indexer.run_batch(100, clock.now()).await {
            Ok(count) => count as usize,
            Err(error) => {
                tracing::warn!(%error, "search indexing batch failed");
                0
            }
        };
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

async fn maintenance_worker_loop(
    maintenance: std::sync::Arc<chaos_core::adapters::postgres::PostgresMaintenance>,
    lifecycle: Lifecycle,
) {
    const CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

    let mut next_cleanup = tokio::time::Instant::now();
    while lifecycle.is_accepting_traffic() {
        if tokio::time::Instant::now() >= next_cleanup {
            match maintenance.cleanup_expired().await {
                Ok(deleted) if deleted > 0 => {
                    tracing::info!(deleted, "expired and terminal records cleaned up")
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "expired and terminal record cleanup failed"),
            }
            next_cleanup = tokio::time::Instant::now() + CLEANUP_INTERVAL;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {}
            Err(error) => tracing::error!(%error, "failed to receive Ctrl+C signal"),
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tokio::task::JoinSet;

    use super::drain_workers;

    struct DropSignal(Arc<AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn worker_drain_waits_for_normal_completion() {
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            worker_completed.store(true, Ordering::SeqCst);
            "test"
        });

        let error = drain_workers(&mut workers, std::time::Duration::from_secs(1)).await;

        assert!(completed.load(Ordering::SeqCst));
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn worker_drain_aborts_after_the_bounded_timeout() {
        let dropped = Arc::new(AtomicBool::new(false));
        let worker_dropped = dropped.clone();
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            let _drop_signal = DropSignal(worker_dropped);
            std::future::pending::<()>().await;
            "test"
        });
        tokio::task::yield_now().await;

        let error = drain_workers(&mut workers, std::time::Duration::from_millis(1)).await;

        assert!(dropped.load(Ordering::SeqCst));
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn worker_drain_reports_panics() {
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            panic!("worker failed");
            #[allow(unreachable_code)]
            "test"
        });

        let error = drain_workers(&mut workers, std::time::Duration::from_secs(1)).await;

        assert!(error.is_some());
    }
}
