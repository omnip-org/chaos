use anyhow::Context;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

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
