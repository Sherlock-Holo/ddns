use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use futures_util::{stream, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Service;
use kube::api::{ListParams, Patch, PatchParams};
use kube::runtime::controller::{Context, ReconcilerAction};
use kube::Api;
use tracing::{error, info, instrument, warn};

use crate::cf_dns::RecordKind;
use crate::reconcile::{ContextData, Error, PatchFinalizers, FINALIZER};
use crate::spec::Ddns;

#[instrument(err, skip(ddns, ctx))]
pub async fn handle_apply(
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

    info!(%name, ?status, ?spec, "handle apply");

    let mut status = status.unwrap_or_default();

    if status.domain != spec.domain {
        cf_dns
            .remove_dns_records(&spec.domain, &spec.zone, RecordKind::A)
            .await?;

        info!(%name, ?spec, ?status, "remove old dns records");
    }

    let ddns_api: Api<Ddns> = Api::namespaced(client.clone(), &namespace);
    let service_api: Api<Service> = Api::namespaced(client.clone(), &namespace);

    let lb_ips = get_service_lb_ips(&service_api, &spec.selector).await?;

    if lb_ips.is_empty() {
        warn!(%name, ?spec, ?status, "load balancer has no ip");

        return Ok(ReconcilerAction {
            requeue_after: Some(Duration::from_secs(3)),
        });
    }

    info!(
        %name,
        ?spec,
        ?status,
        load_balancer_ip_list=?lb_ips,
        "get service load balancer ip list success"
    );

    cf_dns
        .set_dns_record(&spec.domain, &spec.zone, RecordKind::A, &lb_ips)
        .await?;

    info!(
        %name,
        ?spec,
        ?status,
        load_balancer_ip_list=?lb_ips,
        "set dns record success"
    );

    let finalizer_patch = match metadata.finalizers {
        None => Some(PatchFinalizers::from(FINALIZER.to_string())),
        Some(mut finalizers) if !finalizers.iter().any(|finalizer| finalizer == FINALIZER) => {
            finalizers.push(FINALIZER.to_string());

            Some(PatchFinalizers::from(finalizers))
        }

        _ => None,
    };

    if let Some(finalizer_patch) = finalizer_patch {
        ddns_api
            .patch(
                &name,
                &PatchParams::default(),
                &Patch::Merge(finalizer_patch),
            )
            .await?;

        info!(
            %name,
            ?spec,
            ?status,
            load_balancer_ip_list=?lb_ips,
            "set finalizer success"
        );
    }

    status.status = "RUNNING".to_string();
    status.selector = spec.selector;
    status.domain = spec.domain;
    status.zone = spec.zone;

    ddns_api
        .patch_status(
            &name,
            &PatchParams::default(),
            &Patch::Merge(status.to_patch_status()),
        )
        .await?;

    info!(
        %name,
        ?status,
        load_balancer_ip_list=?lb_ips,
        "update status success"
    );

    Ok(ReconcilerAction {
        requeue_after: None,
    })
}

#[instrument(err, skip(service_api))]
async fn get_service_lb_ips(
    service_api: &Api<Service>,
    selector: &HashMap<String, String>,
) -> Result<Vec<IpAddr>, Error> {
    stream::iter(selector.iter())
        .then(|(key, value)| async move {
            let list_params = ListParams::default().labels(&format!("{}={}", key, value));

            let svc_list = service_api.list(&list_params).await.map_err(|err| {
                error!(selector_key=%key, selector_value=%value, "list service failed");

                err
            })?;

            Ok::<_, Error>(svc_list.items)
        })
        .try_fold(vec![], |mut svc_ips, svc_list| async move {
            let mut svc_status_list = svc_list
                .into_iter()
                .filter_map(|svc| {
                    svc.status
                        .and_then(|status| status.load_balancer)
                        .and_then(|lb| lb.ingress)
                })
                .flatten()
                .flat_map(|ingress| ingress.ip)
                .map(|ip| {
                    ip.parse().map_err(|err| {
                        error!(addr_parse_err=%err, "parse load balancer ingress IP failed");

                        anyhow::Error::from(err).into()
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;

            svc_ips.append(&mut svc_status_list);

            Ok(svc_ips)
        })
        .await
}
