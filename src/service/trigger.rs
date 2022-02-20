use futures_util::TryStreamExt;
use itertools::Itertools;
use kube::api::ListParams;
use kube::{Api, Client, Error};
use tap::TapFallible;
use tracing::{error, info, info_span, Instrument};

use crate::ddns::{ErrorPolicy, Reconcile};
use crate::service::watch::{watch_service, ServiceEvent};
use crate::spec::Ddns;

pub struct Trigger<R, E> {
    client: Client,
    reconciler: R,
    err_policy: E,
}

impl<R, E> Trigger<R, E> {
    pub fn new(client: Client, reconciler: R, err_policy: E) -> Self {
        Self {
            client,
            reconciler,
            err_policy,
        }
    }
}

impl<R, E> Trigger<R, E>
where
    R: Reconcile + Clone + Send + Sync + 'static,
    E: ErrorPolicy<Error = R::Error> + Clone + Send + Sync + 'static,
{
    #[allow(unstable_name_collisions)] // remove it when we can use intersperse only with std lib
    pub async fn trigger_ddns_reconcile(self) -> Result<(), anyhow::Error> {
        info!("start trigger ddns reconcile");

        let svc_api = Api::all(self.client.clone());

        let service_change_stream = watch_service(svc_api);
        futures_util::pin_mut!(service_change_stream);

        while let Some(service_event) = service_change_stream.try_next().await.tap_err(|err| {
            error!(%err, "get service change stream failed");
        })? {
            info!(?service_event, "get service change event");

            let reconciler = self.reconciler.clone();
            let err_policy = self.err_policy.clone();

            let ddns_api = Api::<Ddns>::all(self.client.clone());

            tokio::spawn(
                async move {
                    let svc = match service_event {
                        ServiceEvent::Applied(svc) | ServiceEvent::Deleted(svc) => svc,
                    };

                    let labels = svc
                        .metadata
                        .labels
                        .expect("service_change_stream return empty labels service");

                    info!(?labels, "get labels map");

                    let labels = labels
                        .into_iter()
                        .map(|(key, value)| format!("{}={}", key, value))
                        .intersperse(",".to_string())
                        .collect::<String>();

                    info!(%labels, "get filter labels");

                    let list_params = ListParams::default().labels(&labels);

                    let ddns_list = ddns_api.list(&list_params).await.tap_err(|err| {
                        error!(%err, "list ddns failed");
                    })?;

                    for ddns in ddns_list {
                        let reconciler = reconciler.clone();
                        let err_policy = err_policy.clone();

                        tokio::spawn(
                            async move {
                                info!(?ddns, "start reconcile ddns");

                                if let Err(err) = reconciler.reconcile_ddns(ddns.clone()).await {
                                    error!(%err, ?ddns, "reconcile failed");

                                    err_policy.error_policy(ddns.clone(), err).await;

                                    info!(?ddns, "run error policy done");
                                }
                            }
                            .instrument(info_span!("reconcile ddns")),
                        );
                    }

                    Ok::<_, Error>(())
                }
                .instrument(info_span!("trigger ddns reconcile")),
            );
        }

        error!("service change stream is dry, that should not happened");

        Err(anyhow::anyhow!(
            "service change stream is dry, that should not happened"
        ))
    }
}
