use anyhow::Context;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(filter: &str, json: bool) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(filter).context("invalid RUST_LOG filter")?;
    let registry = tracing_subscriber::registry().with(filter);

    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?;
    } else {
        registry.with(tracing_subscriber::fmt::layer()).try_init()?;
    }

    Ok(())
}
