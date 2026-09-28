use std::sync::Arc;

use chaos_domain::{
    CurrencyCode,
    pricing::{PriceList, PriceListCode, PriceListId, PriceListSchedule, PriceListStatus},
    store::StoreId,
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresPricingManagementRepository,
    contracts::{AdminActor, PriceListDetail, PriceListSelection},
};

use super::CreatePriceInput;

pub struct UpdatePriceListInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub price_list_id: PriceListId,
    pub code: String,
    pub name: String,
    pub currency: String,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub prices: Vec<CreatePriceInput>,
}

pub struct UpdatePriceListOutput {
    pub price_list_id: PriceListId,
    pub replacement_price_count: usize,
    pub is_currently_effective: bool,
}

pub struct UpsertPriceListPricesInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub price_list_id: PriceListId,
    pub prices: Vec<CreatePriceInput>,
}

pub struct UpsertPriceListPricesOutput {
    pub price_list_id: PriceListId,
    pub processed_count: usize,
    pub changed_count: usize,
    pub is_currently_effective: bool,
}

pub struct ChangePriceListStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub price_list_id: PriceListId,
}

pub struct PriceListPage {
    pub items: Vec<crate::contracts::PriceListReadItem>,
    pub has_more: bool,
    pub selection: PriceListSelection,
}

pub struct PricingManagement {
    repository: Arc<PostgresPricingManagementRepository>,
}

impl PricingManagement {
    pub fn new(repository: Arc<PostgresPricingManagementRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PriceListId>,
        limit: u16,
    ) -> Result<PriceListPage, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut snapshot = self
            .repository
            .list_price_lists(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = snapshot.items.len() > usize::from(limit);
        if has_more {
            snapshot.items.pop();
        }
        Ok(PriceListPage {
            items: snapshot.items,
            has_more,
            selection: snapshot.selection,
        })
    }

    pub async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<PriceListDetail, ApplicationError> {
        self.repository
            .get_price_list(actor, store_id, price_list_id)
            .await?
            .ok_or_else(|| price_list_not_found(price_list_id))
    }

