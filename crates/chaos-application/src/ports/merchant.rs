use async_trait::async_trait;
use chaos_domain::merchant::{MerchantAccount, MerchantAccountMembership};

use crate::ApplicationError;

#[async_trait]
pub trait MerchantProvisioningUnitOfWork: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn MerchantProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait MerchantProvisioningTransaction: Send {
    async fn insert_merchant_account(
        &mut self,
        account: &MerchantAccount,
    ) -> Result<(), ApplicationError>;

    async fn insert_membership(
        &mut self,
        membership: &MerchantAccountMembership,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
