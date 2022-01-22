use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::{AbortHandle, Abortable};
use kube::runtime::reflector::ObjectRef;
use tokio::sync::Mutex;
use tracing::{info, instrument};

use crate::ddns::error::Error;
use crate::ddns::Reconcile;
use crate::spec::Ddns;

#[derive(Clone)]
pub struct DeDupReconciler<R> {
    running_futures: Arc<Mutex<HashMap<ObjectRef<Ddns>, AbortHandle>>>,
    inner_reconciler: R,
}

impl<R> DeDupReconciler<R> {
    pub fn new(reconciler: R) -> Self {
        Self {
            running_futures: Arc::new(Default::default()),
            inner_reconciler: reconciler,
        }
    }
}

#[async_trait]
impl<R> Reconcile for DeDupReconciler<R>
where
    R: Reconcile + Clone + Send + Sync,
    R::Error: Into<Error>,
{
    type Error = Error;

    #[instrument(err, skip(self))]
    async fn reconcile_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        let obj_ref = ObjectRef::from_obj(&ddns);

        info!(%obj_ref, "create object ref done");

        let mut running_futures = self.running_futures.lock().await;

        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        if let Some(running_future_abort_handle) =
            running_futures.insert(obj_ref.clone(), abort_handle)
        {
            running_future_abort_handle.abort();

            info!(
                ?running_future_abort_handle,
                ?obj_ref,
                "has exist reconcile ddns task, abort it done"
            );
        }

        drop(running_futures);

        match Abortable::new(
            self.inner_reconciler.reconcile_ddns(ddns.clone()),
            abort_registration,
        )
        .await
        {
            Err(_) => {
                info!(
                    ?ddns,
                    "current reconcile task has been abort by a new reconcile ddns task"
                );

                Err(Error::Aborted)
            }

            Ok(result) => {
                info!(?ddns, ?result, "reconcile ddns done");

                self.running_futures.lock().await.remove(&obj_ref);

                info!(%obj_ref, "remove current reconcile ddns task abort handle done");

                result.map_err(|err| err.into())
            }
        }
    }

    #[instrument(err, skip(self))]
    async fn delete_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        let obj_ref = ObjectRef::from_obj(&ddns);

        info!(%obj_ref, "create object ref done");

        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        if let Some(running_future_abort_handle) = self
            .running_futures
            .lock()
            .await
            .insert(obj_ref.clone(), abort_handle)
        {
            running_future_abort_handle.abort();

            info!(
                ?running_future_abort_handle,
                ?obj_ref,
                "has exist delete ddns task, abort it"
            );
        }

        match Abortable::new(
            self.inner_reconciler.delete_ddns(ddns.clone()),
            abort_registration,
        )
        .await
        {
            Err(_) => {
                info!(
                    ?ddns,
                    "current delete ddns task has been abort by a new delete ddns task"
                );

                Err(Error::Aborted)
            }

            Ok(result) => {
                info!(?ddns, ?result, "reconcile ddns done");

                self.running_futures.lock().await.remove(&obj_ref);

                info!(%obj_ref, "remove current delete ddns task abort handle done");

                result.map_err(|err| err.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[tokio::test]
    async fn reconcile_ddns() {
        #[derive(Clone)]
        struct TestReconciler {
            n: Arc<AtomicBool>,
        }

        #[async_trait]
        impl Reconcile for TestReconciler {
            type Error = Error;

            async fn reconcile_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                self.n.store(true, Ordering::Relaxed);

                Ok(())
            }

            async fn delete_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                unimplemented!()
            }
        }

        let test_reconciler = TestReconciler {
            n: Arc::new(AtomicBool::new(false)),
        };

        let de_dup_reconciler = DeDupReconciler::new(test_reconciler);

        let mut ddns = Ddns::default();
        ddns.metadata.namespace.replace(String::from("123"));
        ddns.metadata.name.replace(String::from("123"));

        de_dup_reconciler.reconcile_ddns(ddns).await.unwrap();

        assert!(de_dup_reconciler.inner_reconciler.n.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn delete_ddns() {
        #[derive(Clone)]
        struct TestReconciler {
            n: Arc<AtomicBool>,
        }

        #[async_trait]
        impl Reconcile for TestReconciler {
            type Error = Error;

            async fn reconcile_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                unimplemented!()
            }

            async fn delete_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                self.n.store(true, Ordering::Relaxed);

                Ok(())
            }
        }

        let test_reconciler = TestReconciler {
            n: Arc::new(AtomicBool::new(false)),
        };

        let de_dup_reconciler = DeDupReconciler::new(test_reconciler);

        let mut ddns = Ddns::default();
        ddns.metadata.namespace.replace(String::from("123"));
        ddns.metadata.name.replace(String::from("123"));

        de_dup_reconciler.delete_ddns(ddns).await.unwrap();

        assert!(de_dup_reconciler.inner_reconciler.n.load(Ordering::Relaxed));
    }
}
