use futures_util::Stream;
use kube::api::ListParams;
use kube::runtime::utils::try_flatten_applied;
use kube::runtime::watcher;
use kube::runtime::watcher::Error;
use kube::Api;

use crate::spec::Ddns;

pub fn watch_ddns(api: Api<Ddns>) -> impl Stream<Item = Result<Ddns, Error>> {
    try_flatten_applied(watcher(api, ListParams::default()))
}
