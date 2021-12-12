use anyhow::Result;

mod cf_dns;
mod control;
mod reconcile;
mod spec;
mod trace;

pub async fn run() -> Result<()> {
    let _stop_guard = trace::init_tracing()?;

    control::run_scheduler().await?;

    Ok(())
}
