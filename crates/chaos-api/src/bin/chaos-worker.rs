use anyhow::Context;
use chaos_api::{lifecycle::Lifecycle, runtime::WorkerRuntime, telemetry, workers};
use chaos_infrastructure::{config::Settings, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let settings = Settings::from_env()?;
    let trace_provider = telemetry::init("chaos-worker", &settings.log_filter, settings.log_json)?;
    let lifecycle = Lifecycle::new();
    let infrastructure = AppState::new(&settings)?;
    let runtime = WorkerRuntime::new(&infrastructure, &settings)?;

    workers::run(runtime, lifecycle, settings.shutdown_worker_timeout).await;
    if let Some(provider) = trace_provider {
        provider
            .shutdown()
            .context("failed to shut down trace exporter")?;
    }
    Ok(())
}
