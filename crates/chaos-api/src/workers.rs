use crate::{lifecycle::Lifecycle, runtime::WorkerRuntime};
use uuid::Uuid;

pub async fn run(
    runtime: WorkerRuntime,
    lifecycle: Lifecycle,
    worker_shutdown_timeout: std::time::Duration,
) {
    let payment_worker = tokio::spawn(payment_worker_loop(
        runtime.payment_workers.clone(),
        runtime.clock.clone(),
        lifecycle.clone(),
    ));
    let shipping_event_worker = tokio::spawn(shipping_event_worker_loop(
        runtime.shipping_event_workers.clone(),
        runtime.clock.clone(),
        lifecycle.clone(),
    ));
    let analytics_worker = tokio::spawn(analytics_worker_loop(
        runtime.analytics_delivery_worker.clone(),
        runtime.clock.clone(),
        lifecycle.clone(),
    ));
    let search_worker = tokio::spawn(search_worker_loop(
        runtime.search_indexer.clone(),
        runtime.clock.clone(),
        lifecycle.clone(),
    ));
    tracing::info!("background worker started");
    shutdown_signal(lifecycle).await;
    tokio::join!(
        drain_worker("payment", payment_worker, worker_shutdown_timeout),
        drain_worker(
            "shipping-events",
            shipping_event_worker,
            worker_shutdown_timeout
        ),
        drain_worker("analytics", analytics_worker, worker_shutdown_timeout),
        drain_worker("search", search_worker, worker_shutdown_timeout),
    );
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

async fn analytics_worker_loop(
    delivery: std::sync::Arc<chaos_application::analytics::AnalyticsDeliveryWorker>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        let mut processed = 0usize;
        match delivery.run_delivery_batch(now, 10).await {
            Ok(count) => {
                processed += count;
            }
            Err(error) => {
                tracing::warn!(%worker_id, error = ?error, "analytics provider delivery batch failed");
            }
        }
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

async fn drain_worker(
    worker_name: &'static str,
    mut worker: tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) {
    match tokio::time::timeout(timeout, &mut worker).await {
        Ok(Ok(())) => tracing::info!(worker = worker_name, "worker drained"),
        Ok(Err(error)) => {
            tracing::warn!(worker = worker_name, %error, "worker stopped unexpectedly");
        }
        Err(_) => {
            tracing::warn!(
                worker = worker_name,
                ?timeout,
                "worker drain timed out; aborting task"
            );
            worker.abort();
            let _ = worker.await;
        }
    }
}

async fn search_worker_loop(
    indexer: std::sync::Arc<chaos_infrastructure::repositories::PostgresSearchIndexer>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
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

async fn payment_worker_loop(
    workers: std::sync::Arc<chaos_application::payments::PaymentWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        let mut processed = 0usize;
        match workers.run_outbox_batch(now, 10).await {
            Ok(count) => processed += count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "payment outbox batch failed");
            }
        }
        match workers.run_webhook_batch(now, 50).await {
            Ok(count) => processed += count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "payment webhook batch failed");
            }
        }
        match workers.run_readiness_batch(worker_id, now, 25).await {
            Ok(count) => processed += count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "Payment Provider readiness batch failed");
            }
        }
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

async fn shipping_event_worker_loop(
    workers: std::sync::Arc<chaos_application::shipping_events::ShippingEventWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut backoff = PollBackoff::new();
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        let mut processed = 0usize;
        match workers.run_batch(now, 50).await {
            Ok(count) => processed += count,
            Err(error) => {
                tracing::warn!(%worker_id, %error, "shipping event batch failed");
            }
        }
        tokio::time::sleep(backoff.observe(processed)).await;
    }
}

async fn shutdown_signal(lifecycle: Lifecycle) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    lifecycle.begin_draining();
    tracing::info!("shutdown signal received; worker is draining");
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::drain_worker;

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
        let worker = tokio::spawn(async move {
            worker_completed.store(true, Ordering::SeqCst);
        });

        drain_worker("test", worker, std::time::Duration::from_secs(1)).await;

        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn worker_drain_aborts_after_the_bounded_timeout() {
        let dropped = Arc::new(AtomicBool::new(false));
        let worker_dropped = dropped.clone();
        let worker = tokio::spawn(async move {
            let _drop_signal = DropSignal(worker_dropped);
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;

        drain_worker("test", worker, std::time::Duration::from_millis(1)).await;

        assert!(dropped.load(Ordering::SeqCst));
    }
}
