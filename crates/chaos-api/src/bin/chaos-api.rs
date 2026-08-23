use anyhow::Context;
use chaos_api::http::{self, ApiState};
use chaos_core::runtime::{config::Settings, lifecycle::Lifecycle, state::AppState, telemetry};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    let trace_provider = telemetry::init("chaos-api", &settings.log_filter, settings.log_json)?;
    let lifecycle = Lifecycle::new();
    let state = ApiState::new(AppState::new(&settings)?, lifecycle.clone(), &settings)?;
    let app = http::router(state);
    let listener = TcpListener::bind(settings.bind_addr)
        .await
        .with_context(|| format!("failed to bind HTTP server to {}", settings.bind_addr))?;

    tracing::info!(address = %settings.bind_addr, "HTTP server started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(lifecycle, settings.shutdown_drain_delay))
        .await
        .context("HTTP server failed")?;
    if let Some(provider) = trace_provider {
        provider
            .shutdown()
            .context("failed to shut down trace exporter")?;
    }
    Ok(())
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
