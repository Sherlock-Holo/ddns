use anyhow::Error;
use futures_channel::mpsc;
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::{stream, StreamExt, TryFutureExt, TryStreamExt};
use kube::{Api, Client};
use tracing::{error, info, info_span, Instrument};

use crate::cf_dns::CfDns;
use crate::ddns::default_err_policy::DefaultErrPolicy;
use crate::ddns::default_reconciler::DefaultReconciler;
use crate::ddns::watch::watch_ddns;
use crate::ddns::{ErrorPolicy, Reconcile};
use crate::service::Trigger;
use crate::spec::Ddns;

pub struct Controller {
    client: Client,
    reconciler: DefaultReconciler,
    err_policy: DefaultErrPolicy<UnboundedSender<Ddns>>,
    trigger: Trigger<DefaultReconciler, DefaultErrPolicy<UnboundedSender<Ddns>>>,
    retry_queue_receiver: UnboundedReceiver<Ddns>,
}

impl Controller {
    pub fn new(client: Client, cf_dns: CfDns) -> Self {
        let (queue_sender, queue_receiver) = mpsc::unbounded();

        let default_reconciler = DefaultReconciler::new(client.clone(), cf_dns);
        let default_err_policy = DefaultErrPolicy::new(queue_sender);

        let trigger = Trigger::new(
            client.clone(),
            default_reconciler.clone(),
            default_err_policy.clone(),
        );

        Self {
            client,
            reconciler: default_reconciler,
            err_policy: default_err_policy,
            trigger,
            retry_queue_receiver: queue_receiver,
        }
    }

    pub async fn run(self) -> Result<(), Error> {
        let trigger = self.trigger;

        tokio::spawn(trigger.trigger_ddns_reconcile());

        info!("trigger start to trigger ddns reconcile");

        let ddns_stream = watch_ddns(Api::all(self.client));
        let retry_queue_receiver = self.retry_queue_receiver.map(Ok);

        let ddns_stream = stream::select(ddns_stream, retry_queue_receiver);
        futures_util::pin_mut!(ddns_stream);

        info!("start to acquire ddns from ddns stream");

        while let Some(ddns) = ddns_stream
            .try_next()
            .inspect_err(|err| error!(%err, "get ddns from stream failed"))
            .await?
        {
            info!(?ddns, "acquire ddns done");

            let reconciler = self.reconciler.clone();
            let err_policy = self.err_policy.clone();

            tokio::spawn(
                async move {
                    if ddns.metadata.deletion_timestamp.is_some() {
                        info!(?ddns.metadata, "deletion_timestamp is set, delete dns");

                        if let Err(err) = reconciler.delete_ddns(ddns.clone()).await {
                            error!(%err, "delete ddns failed");

                            err_policy.error_policy(ddns, err).await;
                        }
                    } else if let Err(err) = reconciler.reconcile_ddns(ddns.clone()).await {
                        error!(%err, "reconcile ddns failed");

                        err_policy.error_policy(ddns, err).await;
                    }
                }
                .instrument(info_span!("reconcile ddns")),
            );
        }

        error!("acquire ddns from stream stop, but that should not happened");

        Err(anyhow::anyhow!(
            "acquire ddns from stream stop, but that should not happened"
        ))
    }
}
