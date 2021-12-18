use std::ops::Deref;

use async_trait::async_trait;

use crate::spec::Ddns;

#[async_trait]
pub trait Reconcile {
    type Error: std::error::Error + Send;

    async fn reconcile_ddns(&self, ddns: Ddns) -> Result<(), Self::Error>;

    async fn delete_ddns(&self, ddns: Ddns) -> Result<(), Self::Error>;
}

#[async_trait]
impl<R, T> Reconcile for T
where
    T: Deref<Target = R> + Send + Sync,
    R: Reconcile + Sync,
{
    type Error = R::Error;

    async fn reconcile_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        self.deref().reconcile_ddns(ddns).await
    }

    async fn delete_ddns(&self, ddns: Ddns) -> Result<(), Self::Error> {
        self.deref().delete_ddns(ddns).await
    }
}
