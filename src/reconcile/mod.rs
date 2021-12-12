use std::time::Duration;

use kube::runtime::controller::{Context, ReconcilerAction};
use kube::Client;
use serde::Serialize;
use thiserror::Error;
use tracing::{error, instrument, warn};

use crate::cf_dns::CfDns;
use crate::spec::*;

mod apply;
mod delete;
pub mod reflect;
pub mod schedule;
mod watch;

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

#[instrument(err, skip(ctx))]
pub async fn reconcile(ddns: Ddns, ctx: Context<ContextData>) -> Result<ReconcilerAction, Error> {
    if ddns.metadata.deletion_timestamp.is_some() {
        return delete::handle_delete(ddns, ctx).await;
    }

    apply::handle_apply(ddns, ctx).await
}

#[instrument(skip(_ctx))]
pub fn reconcile_failed(err: &Error, _ctx: Context<ContextData>) -> ReconcilerAction {
    error!(%err, "reconcile failed");

    ReconcilerAction {
        requeue_after: Some(Duration::from_secs(3)),
    }
}

#[instrument(skip(_ctx))]
pub async fn async_reconcile_failed(err: Error, _ctx: Context<ContextData>) -> ReconcilerAction {
    error!(%err, "reconcile failed");

    ReconcilerAction {
        requeue_after: Some(Duration::from_secs(3)),
    }
}
