use std::sync::Arc;

use chaos_domain::{
    merchant::{ApiKeyScope, StoreId},
    pricing::{TaxRule, TaxRuleId, TaxRuleStatus},
};

use crate::{
    ApplicationError,
    ports::{AdminActor, IdempotencyRequest, TaxRuleDetail, TaxRuleRepository},
};

pub struct CreateTaxRuleInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub country_code: String,
    pub rate_basis_points: u32,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeTaxRuleStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub rule_id: TaxRuleId,
    pub status: TaxRuleStatus,
    pub idempotency: IdempotencyRequest,
}

pub struct TaxManagement {
    repository: Arc<dyn TaxRuleRepository>,
}

impl TaxManagement {
    pub fn new(repository: Arc<dyn TaxRuleRepository>) -> Self {
        Self { repository }
    }

    pub async fn create(
        &self,
        input: CreateTaxRuleInput,
    ) -> Result<TaxRuleDetail, ApplicationError> {
        require_operator(&input.actor)?;
        let rule = TaxRule::create(
            input.code,
            input.name,
            input.country_code,
            input.rate_basis_points,
        )?;
        self.repository
            .create_tax_rule(input.actor, input.store_id, &rule, &input.idempotency)
            .await
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<TaxRuleDetail>, ApplicationError> {
        require_operator(&actor)?;
        self.repository.list_tax_rules(actor, store_id).await
    }

    pub async fn change_status(
        &self,
        input: ChangeTaxRuleStatusInput,
    ) -> Result<TaxRuleDetail, ApplicationError> {
        require_operator(&input.actor)?;
        self.repository
            .change_tax_rule_status(
                input.actor,
                input.store_id,
                input.rule_id,
                input.status,
                &input.idempotency,
            )
            .await
    }
}

fn require_operator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(machine) => {
            if machine.scopes.contains(&ApiKeyScope::PricingWrite) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}
