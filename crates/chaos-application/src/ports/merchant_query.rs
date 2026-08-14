use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    merchant::{
        MerchantAccountId, MerchantAccountSlug, MerchantAccountStatus, MerchantRole, StoreCode,
        StoreId, StoreStatus,
    },
};

use crate::ApplicationError;

pub struct MerchantAccountListItem {
    pub id: MerchantAccountId,
    pub slug: MerchantAccountSlug,
    pub display_name: String,
    pub status: MerchantAccountStatus,
    pub role: MerchantRole,
}

pub struct StoreListItem {
    pub id: StoreId,
    pub code: StoreCode,
    pub name: String,
    pub default_region: RegionCode,
    pub default_currency: CurrencyCode,
    pub status: StoreStatus,
}

#[async_trait]
pub trait MerchantReadRepository: Send + Sync {
    async fn list_merchant_accounts(
        &self,
        user_id: UserId,
        after: Option<MerchantAccountId>,
        limit: u16,
    ) -> Result<Vec<MerchantAccountListItem>, ApplicationError>;

    async fn membership_role(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Option<MerchantRole>, ApplicationError>;

    async fn list_stores(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Vec<StoreListItem>, ApplicationError>;
}
