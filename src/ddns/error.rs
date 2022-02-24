use std::time::Duration;

use futures_channel::mpsc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("re reconcile {0:?}")]
    ReRun(Duration),

    #[error("reconcile failed: {0}")]
    Other(#[from] anyhow::Error),
}

impl From<kube::Error> for Error {
    fn from(err: kube::Error) -> Self {
        Self::Other(err.into())
    }
}

impl From<mpsc::SendError> for Error {
    fn from(err: mpsc::SendError) -> Self {
        Self::Other(err.into())
    }
}

impl From<Duration> for Error {
    fn from(dur: Duration) -> Self {
        Self::ReRun(dur)
    }
}
