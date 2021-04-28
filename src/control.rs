use anyhow::Result;
use futures_util::StreamExt;
use kube::api::ListParams;
use kube::{Api, Client};
use kube_runtime::controller::Context;
use kube_runtime::Controller;
use tracing::{error, info, instrument};

use crate::cf_dns::CfDns;
use crate::reconcile;
use crate::reconcile::ContextData;
use crate::spec::*;

#[instrument]
pub async fn run_controller() -> Result<()> {
    let client = Client::try_default().await?;

    info!("init k8s client");

    let cf_dns = CfDns::new().await?;

    info!("init cf dns client");

    let ctx = Context::new(ContextData {
        client: client.clone(),
        cf_dns,
    });

    let ddns_api: Api<Ddns> = Api::all(client);

    Controller::new(ddns_api, ListParams::default())
        .run(reconcile::reconcile, reconcile::reconcile_failed, ctx)
        .for_each(|res| async move {
            if let Err(err) = res {
                error!(%err, "reconcile failed");
            }
        })
        .await;

    error!("controller stop");

    Err(anyhow::anyhow!("controller stop"))
}
