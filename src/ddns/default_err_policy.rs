use std::time::Duration;

use async_trait::async_trait;
use futures_util::{Sink, SinkExt};
use tokio::time;
use tracing::{info, instrument};

use crate::ddns::{Error, ErrorPolicy};
use crate::spec::Ddns;

#[derive(Clone)]
pub struct DefaultErrPolicy<S> {
    retry_queue: S,
}

impl<S> DefaultErrPolicy<S> {
    pub fn new(retry_queue: S) -> Self {
        Self { retry_queue }
    }
}

#[async_trait]
impl<S> ErrorPolicy for DefaultErrPolicy<S>
where
    S: Sink<Ddns> + Clone + Send + Sync + 'static,
{
    type Error = Error;

    #[instrument(skip(self))]
    async fn error_policy(&self, ddns: Ddns, err: Self::Error) {
        let retry_queue = self.retry_queue.clone();

        match err {
            Error::ReRun(dur) => {
                tokio::spawn(async move {
                    info!(?ddns, %err, "ddns need to re reconcile");

                    time::sleep(dur).await;

                    futures_util::pin_mut!(retry_queue);

                    let _ = retry_queue.send(ddns).await;
                });
            }

            Error::Other(err) => {
                info!(?ddns, %err, "handle ddns failed, need to reconcile after 3 seconds");

                tokio::spawn(async move {
                    time::sleep(Duration::from_secs(3)).await;

                    futures_util::pin_mut!(retry_queue);

                    let _ = retry_queue.send(ddns).await;
                });
            }

            // aborted task no need do anything
            Error::Aborted => {}
        }
    }
}
