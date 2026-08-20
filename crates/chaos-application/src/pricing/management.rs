use std::sync::Arc;

use chaos_domain::{
    CurrencyCode,
    merchant::StoreId,
    pricing::{PriceList, PriceListCode, PriceListId, PriceListSchedule, PriceListStatus},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, PriceListDetail, PricingManagementTransaction,
        PricingManagementUnitOfWork, PricingReadRepository,
    },
};

use super::CreatePriceInput;

const UPDATE_PRICE_LIST_OPERATION: &str = "price_lists.update.v1";
const ACTIVATE_PRICE_LIST_OPERATION: &str = "price_lists.activate.v1";
const ARCHIVE_PRICE_LIST_OPERATION: &str = "price_lists.archive.v1";

pub struct UpdatePriceListInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub price_list_id: PriceListId,
    pub code: String,
    pub name: String,
    pub currency: String,
    pub tax_inclusive: bool,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub prices: Vec<CreatePriceInput>,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangePriceListStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub price_list_id: PriceListId,
    pub idempotency: IdempotencyRequest,
}

pub struct PriceListPage {
    pub items: Vec<crate::ports::PriceListReadItem>,
    pub has_more: bool,
}

pub struct PricingManagement {
    reads: Arc<dyn PricingReadRepository>,
    unit_of_work: Arc<dyn PricingManagementUnitOfWork>,
}

impl PricingManagement {
    pub fn new(
        reads: Arc<dyn PricingReadRepository>,
        unit_of_work: Arc<dyn PricingManagementUnitOfWork>,
    ) -> Self {
        Self {
            reads,
            unit_of_work,
        }
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PriceListId>,
        limit: u16,
    ) -> Result<PriceListPage, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .reads
            .list_price_lists(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(PriceListPage { items, has_more })
    }

    pub async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<PriceListDetail, ApplicationError> {
        self.reads
            .get_price_list(actor, store_id, price_list_id)
            .await?
            .ok_or_else(|| price_list_not_found(price_list_id))
    }

    pub async fn update(
        &self,
        input: UpdatePriceListInput,
    ) -> Result<PriceListId, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut replacement = PriceList::create(
            input.store_id,
            PriceListCode::parse(input.code)?,
            input.name,
            CurrencyCode::parse(&input.currency)?,
            input.tax_inclusive,
            PriceListSchedule::new(input.starts_at, input.ends_at)?,
        )?;
        for price in input.prices {
            replacement.add_price(price.product_variant_id, price.amount_minor)?;
        }
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(UPDATE_PRICE_LIST_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
        let snapshot = transaction
            .load_for_update()
            .await?
            .ok_or_else(|| price_list_not_found(input.price_list_id))?;
        if !transaction
            .currency_is_enabled(replacement.currency())
            .await?
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "currency",
                    reason: "must be enabled for the Store".into(),
                }],
            });
        }
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
        transaction.replace(&replacement).await?;
        complete(
            transaction,
            UPDATE_PRICE_LIST_OPERATION,
            &input.idempotency,
            input.price_list_id,
        )
        .await
    }

    pub async fn activate(
        &self,
        input: ChangePriceListStatusInput,
    ) -> Result<PriceListId, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(ACTIVATE_PRICE_LIST_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
        let snapshot = transaction
            .load_for_update()
            .await?
            .ok_or_else(|| price_list_not_found(input.price_list_id))?;
        let active_ids = transaction
            .active_variant_ids(&snapshot.priced_variant_ids)
            .await?;
        PriceList::validate_activation(&snapshot.priced_variant_ids, &active_ids)?;
        transaction.set_status(PriceListStatus::Active).await?;
        complete(
            transaction,
            ACTIVATE_PRICE_LIST_OPERATION,
            &input.idempotency,
            input.price_list_id,
        )
        .await
    }

    pub async fn archive(
        &self,
        input: ChangePriceListStatusInput,
    ) -> Result<PriceListId, ApplicationError> {
        require_pricing_writer(&input.actor)?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.price_list_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(ARCHIVE_PRICE_LIST_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
        if transaction.load_for_update().await?.is_none() {
            return Err(price_list_not_found(input.price_list_id));
        }
        transaction.set_status(PriceListStatus::Archived).await?;
        complete(
            transaction,
            ARCHIVE_PRICE_LIST_OPERATION,
            &input.idempotency,
            input.price_list_id,
        )
        .await
    }
}

async fn complete(
    mut transaction: Box<dyn PricingManagementTransaction>,
    operation: &'static str,
    request: &IdempotencyRequest,
    price_list_id: PriceListId,
) -> Result<PriceListId, ApplicationError> {
    transaction
        .complete_mutation(operation, request, price_list_id)
        .await?;
    transaction.commit().await?;
    Ok(price_list_id)
}

fn require_pricing_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
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
