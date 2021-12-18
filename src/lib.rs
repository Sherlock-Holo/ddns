use anyhow::Result;
use kube::Client;
use tracing::info;

use crate::cf_dns::CfDns;
use crate::ddns::Controller;

mod cf_dns;
mod ddns;
mod service;
mod spec;
mod trace;

pub async fn run() -> Result<()> {
    let _stop_guard = trace::init_tracing()?;

    let client = Client::try_default().await?;

    info!("init k8s client done");

    let cf_dns = CfDns::new().await?;

    info!("init cf dns client done");

    Controller::new(client, cf_dns).run().await
}
