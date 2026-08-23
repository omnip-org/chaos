use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    pricing::{PriceList, PriceListCode, PriceListId, PriceListSchedule},
    store::StoreId,
};
use time::OffsetDateTime;

use crate::{
    ApplicationError, adapters::postgres::PostgresPricingProvisioningRepository,
    contracts::AdminActor,
};

pub struct CreatePriceInput {
    pub product_variant_id: ProductVariantId,
    pub amount_minor: i64,
}

pub struct CreatePriceListInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub currency: String,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub activate: bool,
    pub prices: Vec<CreatePriceInput>,
}

#[derive(Debug)]
pub struct CreatePriceListOutput {
    pub price_list_id: PriceListId,
}

pub struct CreatePriceList {
    repository: Arc<PostgresPricingProvisioningRepository>,
}

impl CreatePriceList {
    pub fn new(repository: Arc<PostgresPricingProvisioningRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        input: CreatePriceListInput,
    ) -> Result<CreatePriceListOutput, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let currency = CurrencyCode::parse(&input.currency)?;
        let mut price_list = PriceList::create(
            input.store_id,
            PriceListCode::parse(input.code)?,
            input.name,
            currency,
            PriceListSchedule::new(input.starts_at, input.ends_at)?,
        )?;
        for price in &input.prices {
            price_list.add_price(price.product_variant_id, price.amount_minor)?;
        }

        let mut transaction = self.repository.begin(input.actor, input.store_id).await?;
        transaction.require_writable_store().await?;
        transaction.require_store_currency(currency).await?;
        let requested_ids = input
            .prices
            .iter()
            .map(|price| price.product_variant_id)
            .collect::<Vec<_>>();
        let store_variant_ids = transaction.store_variant_ids(&requested_ids).await?;
        if store_variant_ids.len() != requested_ids.len() {
            return Err(ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "product_variant_id",
                    reason: "must identify a Variant in the Store".into(),
                }],
            });
        }
        if input.activate {
            let active_ids = transaction.active_variant_ids(&requested_ids).await?;
            price_list.activate(&active_ids)?;
        }
        transaction.insert_price_list(&price_list).await?;
        transaction.commit().await?;
        Ok(CreatePriceListOutput {
            price_list_id: price_list.id(),
        })
    }
}

fn require_pricing_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}
