use anyhow::Context;
use chaos_api::{http::ApiState, lifecycle::Lifecycle, telemetry, workers};
use chaos_infrastructure::{config::Settings, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    let trace_provider = telemetry::init("chaos-worker", &settings.log_filter, settings.log_json)?;
    let lifecycle = Lifecycle::new();
    let state = ApiState::new(AppState::new(&settings)?, lifecycle.clone(), &settings)?;

    workers::run(state, lifecycle, settings.shutdown_worker_timeout).await;
    if let Some(provider) = trace_provider {
        provider
            .shutdown()
            .context("failed to shut down trace exporter")?;
    }
    Ok(())
}
