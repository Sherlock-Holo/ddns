use anyhow::Result;
use opentelemetry::global;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

pub fn init_tracing() -> Result<()> {
    global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());

    let tracer = opentelemetry_jaeger::new_pipeline()
        .with_service_name("ddns")
        .install_simple()?;

    let stderr_subscriber = tracing_subscriber::fmt::layer().json().with_target(true);
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
