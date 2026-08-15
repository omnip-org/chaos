use anyhow::Context;
use chaos_api::{
    http::{self, ApiState},
    lifecycle::Lifecycle,
    telemetry,
};
use chaos_infrastructure::{config::Settings, state::AppState};
use tokio::net::TcpListener;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    let worker_shutdown_timeout = settings.shutdown_worker_timeout;
    let trace_provider = telemetry::init(&settings.log_filter, settings.log_json)?;

    let lifecycle = Lifecycle::new();
    let state = ApiState::new(AppState::new(&settings)?, lifecycle.clone(), &settings)?;
    let payment_worker = tokio::spawn(payment_worker_loop(
        state.payment_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let fulfillment_worker = tokio::spawn(fulfillment_worker_loop(
        state.fulfillment_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let notification_worker = tokio::spawn(notification_worker_loop(
        state.notification_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let analytics_worker = tokio::spawn(analytics_worker_loop(
        state.analytics_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let search_worker = tokio::spawn(search_worker_loop(
        state.search_indexer.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let checkout_expiry_worker = tokio::spawn(checkout_expiry_worker_loop(
        state.checkout_expiry_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let app = http::router(state);
    let listener = TcpListener::bind(settings.bind_addr)
        .await
        .with_context(|| format!("failed to bind HTTP server to {}", settings.bind_addr))?;

    tracing::info!(address = %settings.bind_addr, "HTTP server started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(lifecycle, settings.shutdown_drain_delay))
        .await
        .context("HTTP server failed")?;
    tokio::join!(
        drain_worker("payment", payment_worker, worker_shutdown_timeout),
        drain_worker("fulfillment", fulfillment_worker, worker_shutdown_timeout),
        drain_worker("notification", notification_worker, worker_shutdown_timeout),
        drain_worker("analytics", analytics_worker, worker_shutdown_timeout),
        drain_worker("search", search_worker, worker_shutdown_timeout),
        drain_worker(
            "checkout-expiry",
            checkout_expiry_worker,
            worker_shutdown_timeout
        ),
    );
    if let Some(provider) = trace_provider {
        provider
            .shutdown()
            .context("failed to shut down trace exporter")?;
    }

    Ok(())
}

async fn analytics_worker_loop(
    workers: std::sync::Arc<chaos_application::analytics::AnalyticsWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    let mut next_retention_at = time::OffsetDateTime::UNIX_EPOCH;
    let mut next_erasure_at = time::OffsetDateTime::UNIX_EPOCH;
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        if let Err(error) = workers.run_sessionization_batch(worker_id, now, 100).await {
            tracing::warn!(%worker_id, %error, "analytics sessionization batch failed");
        }
        if now >= next_retention_at {
            match workers.run_retention_batch(now, 1000).await {
                Ok(result) => {
                    ::metrics::counter!("chaos_analytics_retention_behavior_events_deleted_total")
                        .increment(result.behavior_events_deleted);
                    ::metrics::counter!("chaos_analytics_retention_sessions_deleted_total")
                        .increment(result.sessions_deleted);
                    ::metrics::counter!("chaos_analytics_retention_identity_links_deleted_total")
                        .increment(result.identity_links_deleted);
                    next_retention_at = now + time::Duration::minutes(1);
                }
                Err(error) => {
                    tracing::warn!(%worker_id, %error, "analytics retention batch failed");
                    next_retention_at = now + time::Duration::seconds(5);
                }
            }
        }
        if now >= next_erasure_at {
            match workers.run_erasure_batch(now, 100).await {
                Ok(result) => {
                    ::metrics::counter!("chaos_analytics_erasure_requests_completed_total")
                        .increment(result.requests_completed);
                    ::metrics::counter!("chaos_analytics_erasure_behavior_events_deleted_total")
                        .increment(result.behavior_events_deleted);
                    ::metrics::counter!("chaos_analytics_erasure_sessions_deleted_total")
                        .increment(result.sessions_deleted);
                    ::metrics::counter!("chaos_analytics_erasure_identity_links_deleted_total")
                        .increment(result.identity_links_deleted);
                    next_erasure_at = now + time::Duration::seconds(1);
                }
                Err(error) => {
                    tracing::warn!(%worker_id, %error, "analytics erasure batch failed");
                    next_erasure_at = now + time::Duration::seconds(5);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn notification_worker_loop(
    workers: std::sync::Arc<chaos_application::notifications::NotificationWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    while lifecycle.is_accepting_traffic() {
        if let Err(error) = workers.run_batch(worker_id, clock.now(), 50).await {
            tracing::warn!(%worker_id, %error, "notification delivery batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
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
    let worker_id = Uuid::now_v7();
    while lifecycle.is_accepting_traffic() {
        if let Err(error) = indexer.run_batch(worker_id, 100, clock.now()).await {
            tracing::warn!(%worker_id, %error, "search indexing batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn payment_worker_loop(
    workers: std::sync::Arc<chaos_application::payments::PaymentWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        if let Err(error) = workers.run_outbox_batch(worker_id, now, 50).await {
            tracing::warn!(%worker_id, %error, "payment outbox batch failed");
        }
        if let Err(error) = workers.run_webhook_batch(worker_id, now, 50).await {
            tracing::warn!(%worker_id, %error, "payment webhook batch failed");
        }
        if let Err(error) = workers.run_readiness_batch(worker_id, now, 25).await {
            tracing::warn!(%worker_id, %error, "Payment Provider readiness batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn fulfillment_worker_loop(
    workers: std::sync::Arc<chaos_application::fulfillment::FulfillmentWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    while lifecycle.is_accepting_traffic() {
        let now = clock.now();
        if let Err(error) = workers.run_batch(worker_id, now, 50).await {
            tracing::warn!(%worker_id, %error, "fulfillment event batch failed");
        }
        if let Err(error) = workers.run_tracking_batch(worker_id, now, 25).await {
            tracing::warn!(%worker_id, %error, "shipping tracking batch failed");
        }
        if let Err(error) = workers.run_cancellation_batch(worker_id, now, 25).await {
            tracing::warn!(%worker_id, %error, "shipping cancellation batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn checkout_expiry_worker_loop(
    workers: std::sync::Arc<chaos_application::sales::CheckoutExpiryWorkers>,
    clock: std::sync::Arc<dyn chaos_application::ports::Clock>,
    lifecycle: Lifecycle,
) {
    let worker_id = Uuid::now_v7();
    while lifecycle.is_accepting_traffic() {
        if let Err(error) = workers.run_batch(worker_id, clock.now(), 100).await {
            tracing::warn!(%worker_id, %error, "Checkout expiry batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn shutdown_signal(lifecycle: Lifecycle, drain_delay: std::time::Duration) {
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
    tracing::info!(
        ?drain_delay,
        "shutdown signal received; instance is draining"
    );
    tokio::time::sleep(drain_delay).await;
    tracing::info!("load-balancer drain delay elapsed; stopping HTTP listener");
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
