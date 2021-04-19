use std::net::{AddrParseError, IpAddr};
use std::time::Duration;

use k8s_openapi::api::core::v1::Service;
use kube::api::{ObjectMeta, Patch, PatchParams};
use kube::{Api, Client};
use kube_runtime::controller::{Context, ReconcilerAction};
use serde::Serialize;
use thiserror::Error;
use tracing::{error, info, instrument, warn};

use crate::cf_dns::{CfDns, RecordKind};
use crate::spec::*;

const FINALIZER: &str = "ddns.finalizer.api.sherlockholo.xyz";

#[derive(Error, Debug)]
#[error(transparent)]
pub struct Error(anyhow::Error);

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Self(err)
    }
}

impl From<kube::Error> for Error {
    fn from(err: kube::Error) -> Self {
        Self(err.into())
    }
}

pub struct ContextData {
    pub client: Client,
    pub cf_dns: CfDns,
}

#[derive(Debug, Serialize)]
struct Finalizers {
    finalizers: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PatchFinalizers {
    metadata: Finalizers,
}

impl From<Vec<String>> for PatchFinalizers {
    fn from(finalizers: Vec<String>) -> Self {
        Self {
            metadata: Finalizers { finalizers },
        }
    }
}

impl From<String> for PatchFinalizers {
    fn from(finalizer: String) -> Self {
        Self {
            metadata: Finalizers {
                finalizers: vec![finalizer],
            },
        }
    }
}

#[instrument(skip(ctx))]
pub async fn reconcile(ddns: Ddns, ctx: Context<ContextData>) -> Result<ReconcilerAction, Error> {
    if ddns.metadata.deletion_timestamp.is_some() {
        return handle_delete(ddns, ctx).await;
    }

    handle_apply(ddns, ctx).await
}

#[instrument(skip(_ctx))]
pub fn reconcile_failed(err: &Error, _ctx: Context<ContextData>) -> ReconcilerAction {
    error!(%err, "reconcile failed");

    ReconcilerAction {
        requeue_after: Some(Duration::from_secs(3)),
    }
}

#[instrument(skip(ctx))]
async fn handle_delete(ddns: Ddns, ctx: Context<ContextData>) -> Result<ReconcilerAction, Error> {
    let ContextData {
        client: client_ref,
        cf_dns,
    } = ctx.get_ref();

    let metadata: ObjectMeta = ddns.metadata;
    let name = metadata
        .name
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have name"))?;
    let namespace = metadata
        .namespace
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have namespace field"))?;

    let status: Option<DdnsStatus> = ddns.status;
    let spec: DdnsSpec = ddns.spec;
    let finalizers = metadata.finalizers;

    let patch_params = PatchParams::default();

    let mut status = if let Some(mut status) = status {
        status.status = "DELETING".to_string();

        status
    } else {
        DdnsStatus {
            status: "DELETING".to_string(),
            service_name: spec.service_name,
            domain_name: spec.domain_name,
            domain: spec.domain,
        }
    };

    let ddns_api: Api<Ddns> = Api::namespaced(client_ref.clone(), &namespace);

    ddns_api
        .patch_status(
            &name,
            &patch_params,
            &Patch::Merge(status.to_patch_status()),
        )
        .await?;

    info!(%name, ?status, "update status to DELETING");

    cf_dns
        .remove_dns_records(&status.domain_name, &status.domain, RecordKind::A)
        .await?;

    info!(%name, ?status, "remove dns records success");

    status.status = "DELETED".to_string();

    ddns_api
        .patch_status(
            &name,
            &patch_params,
            &Patch::Merge(status.to_patch_status()),
        )
        .await?;

    info!(%name, ?status, "update status to DELETED");

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

    info!(%name, ?status,"delete Ddns success");

    Ok(ReconcilerAction {
        requeue_after: None,
    })
}

#[instrument(skip(ddns, ctx))]
async fn handle_apply(ddns: Ddns, ctx: Context<ContextData>) -> Result<ReconcilerAction, Error> {
    let ContextData {
        client: client_ref,
        cf_dns,
    } = ctx.get_ref();

    let metadata: ObjectMeta = ddns.metadata;
    let name = metadata
        .name
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have name"))?;
    let namespace = metadata
        .namespace
        .ok_or_else(|| anyhow::anyhow!("ddns resource doesn't have namespace field"))?;

    let status: Option<DdnsStatus> = ddns.status;
    let spec: DdnsSpec = ddns.spec;

    let mut status = status.unwrap_or_default();

    if status.domain_name != spec.domain_name {
        cf_dns
            .remove_dns_records(&status.domain_name, &status.domain, RecordKind::A)
            .await?;

        info!(%name, ?spec, ?status, "remove old dns records");
    }

    let ddns_api: Api<Ddns> = Api::namespaced(client_ref.clone(), &namespace);
    let service_api: Api<Service> = Api::namespaced(client_ref.clone(), &namespace);

    let service = match service_api.get(&spec.service_name).await {
        Err(kube::Error::Api(err)) if err.code == 404 => {
            return Ok(ReconcilerAction {
                requeue_after: Some(Duration::from_secs(3)),
            });
        }

        Err(err) => return Err(err.into()),
        Ok(service) => service,
    };

    let service_status = service.status.ok_or_else(|| {
        error!(%name, ?spec, ?status, "service has no status");

        anyhow::anyhow!("service {} doesn't have status", spec.service_name)
    })?;

    let lb_status = service_status.load_balancer.ok_or_else(|| {
        error!(%name, ?spec, ?status, "service is not load balancer service");

        anyhow::anyhow!("service {} is not load balancer service", spec.service_name)
    })?;

    let lb_ingress_list = if let Some(lb_ingress_list) = lb_status.ingress {
        lb_ingress_list
    } else {
        warn!(%name, ?spec, ?status, "service load balancer has no ingress info");

        return Ok(ReconcilerAction {
            requeue_after: Some(Duration::from_secs(3)),
        });
    };

    let lb_ips = lb_ingress_list
        .into_iter()
        .filter_map(|lb_ingress| {
            lb_ingress.ip.map(|ip| {
                ip.parse()
                    .map_err(|err: AddrParseError| {
                        warn!(
                            %name,
                            ?spec,
                            ?status,
                            %ip,
                            ?err,
                            "parse ip from str failed"
                        );

                        err
                    })
                    .ok()
            })?
        })
        .collect::<Vec<IpAddr>>();

    if lb_ips.is_empty() {
        warn!(%name, ?spec, ?status, "load balancer has no ip");

        return Ok(ReconcilerAction {
            requeue_after: Some(Duration::from_secs(3)),
        });
    }

    cf_dns
        .set_dns_record(&spec.service_name, &spec.domain, RecordKind::A, &lb_ips)
        .await?;

    info!(%name, ?spec, ?status, "set dns record success");

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

        info!(%name, ?spec, ?status, "set finalizer success");
    }

    status.status = "RUNNING".to_string();
    status.service_name = spec.service_name;
    status.domain_name = spec.domain_name;

    ddns_api
        .patch_status(
            &name,
            &PatchParams::default(),
            &Patch::Merge(status.to_patch_status()),
        )
        .await?;

    info!(%name, ?status, "update status success");

    Ok(ReconcilerAction {
        requeue_after: Some(Duration::from_secs(30)),
    })
}
