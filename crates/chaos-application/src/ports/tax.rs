use async_trait::async_trait;
use chaos_domain::{
    pricing::{TaxRule, TaxRuleId, TaxRuleStatus},
    store::StoreId,
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest};

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
        actor: AdminActor,
        store_id: StoreId,
        rule: &TaxRule,
        idempotency: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError>;

    async fn list_tax_rules(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<TaxRuleDetail>, ApplicationError>;

    async fn change_tax_rule_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        rule_id: TaxRuleId,
        status: TaxRuleStatus,
        idempotency: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError>;
}
