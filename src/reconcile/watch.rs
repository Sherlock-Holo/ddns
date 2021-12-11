use futures_util::{stream, Stream, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::runtime::watcher;
use kube::runtime::watcher::{Error, Event};
use kube::{Api, Client};

use crate::spec::Ddns;

pub fn watch_ddns(api: Api<Ddns>) -> impl Stream<Item = Result<Event<Ddns>, Error>> {
    watcher(api, ListParams::default())
}

pub fn watch_service(
    svc_api: Api<Service>,
    ddns_client: Client,
) -> impl Stream<Item = Result<Event<Ddns>, Error>> {
    watcher(svc_api, ListParams::default())
        .try_filter_map(|mut event| async {
            match &mut event {
                Event::Applied(svc) | Event::Deleted(svc) => {
                    if svc
                        .status
                        .as_ref()
                        .and_then(|status| status.load_balancer.as_ref())
                        .is_some()
                        && svc.metadata.labels.is_some()
                    {
                        return Ok(Some(event));
                    }
                }

                Event::Restarted(svcs) => {
                    // try to reduce re-allocate
                    let mut lb_svcs = std::mem::take(svcs);

                    lb_svcs = lb_svcs
                        .into_iter()
                        .filter(|svc| {
                            svc.status
                                .as_ref()
                                .and_then(|status| status.load_balancer.as_ref())
                                .is_some()
                                && svc.metadata.labels.is_some()
                        })
                        .collect::<Vec<_>>(); // compiler will optimize to avoid re-allocate

                    *svcs = lb_svcs;

                    return Ok(Some(event));
                }
            }

            Ok(None)
        })
        .and_then(move |event| {
            let ddns_client = ddns_client.clone();

            async move {
                match &event {
                    Event::Applied(svc) | Event::Deleted(svc) => {
                        let ddns_list = get_svc_related_ddns(svc, ddns_client).await?;

                        let events = ddns_list
                            .into_iter()
                            .map(Event::Applied)
                            .collect::<Vec<_>>();

                        Ok(events)
                    }

                    Event::Restarted(svcs) => {
                        let ddns_list = stream::iter(svcs)
                            .map(Ok::<_, Error>)
                            .and_then(|svc| async {
                                let ddns_list =
                                    get_svc_related_ddns(svc, ddns_client.clone()).await?;

                                Ok(Event::Restarted(ddns_list))
                            })
                            .try_collect::<Vec<_>>()
                            .await?;

                        Ok(ddns_list)
                    }
                }
            }
        })
        .map_ok(|events| stream::iter(events).map(Ok))
        .try_flatten()
}

async fn get_svc_related_ddns(svc: &Service, client: Client) -> Result<Vec<Ddns>, Error> {
    let ns = svc.metadata.namespace.as_ref().ok_or_else(|| {
        Error::InitialListFailed(kube::Error::Service(
            "service doesn't contain namespace field".into(),
        ))
    })?;

    let labels = svc.metadata.labels.as_ref().unwrap();

    let ddns_api = Api::<Ddns>::namespaced(client, ns);

    let ddns_list = ddns_api
        .list(&ListParams::default())
        .await
        .map_err(Error::InitialListFailed)?;
    let ddns_list = ddns_list
        .into_iter()
        .filter(|ddns| {
            for (key, value) in ddns.spec.selector.iter() {
                if labels
                    .get(key)
                    .filter(|label_value| *label_value == value)
                    .is_none()
                {
                    return false;
                }
            }

            true
        })
        .collect::<Vec<_>>();

    Ok(ddns_list)
}
