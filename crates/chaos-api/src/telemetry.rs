use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

static METRICS: OnceLock<PrometheusHandle> = OnceLock::new();
static METRICS_INIT: Mutex<()> = Mutex::new(());

pub fn init(
    service_name: &'static str,
    filter: &str,
    json: bool,
) -> anyhow::Result<Option<SdkTracerProvider>> {
    let filter = EnvFilter::try_new(filter).context("invalid RUST_LOG filter")?;
    let provider = if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build()
            .context("failed to create OTLP trace exporter")?;
        Some(
            SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(
                    opentelemetry_sdk::Resource::builder()
                        .with_service_name(service_name)
                        .build(),
                )
                .build(),
        )
    } else {
        None
    };
    let otel_layer = provider
        .as_ref()
        .map(|provider| tracing_opentelemetry::layer().with_tracer(provider.tracer(service_name)));
    let registry = tracing_subscriber::registry().with(filter).with(otel_layer);

    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?;
    } else {
        registry.with(tracing_subscriber::fmt::layer()).try_init()?;
    }

    Ok(provider)
}

pub fn init_metrics() -> anyhow::Result<PrometheusHandle> {
    if let Some(handle) = METRICS.get() {
        return Ok(handle.clone());
    }

    let _guard = METRICS_INIT
        .lock()
        .map_err(|_| anyhow::anyhow!("Prometheus metrics initialization lock was poisoned"))?;
    if let Some(handle) = METRICS.get() {
        return Ok(handle.clone());
    }

    let handle = PrometheusBuilder::new()
        .with_recommended_naming(true)
        .set_buckets_for_metric(
            Matcher::Full("chaos_http_request_duration_seconds".to_owned()),
            &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
        )
        .context("failed to configure HTTP request latency buckets")?
        .install_recorder()
        .context("failed to install Prometheus metrics recorder")?;

    let _ = METRICS.set(handle.clone());
    Ok(handle)
}
