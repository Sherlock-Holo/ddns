use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use futures_channel::mpsc::Sender;
use futures_channel::oneshot::Sender as OneshotSender;
use futures_channel::{mpsc, oneshot};
use futures_util::{SinkExt, StreamExt};
use kube::runtime::reflector::ObjectRef;
use tap::TapFallible;
use tokio::sync::Mutex;
use tracing::{error, info, instrument};

use crate::ddns::error::Error;
use crate::ddns::Reconcile;
use crate::spec::Ddns;

type OnlineDdnsList<E> =
    Arc<Mutex<HashMap<ObjectRef<Ddns>, Sender<(Ddns, OneshotSender<Result<(), E>>)>>>>;

pub struct QueueReconciler<R, E> {
    online_ddns_list: OnlineDdnsList<E>,
    inner_reconciler: R,
}

impl<R: Clone, E> Clone for QueueReconciler<R, E> {
    fn clone(&self) -> Self {
        Self {
            online_ddns_list: self.online_ddns_list.clone(),
            inner_reconciler: self.inner_reconciler.clone(),
        }
    }
}

impl<R, E> QueueReconciler<R, E>
where
    R: Reconcile<Error = E>,
{
    pub fn new(reconciler: R) -> Self {
        Self {
            online_ddns_list: Arc::new(Default::default()),
            inner_reconciler: reconciler,
        }
    }
}

#[async_trait]
impl<R> Reconcile for QueueReconciler<R, Error>
where
    R: Reconcile<Error = Error> + Clone + Send + Sync + 'static,
{
    type Error = Error;

    #[instrument(err, skip(self))]
    async fn reconcile_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        let obj_ref = ObjectRef::from_obj(&ddns);

        info!(%obj_ref, "create object ref done");

        let (result_sender, result_receiver) = oneshot::channel();

        let mut ddns_list = self.online_ddns_list.lock().await;

        if let Some(mut sender) = ddns_list.get(&obj_ref).cloned() {
            drop(ddns_list);

            sender
                .send((ddns.clone(), result_sender))
                .await
                .tap_err(|err| {
                    error!(?ddns, %err, "send ddns object reference to handler task failed");
                })?;
        } else {
            // I think the 3 buffer is enough in normal
            let (mut sender, mut receiver) = mpsc::channel(3);

            sender.send((ddns, result_sender)).await.unwrap();

            ddns_list.insert(obj_ref, sender);
            drop(ddns_list);

            let ddns_list = self.online_ddns_list.clone();
            let reconciler = self.inner_reconciler.clone();

            tokio::spawn(async move {
                while let Some((ddns, result_sender)) = receiver.next().await {
                    if ddns.metadata.deletion_timestamp.is_none() {
                        let result = handle_change(&reconciler, ddns).await;

                        let _ = result_sender.send(result);
                    } else {
                        ddns_list.lock().await.remove(&ObjectRef::from_obj(&ddns));

                        let result = handle_delete(reconciler, ddns).await;

                        let _ = result_sender.send(result);

                        return;
                    };
                }
            });
        }

        result_receiver
            .await
            .unwrap_or_else(|_| Err(Error::Other(anyhow!("ddns handle task is stopped"))))
    }

    #[instrument(err, skip(self))]
    async fn delete_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        self.reconcile_ddns(ddns).await
    }
}

#[instrument(err, skip(reconciler))]
async fn handle_change<R: Reconcile>(reconciler: &R, ddns: Ddns) -> Result<(), R::Error> {
    reconciler.reconcile_ddns(ddns).await
}

#[instrument(err, skip(reconciler))]
async fn handle_delete<R: Reconcile>(reconciler: R, ddns: Ddns) -> Result<(), R::Error> {
    reconciler.delete_ddns(ddns).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::{Duration, SystemTime};

    use chrono::DateTime;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use tokio::time;

    use super::*;

    #[tokio::test]
    async fn test_reconcile() {
        #[derive(Clone)]
        struct TestReconciler {
            n: Arc<AtomicU8>,
        }

        #[async_trait]
        impl Reconcile for TestReconciler {
            type Error = Error;

            async fn reconcile_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                self.n.fetch_add(1, Ordering::AcqRel);

                Ok(())
            }

            async fn delete_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                unimplemented!()
            }
        }

        let reconciler = QueueReconciler::new(TestReconciler {
            n: Arc::new(Default::default()),
        });

        let n = reconciler.inner_reconciler.n.clone();
        let online_ddns_list = reconciler.online_ddns_list.clone();

        let mut ddns = Ddns::default();
        ddns.metadata.namespace.replace(String::from("123"));
        ddns.metadata.name.replace(String::from("123"));

        let obj_ref = ObjectRef::from_obj(&ddns);

        let task1 = {
            let reconciler = reconciler.clone();
            let ddns = ddns.clone();

            tokio::spawn(async move { reconciler.reconcile_ddns(ddns).await })
        };

        let task2 = { tokio::spawn(async move { reconciler.reconcile_ddns(ddns).await }) };

        task1.await.unwrap().unwrap();
        task2.await.unwrap().unwrap();

        assert_eq!(n.load(Ordering::Acquire), 2);
        assert!(online_ddns_list.lock().await.get(&obj_ref).is_some());
    }

    #[tokio::test]
    async fn test_delete() {
        #[derive(Clone)]
        struct TestReconciler {
            n: Arc<AtomicU8>,
        }

        #[async_trait]
        impl Reconcile for TestReconciler {
            type Error = Error;

            async fn reconcile_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                self.n.fetch_add(1, Ordering::AcqRel);

                Ok(())
            }

            async fn delete_ddns(&self, _: Ddns) -> Result<(), Self::Error> {
                self.n.fetch_add(1, Ordering::AcqRel);

                Ok(())
            }
        }

        let reconciler = QueueReconciler::new(TestReconciler {
            n: Arc::new(Default::default()),
        });

        let n = reconciler.inner_reconciler.n.clone();
        let online_ddns_list = reconciler.online_ddns_list.clone();

        let mut ddns = Ddns::default();
        ddns.metadata.namespace.replace(String::from("123"));
        ddns.metadata.name.replace(String::from("123"));

        let obj_ref = ObjectRef::from_obj(&ddns);

        let task1 = {
            let reconciler = reconciler.clone();
            let ddns = ddns.clone();

            tokio::spawn(async move { reconciler.reconcile_ddns(ddns).await })
        };

        let task2 = {
            tokio::spawn(async move {
                // make sure the delete happened after reconcile
                time::sleep(Duration::from_millis(100)).await;

                ddns.metadata
                    .deletion_timestamp
                    .replace(Time(DateTime::from(SystemTime::now())));

                reconciler.delete_ddns(ddns).await
            })
        };

        task1.await.unwrap().unwrap();
        task2.await.unwrap().unwrap();

        assert_eq!(n.load(Ordering::Acquire), 2);
        assert!(online_ddns_list.lock().await.get(&obj_ref).is_none());
    }
}
