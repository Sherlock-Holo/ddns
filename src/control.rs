use anyhow::Result;
use futures_util::StreamExt;
use kube::api::ListParams;
use kube::{Api, Client};
use kube_runtime::controller::Context;
use kube_runtime::Controller;
use tracing::{error, info};

use crate::cf_dns::CfDns;
use crate::reconcile;
use crate::reconcile::ContextData;
use crate::spec::*;

pub async fn run_controller() -> Result<()> {
    let client = Client::try_default().await?;
    let cf_dns = CfDns::new().await?;

    let ctx = Context::new(ContextData {
        client: client.clone(),
        cf_dns,
    });

    let ddns_api: Api<Ddns> = Api::all(client);

    Controller::new(ddns_api, ListParams::default())
        .run(reconcile::reconcile, reconcile::reconcile_failed, ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => info!("reconciled {:?}", o),
                Err(e) => error!("reconcile failed: {}", e),
            }
        })
        .await;

    Ok(())
}
