use futures_util::{future, Stream, TryStreamExt};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::runtime::watcher;
use kube::runtime::watcher::{Error, Event};
use kube::Api;
use tracing::info;

/// The Service Changing event
#[derive(Debug)]
pub enum ServiceEvent {
    /// A service is applied, it may be created or modified
    Applied(Service),

    /// A service is deleted
    Deleted(Service),
}

pub fn watch_service(api: Api<Service>) -> impl Stream<Item = Result<ServiceEvent, Error>> {
    watcher(api, ListParams::default())
        .try_filter_map(|event| {
            // we only care the service changing, such as adding a new load balance service, or
            // remove an exist load balance service
            let service_event = match event {
                Event::Applied(svc) => ServiceEvent::Applied(svc),
                Event::Deleted(svc) => ServiceEvent::Deleted(svc),

                _ => return future::ok(None),
            };

            info!(?service_event, "get service change event");

            future::ok(Some(service_event))
        })
        // we only care about the load balance service
        .try_filter(|service_event| match service_event {
            ServiceEvent::Applied(svc) | ServiceEvent::Deleted(svc) => {
                let is_lb_svc = svc
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.type_.as_ref())
                    .map(|svc_type| svc_type == "LoadBalancer")
                    .unwrap_or(false);

                if is_lb_svc {
                    info!(?svc, "get load balancer service");
                }

                future::ready(is_lb_svc)
            }
        })
        // we only care about the load balance service who has labels, because ddns need it, if a
        // service doesn't have labels, ddns won't work with it
        .try_filter(|service_event| match service_event {
            ServiceEvent::Applied(svc) | ServiceEvent::Deleted(svc) => {
                let contain_labels = svc
                    .metadata
                    .labels
                    .as_ref()
                    .map(|labels| !labels.is_empty())
                    .unwrap_or(false);

                if contain_labels {
                    info!(?svc, "get labels contained load balancer service");
                }

                future::ready(contain_labels)
            }
        })
}
