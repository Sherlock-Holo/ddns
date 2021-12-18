use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{stream, StreamExt, TryFutureExt, TryStreamExt};
use k8s_openapi::api::core::v1::Service;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, Client};
use serde::Serialize;
use tracing::{error, info, instrument, warn};

use crate::cf_dns::{CfDns, RecordKind};
use crate::ddns::{Error, Reconcile};
use crate::spec::{Ddns, DdnsStatus};

const FINALIZER: &str = "ddns.finalizer.api.sherlockholo.xyz";

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

#[derive(Clone)]
pub struct DefaultReconciler {
    client: Client,
    cf_dns: CfDns,
}

impl DefaultReconciler {
    pub fn new(client: Client, cf_dns: CfDns) -> Self {
        Self { client, cf_dns }
    }
}

#[async_trait]
impl Reconcile for DefaultReconciler {
    type Error = Error;

    #[instrument(err, skip(self))]
    async fn reconcile_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        let metadata = ddns.metadata;
        let name = metadata.name.ok_or_else(|| {
            error!("ddns resource doesn't have name");

            anyhow::anyhow!("ddns resource doesn't have name")
        })?;
        let namespace = metadata.namespace.ok_or_else(|| {
            error!("ddns resource doesn't have namespace field");

            anyhow::anyhow!("ddns resource doesn't have namespace field")
        })?;

        let status = ddns.status;
        let spec = ddns.spec;

        info!(%name, ?status, ?spec, "get dns name, status and spec");

        let mut status = status.unwrap_or_default();

        if status.domain != spec.domain {
            info!(?status, ?spec, "status domain != spec domain");

            self.cf_dns
                .remove_dns_records(&spec.domain, &spec.zone, RecordKind::A)
                .await?;

            info!(%name, ?spec, ?status, "remove old dns records done");
        }

        let ddns_api: Api<Ddns> = Api::namespaced(self.client.clone(), &namespace);
        let service_api: Api<Service> = Api::namespaced(self.client.clone(), &namespace);

        let lb_ips = get_service_lb_ips(&service_api, &spec.selector).await?;

        if lb_ips.is_empty() {
            warn!(%name, ?spec, ?status, "load balancer has no ip");

            return Err(Duration::from_secs(3).into());
        }

        info!(
            %name,
            ?spec,
            ?status,
            load_balancer_ip_list=?lb_ips,
            "get service load balancer ip list success"
        );

        self.cf_dns
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
                "set finalizer done"
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
            "update status done"
        );

        Ok(())
    }

    #[instrument(err, skip(self))]
    async fn delete_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        let metadata = ddns.metadata;
        let name = metadata.name.ok_or_else(|| {
            error!("ddns resource doesn't have name");

            anyhow::anyhow!("ddns resource doesn't have name")
        })?;
        let namespace = metadata.namespace.ok_or_else(|| {
            error!("ddns resource doesn't have namespace field");

            anyhow::anyhow!("ddns resource doesn't have namespace field")
        })?;

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

        let ddns_api: Api<Ddns> = Api::namespaced(self.client.clone(), &namespace);

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

                return Ok(());
            }

            Err(err) => {
                error!(%err, "patch ddns status failed");

                return Err(err.into());
            }

            Ok(_) => {}
        }

        info!(%name, ?status, ?finalizers, "update status to DELETING done");

        self.cf_dns
            .remove_dns_records(&status.domain, &status.zone, RecordKind::A)
            .await?;

        info!(%name, ?status, ?finalizers, "remove dns records success");

        status.status = "DELETED".to_string();

        match ddns_api
            .patch_status(
                &name,
                &patch_params,
                &Patch::Merge(status.to_patch_status()),
            )
            .await
        {
            Err(kube::Error::Api(err)) if err.code == 404 => {
                info!(%name, "ddns has been deleted");

                return Ok(());
            }

            Err(err) => {
                error!(%name, ?status, %err, "patch status failed");

                return Err(err.into());
            }

            Ok(_) => {}
        }

        info!(%name, ?status, "update status to DELETED done");

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
                    .inspect_err(|err| error!(%err, "remove finalizer failed"))
                    .await?;

                info!(%name, ?status, "remove finalizer success");
            }
        }

        info!(%name, ?status, "delete Ddns success");

        Ok(())
    }
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
