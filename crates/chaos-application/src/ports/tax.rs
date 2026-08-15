use async_trait::async_trait;
use chaos_domain::{
    merchant::StoreId,
    pricing::{TaxRule, TaxRuleId, TaxRuleStatus},
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

#[derive(Clone)]
pub struct TaxRuleDetail {
    pub rule: TaxRule,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait TaxRuleRepository: Send + Sync {
    async fn create_tax_rule(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        rule: &TaxRule,
        idempotency: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError>;

    async fn list_tax_rules(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<TaxRuleDetail>, ApplicationError>;

    async fn change_tax_rule_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        rule_id: TaxRuleId,
        status: TaxRuleStatus,
        idempotency: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError>;
}
