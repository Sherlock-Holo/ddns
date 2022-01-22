use std::time::Duration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("re reconcile {0:?}")]
    ReRun(Duration),

    #[error("reconcile failed: {0}")]
    Other(#[from] anyhow::Error),

    #[error("reconcile or delete task has been aborted")]
    Aborted,
}

impl From<kube::Error> for Error {
    fn from(err: kube::Error) -> Self {
        Self::Other(err.into())
    }
}

impl From<Duration> for Error {
    fn from(dur: Duration) -> Self {
        Self::ReRun(dur)
    }
}
