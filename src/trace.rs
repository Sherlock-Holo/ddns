use std::env;

use anyhow::Result;
use opentelemetry::global;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

pub fn init_tracing() -> Result<()> {
    global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());

    let agent_addr = env::var("JAEGER_AGENT").unwrap_or_else(|_| "127.0.0.1:6831".to_string());

    let tracer = opentelemetry_jaeger::new_pipeline()
        .with_service_name("ddns")
        .with_agent_endpoint(agent_addr)
        .install_simple()?;

    let stderr_subscriber = tracing_subscriber::fmt::layer().pretty().with_target(true);
    let env_filter = tracing_subscriber::filter::EnvFilter::from_default_env();

    let trace = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default()
        .with(env_filter)
        .with(trace)
        .with(stderr_subscriber);

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(())
}

pub fn stop_tracing() {
    global::shutdown_tracer_provider();
}
