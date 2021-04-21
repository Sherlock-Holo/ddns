use anyhow::Result;

mod cf_dns;
mod control;
mod reconcile;
mod spec;
mod trace;

pub async fn run() -> Result<()> {
    trace::init_tracing()?;

    control::run_controller().await?;

    trace::stop_tracing();

    Ok(())
}