    pub async fn update(
        &self,
        input: UpdatePriceListInput,
    ) -> Result<UpdatePriceListOutput, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut replacement = PriceList::create(
            input.store_id,
            PriceListCode::parse(input.code)?,
            input.name,
            CurrencyCode::parse(&input.currency)?,
            PriceListSchedule::new(input.starts_at, input.ends_at)?,
        )?;
        for price in input.prices {
            replacement.add_price(price.product_variant_id, price.amount_minor)?;
        }
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        let snapshot = transaction
            .load_for_update()
            .await?
            .ok_or_else(|| price_list_not_found(input.price_list_id))?;
        if !transaction
            .currency_matches_store(replacement.currency())
            .await?
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "currency",
                    reason: "must match the Store currency".into(),
                }],
            });
        }
        let candidate_checkpoints = [replacement.starts_at(), replacement.ends_at()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let coverage_before = if snapshot.status == PriceListStatus::Active {
            Some(
                transaction
                    .price_coverage_snapshot(&candidate_checkpoints)
                    .await?,
            )
        } else {
            None
        };
        let priced_ids = replacement
            .prices()
            .iter()
            .map(|price| price.product_variant_id())
            .collect::<Vec<_>>();
        if transaction.store_variant_ids(&priced_ids).await?.len() != priced_ids.len() {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "product_variant_id",
                    reason: "must identify a Variant in the Store".into(),
                }],
            });
        }
        match snapshot.status {
            PriceListStatus::Draft => {}
            PriceListStatus::Active => {
                let active_ids = transaction.active_variant_ids(&priced_ids).await?;
                replacement.activate(&active_ids)?;
            }
            PriceListStatus::Archived => replacement.archive(),
        }
        let replacement_price_count = replacement.prices().len();
        transaction.replace(&replacement).await?;
        if let Some(coverage_before) = coverage_before {
            let coverage_after = transaction
                .price_coverage_issues_at(&coverage_before.checkpoints)
                .await?;
            transaction.reject_new_price_coverage_issues(&coverage_before, &coverage_after)?;
        }
        let current_price_list_id = transaction.current_price_list_id().await?;
        transaction.commit().await?;
        Ok(UpdatePriceListOutput {
            price_list_id: input.price_list_id,
            replacement_price_count,
            is_currently_effective: current_price_list_id == Some(input.price_list_id),
        })
    }

    pub async fn upsert_prices(
        &self,
        input: UpsertPriceListPricesInput,
    ) -> Result<UpsertPriceListPricesOutput, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        validate_price_inputs(&input.prices)?;

        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        let snapshot = transaction
            .load_for_update()
            .await?
            .ok_or_else(|| price_list_not_found(input.price_list_id))?;
        let priced_ids = input
            .prices
            .iter()
            .map(|price| price.product_variant_id)
            .collect::<Vec<_>>();
        if transaction.store_variant_ids(&priced_ids).await?.len() != priced_ids.len() {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "product_variant_id",
                    reason: "must identify a Variant in the Store".into(),
                }],
            });
        }
        if snapshot.status == PriceListStatus::Active
            && transaction.active_variant_ids(&priced_ids).await?.len() != priced_ids.len()
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "product_variant_id",
                    reason: "must identify an active Variant when updating an active price list"
                        .into(),
                }],
            });
        }
        let changed_count = transaction.upsert_prices(&input.prices).await?;
        let current_price_list_id = transaction.current_price_list_id().await?;
        transaction.commit().await?;
        Ok(UpsertPriceListPricesOutput {
            price_list_id: input.price_list_id,
            processed_count: input.prices.len(),
            changed_count,
            is_currently_effective: current_price_list_id == Some(input.price_list_id),
        })
    }

    pub async fn activate(
        &self,
        input: ChangePriceListStatusInput,
    ) -> Result<PriceListId, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        let snapshot = transaction
            .load_for_update()
            .await?
            .ok_or_else(|| price_list_not_found(input.price_list_id))?;
        let coverage_before = transaction.price_coverage_snapshot(&[]).await?;
        let active_ids = transaction
            .active_variant_ids(&snapshot.priced_variant_ids)
            .await?;
        PriceList::validate_activation(&snapshot.priced_variant_ids, &active_ids)?;
        transaction.set_status(PriceListStatus::Active).await?;
        let coverage_after = transaction
            .price_coverage_issues_at(&coverage_before.checkpoints)
            .await?;
        transaction.reject_new_price_coverage_issues(&coverage_before, &coverage_after)?;
        transaction.commit().await.map(|()| input.price_list_id)
    }

    pub async fn archive(
        &self,
        input: ChangePriceListStatusInput,
    ) -> Result<PriceListId, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        if transaction.load_for_update().await?.is_none() {
            return Err(price_list_not_found(input.price_list_id));
        }
        let coverage_before = transaction.price_coverage_snapshot(&[]).await?;
        transaction.set_status(PriceListStatus::Archived).await?;
        let coverage_after = transaction
            .price_coverage_issues_at(&coverage_before.checkpoints)
            .await?;
        transaction.reject_new_price_coverage_issues(&coverage_before, &coverage_after)?;
        transaction.commit().await.map(|()| input.price_list_id)
    }
}

fn require_pricing_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn validate_price_inputs(prices: &[CreatePriceInput]) -> Result<(), ApplicationError> {
    let mut variant_ids = std::collections::HashSet::with_capacity(prices.len());
    if prices.is_empty() {
        return Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "prices",
                reason: "must contain at least one price to upsert".into(),
            }],
        });
    }
    for price in prices {
        if price.amount_minor < 0 {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "amount_minor",
                    reason: "must be zero or greater".into(),
                }],
            });
        }
        if !variant_ids.insert(price.product_variant_id) {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "product_variant_id",
                    reason: "must be unique within the upsert request".into(),
                }],
            });
        }
    }
    Ok(())
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn price_list_not_found(price_list_id: PriceListId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "price_list",
        id: price_list_id.as_uuid().to_string(),
    }
}
