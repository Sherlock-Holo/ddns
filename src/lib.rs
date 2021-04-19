use anyhow::Result;

mod cf_dns;
mod control;
mod reconcile;
mod spec;

pub async fn run() -> Result<()> {
    control::run_controller().await
}
