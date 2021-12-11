use anyhow::Result;
use futures_util::{StreamExt, TryStreamExt};
use kube::api::ListParams;
use kube::runtime::controller::Context;
use kube::runtime::reflector::store::Writer;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::Controller;
use kube::{Api, Client};
use tracing::{error, info};

use crate::cf_dns::CfDns;
use crate::reconcile;
use crate::reconcile::reflect::reflect;
use crate::reconcile::schedule::schedule;
use crate::reconcile::ContextData;
use crate::spec::*;

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

pub async fn run_scheduler() -> Result<()> {
    let client = Client::try_default().await?;

    info!("init k8s client");

    let cf_dns = CfDns::new().await?;

    info!("init cf dns client");

    let ctx = Context::new(ContextData {
        client: client.clone(),
        cf_dns,
    });

    let writer = Writer::default();
    let store = writer.as_reader();

    let ddns_stream = reflect(client, writer)?.map_ok(|ddns| ObjectRef::from_obj(&ddns));

    schedule(
        ddns_stream,
        store,
        ctx,
        reconcile::reconcile,
        reconcile::async_reconcile_failed,
    )
    .await?;

    unreachable!()
}
