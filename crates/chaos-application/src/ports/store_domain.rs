use async_trait::async_trait;
use chaos_domain::merchant::{
    SalesChannelId, StoreDomainId, StoreDomainStatus, StoreHostname, StoreId,
};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

pub struct StoreDomainVerificationMaterial {
    pub plaintext_token: SecretString,
    pub token_digest: [u8; 32],
}

pub trait StoreDomainVerificationMaterialGenerator: Send + Sync {
    fn generate(&self) -> StoreDomainVerificationMaterial;
}

#[async_trait]
pub trait StoreDomainOwnershipVerifier: Send + Sync {
    async fn verify(
        &self,
        hostname: &StoreHostname,
        expected_token_digest: &[u8; 32],
    ) -> Result<bool, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreDomainItem {
    pub id: StoreDomainId,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub hostname: StoreHostname,
    pub domain_status: StoreDomainStatus,
    pub created_by: chaos_domain::identity::UserId,
    pub verified_at: Option<OffsetDateTime>,
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct PendingStoreDomainVerification {
    pub item: StoreDomainItem,
    pub token_digest: [u8; 32],
    pub verification_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreDomainCreationStatus {
    Created,
    Replayed,
}

pub struct StoreDomainCreationResult {
    pub item: StoreDomainItem,
    pub status: StoreDomainCreationStatus,
}

pub struct CreateStoreDomainRecord {
    pub store_id: StoreId,
    pub domain_id: StoreDomainId,
    pub sales_channel_id: SalesChannelId,
    pub hostname: StoreHostname,
    pub verification_token_digest: [u8; 32],
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStoreDomain {
    pub merchant_account_id: chaos_domain::merchant::MerchantAccountId,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub hostname: StoreHostname,
}

#[async_trait]
pub trait StoreDomainRepository: Send + Sync {
    async fn create(
        &self,
        actor: MerchantActor,
        record: CreateStoreDomainRecord,
        request: &IdempotencyRequest,
    ) -> Result<StoreDomainCreationResult, ApplicationError>;

    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<StoreDomainId>,
        limit: u16,
    ) -> Result<Option<Vec<StoreDomainItem>>, ApplicationError>;

    async fn pending_verification(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
    ) -> Result<PendingStoreDomainVerification, ApplicationError>;

    async fn mark_verified(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreDomainItem, ApplicationError>;

    async fn archive(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreDomainItem, ApplicationError>;

    async fn resolve(
        &self,
        hostname: &StoreHostname,
    ) -> Result<Option<ResolvedStoreDomain>, ApplicationError>;
}
