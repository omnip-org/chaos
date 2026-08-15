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
    let trace_provider = telemetry::init(&settings.log_filter, settings.log_json)?;

    let lifecycle = Lifecycle::new();
    let state = ApiState::new(AppState::new(&settings)?, lifecycle.clone(), &settings)?;
    let payment_worker = tokio::spawn(payment_worker_loop(
        state.payment_workers.clone(),
        state.clock.clone(),
        lifecycle.clone(),
    ));
    let search_worker = tokio::spawn(search_worker_loop(
        state.search_indexer.clone(),
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
    payment_worker.abort();
    search_worker.abort();
    if let Some(provider) = trace_provider {
        provider
            .shutdown()
            .context("failed to shut down trace exporter")?;
    }

    Ok(())
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
