use anyhow::Result;
use futures_util::TryStreamExt;
use kube::runtime::controller::Context;
use kube::runtime::reflector::store::Writer;
use kube::runtime::reflector::ObjectRef;
use kube::Client;
use tracing::info;

use crate::cf_dns::CfDns;
use crate::reconcile;
use crate::reconcile::reflect::reflect;
use crate::reconcile::schedule::schedule;
use crate::reconcile::ContextData;

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
