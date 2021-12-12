use kube::api::{Patch, PatchParams};
use kube::runtime::controller::{Context, ReconcilerAction};
use kube::Api;
use tracing::{info, instrument};

use crate::cf_dns::RecordKind;
use crate::reconcile::{ContextData, Error, PatchFinalizers, FINALIZER};
use crate::spec::{Ddns, DdnsStatus};

#[instrument(err, skip(ctx))]
pub async fn handle_delete(
    ddns: Ddns,
    ctx: Context<ContextData>,
) -> Result<ReconcilerAction, Error> {
    let ContextData { client, cf_dns } = ctx.get_ref();

    let metadata = ddns.metadata;
    let name = metadata
        .name
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have name"))?;
    let namespace = metadata
        .namespace
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have namespace field"))?;

    let status = ddns.status;
    let spec = ddns.spec;
    let finalizers = metadata.finalizers;

    info!(%name, ?status, ?spec, ?finalizers, "handle delete");

    let patch_params = PatchParams::default();

    let mut status = if let Some(mut status) = status {
        status.status = "DELETING".to_string();

        status
    } else {
        DdnsStatus {
            status: "DELETING".to_string(),
            selector: spec.selector,
            domain: spec.domain,
            zone: spec.zone,
        }
    };

    let ddns_api: Api<Ddns> = Api::namespaced(client.clone(), &namespace);

    match ddns_api
        .patch_status(
            &name,
            &patch_params,
            &Patch::Merge(status.to_patch_status()),
        )
        .await
    {
        Err(kube::Error::Api(err)) if err.code == 404 => {
            info!(%name, ?status, ?finalizers, "resource has been deleted");

            return Ok(ReconcilerAction {
                requeue_after: None,
            });
        }

        Err(err) => return Err(err.into()),

        Ok(_) => {}
    }

    info!(%name, ?status, ?finalizers, "updated status to DELETING");

    cf_dns
        .remove_dns_records(&status.domain, &status.zone, RecordKind::A)
        .await?;

    info!(%name, ?status, ?finalizers, "remove dns records success");

    status.status = "DELETED".to_string();

    ddns_api
        .patch_status(
            &name,
            &patch_params,
            &Patch::Merge(status.to_patch_status()),
        )
        .await?;

    info!(%name, ?status, "updated status to DELETED");

    if let Some(mut finalizers) = finalizers {
        if let Some(index) = finalizers
            .iter()
            .position(|finalizer| finalizer == FINALIZER)
        {
            finalizers.remove(index);

            ddns_api
                .patch(
                    &name,
                    &patch_params,
                    &Patch::Merge(PatchFinalizers::from(finalizers)),
                )
                .await?;

            info!(%name, ?status, "remove finalizer success");
        }
    }

    info!(%name, ?status, "delete Ddns success");

    Ok(ReconcilerAction {
        requeue_after: None,
    })
}
