use std::ops::Deref;

use async_trait::async_trait;

use crate::spec::Ddns;

#[async_trait]
pub trait ErrorPolicy {
    type Error: std::error::Error + Send;

    async fn error_policy(&self, ddns: Ddns, err: Self::Error);
}

#[async_trait]
impl<E, T> ErrorPolicy for T
where
    T: Deref<Target = E> + Send + Sync,
    E: ErrorPolicy + Sync,
    E::Error: 'static,
{
    type Error = E::Error;

    async fn error_policy(&self, ddns: Ddns, err: Self::Error) {
        self.deref().error_policy(ddns, err).await
    }
}
