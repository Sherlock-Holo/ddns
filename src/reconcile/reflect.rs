use futures_util::{stream, Stream};
use kube::runtime::reflector;
use kube::runtime::reflector::store::Writer;
use kube::runtime::utils::try_flatten_applied;
use kube::runtime::watcher::Error;
use kube::{Api, Client};

use crate::reconcile::watch::{watch_ddns, watch_service};
use crate::spec::Ddns;

pub fn reflect(
    client: Client,
    writer: Writer<Ddns>,
) -> Result<impl Stream<Item = Result<Ddns, Error>>, Error> {
    let ddns_stream = watch_ddns(Api::all(client.clone()));
    let svc_stream = watch_service(Api::all(client.clone()), client);

    let combine_ddns_event_stream = stream::select(ddns_stream, svc_stream);

    Ok(try_flatten_applied(reflector(
        writer,
        combine_ddns_event_stream,
    )))
}
