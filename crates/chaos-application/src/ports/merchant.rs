use async_trait::async_trait;
use chaos_domain::merchant::{MerchantAccount, MerchantAccountId, MerchantAccountMembership};

use crate::ApplicationError;

pub struct IdempotencyRequest {
    pub key: String,
    pub request_fingerprint: [u8; 32],
}

#[async_trait]
pub trait MerchantProvisioningUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        owner_user_id: chaos_domain::identity::UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Box<dyn MerchantProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait MerchantProvisioningTransaction: Send {
    async fn reserve_account_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<MerchantAccountId>, ApplicationError>;

    async fn insert_merchant_account(
        &mut self,
        account: &MerchantAccount,
    ) -> Result<(), ApplicationError>;

    async fn insert_membership(
        &mut self,
        membership: &MerchantAccountMembership,
    ) -> Result<(), ApplicationError>;

    async fn complete_account_creation(
        &mut self,
        request: &IdempotencyRequest,
        merchant_account_id: MerchantAccountId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
