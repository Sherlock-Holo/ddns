use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_channel::mpsc;
use futures_channel::mpsc::UnboundedSender;
use futures_util::{stream, TryFuture, TryFutureExt, TryStream, TryStreamExt};
use kube::runtime::controller::{Context, ReconcilerAction};
use kube::runtime::reflector::{ObjectRef, Store};
use tokio::time::sleep;
use tracing::{error, info, info_span, instrument, warn, Instrument};

use crate::spec::Ddns;

pub async fn schedule<S, Reconcile, ErrPolicy, ReconcileFut, ErrPolicyFut, Ctx>(
    ddns_stream: S,
    store: Store<Ddns>,
    ctx: Context<Ctx>,
    mut reconcile: Reconcile,
    err_policy: ErrPolicy,
) -> Result<(), anyhow::Error>
where
    S: TryStream<Ok = ObjectRef<Ddns>>,
    S::Error: std::error::Error + Send + Sync + 'static,
    Ctx: Send + Sync + 'static,
    Reconcile: FnMut(Ddns, Context<Ctx>) -> ReconcileFut,
    ReconcileFut: TryFuture<Ok = ReconcilerAction> + Send + 'static,
    ReconcileFut::Error: std::error::Error,
    ErrPolicy: Fn(ReconcileFut::Error, Context<Ctx>) -> ErrPolicyFut + Send + Sync + 'static,
    ErrPolicyFut: Future<Output = ReconcilerAction> + Send + 'static,
{
    let err_policy = Arc::new(err_policy);

    let (re_reconcile_sender, re_reconcile_receiver) = mpsc::unbounded();

    let ddns_stream = stream::select(re_reconcile_receiver, ddns_stream.into_stream());

    futures_util::pin_mut!(ddns_stream);

    while let Some(obj_ref) = ddns_stream
        .try_next()
        .inspect_err(|err| {
            error!(%err, "acquire ddns object reference from stream failed, exit now");
        })
        .await?
    {
        handle_event(
            obj_ref,
            &store,
            ctx.clone(),
            &mut reconcile,
            err_policy.clone(),
            re_reconcile_sender.clone(),
        )?;
    }

    error!("acquire from stream finished, it should not happened");

    Err(anyhow::anyhow!(
        "acquire from stream finished, it should not happened"
    ))
}

#[instrument(skip(store, ctx, reconcile, err_policy, re_reconcile_sender))]
fn handle_event<E, Reconcile, ErrPolicy, ReconcileFut, ErrPolicyFut, Ctx>(
    obj_ref: ObjectRef<Ddns>,
    store: &Store<Ddns>,
    ctx: Context<Ctx>,
    reconcile: &mut Reconcile,
    err_policy: Arc<ErrPolicy>,
    re_reconcile_sender: UnboundedSender<Result<ObjectRef<Ddns>, E>>,
) -> Result<(), anyhow::Error>
where
    E: std::error::Error + Send + Sync + 'static,
    Ctx: Send + Sync + 'static,
    Reconcile: FnMut(Ddns, Context<Ctx>) -> ReconcileFut,
    ReconcileFut: TryFuture<Ok = ReconcilerAction> + Send + 'static,
    ReconcileFut::Error: std::error::Error,
    ErrPolicy: Fn(ReconcileFut::Error, Context<Ctx>) -> ErrPolicyFut + Send + Sync + 'static,
    ErrPolicyFut: Future<Output = ReconcilerAction> + Send + 'static,
{
    info!("start handle event");

    let store_get_span = info_span!("get ddns from store by object reference", %obj_ref);

    let ddns = match store_get_span.in_scope(|| store.get(&obj_ref)) {
        None => {
            warn!(%obj_ref, "get ddns from store failed, ddns not found");

            tokio::spawn(
                async move {
                    info!("sleep 2 seconds");

                    sleep(Duration::from_secs(2)).await;

                    let _ = re_reconcile_sender.unbounded_send(Ok(obj_ref));
                }
                .instrument(info_span!("retry to acquire ddns object reference")),
            );

            return Ok(());
        }

        Some(ddns) => ddns,
    };

    info!(?ddns, "get ddns from store done");

    let fut = reconcile(ddns, ctx.clone()).map_err(move |err| err_policy(err, ctx));

    tokio::spawn(
        async move {
            let result = fut.await;
            let action = match result {
                Err(err_fut) => err_fut.await,

                Ok(action) => action,
            };

            if let Some(dur) = action.requeue_after {
                info!(?dur, "sleep before re-reconcile");

                sleep(dur).await;

                let _ = re_reconcile_sender.unbounded_send(Ok(obj_ref));
            }
        }
        .in_current_span(),
    );

    Ok(())
}
