use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    CurrencyCode,
    fulfillment::{
        FulfillmentId, FulfillmentStatus, ReturnId, ReturnStatus, ShippingProviderAccount,
        ShippingProviderAccountId, ShippingSecretReference,
    },
    merchant::{MerchantRole, StoreId},
    pricing::Money,
    sales::OrderId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, FulfillmentEventQueue,
        FulfillmentRepository, IdempotencyRequest, ReturnDetail, ReturnLineInput,
        ReturnReceiptInput, ShippingAddress, ShippingProvider,
        ShippingProviderAccountConfiguration, ShippingProviderAccountDetail,
        ShippingProviderAccountRepository, ShippingServiceDetail, ShippingServiceRepository,
    },
};
use chaos_domain::fulfillment::{ShippingService, ShippingServiceId, ShippingServiceStatus};

pub struct CreateFulfillmentInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub allocations: Vec<FulfillmentAllocationInput>,
    pub idempotency: IdempotencyRequest,
}

pub struct TransitionFulfillmentInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub fulfillment_id: FulfillmentId,
    pub target_status: FulfillmentStatus,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateReturnInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub lines: Vec<ReturnLineInput>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct TransitionReturnInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub return_id: ReturnId,
    pub target_status: ReturnStatus,
    pub receipt: Vec<ReturnReceiptInput>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct FulfillmentManagement {
    repository: Arc<dyn FulfillmentRepository>,
}

pub struct FulfillmentWorkers {
    queue: Arc<dyn FulfillmentEventQueue>,
}

impl FulfillmentWorkers {
    pub fn new(queue: Arc<dyn FulfillmentEventQueue>) -> Self {
        Self { queue }
    }

    pub async fn run_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_events(worker_id, limit, now, now - Duration::minutes(1))
            .await?;
        for job in &jobs {
            let result = self
                .queue
                .process_event(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_event(worker_id, job.id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }
}

pub struct CreateShippingServiceInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub currency: String,
    pub amount_minor: i64,
    pub estimated_min_days: u16,
    pub estimated_max_days: u16,
    pub destination_countries: Vec<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeShippingServiceStatusInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub service_id: ShippingServiceId,
    pub status: ShippingServiceStatus,
    pub idempotency: IdempotencyRequest,
}

pub struct ShippingManagement {
    repository: Arc<dyn ShippingServiceRepository>,
}

pub struct CreateShippingProviderAccountInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub provider: String,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub origin: ShippingAddress,
    pub enabled: bool,
    pub idempotency: IdempotencyRequest,
}

pub struct UpdateShippingProviderAccountInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub id: ShippingProviderAccountId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub origin: ShippingAddress,
    pub enabled: bool,
    pub idempotency: IdempotencyRequest,
}

pub struct ShippingProviderAdministration {
    repository: Arc<dyn ShippingProviderAccountRepository>,
    supported_providers: HashSet<String>,
}

impl ShippingProviderAdministration {
    pub fn new(
        repository: Arc<dyn ShippingProviderAccountRepository>,
        providers: impl IntoIterator<Item = Arc<dyn ShippingProvider>>,
    ) -> Self {
        Self {
            repository,
            supported_providers: providers
                .into_iter()
                .map(|provider| provider.name().to_owned())
                .collect(),
        }
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError> {
        require_provider_administrator(actor)?;
        self.repository.list(actor, store_id).await
    }

    pub async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: ShippingProviderAccountId,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        require_provider_administrator(actor)?;
        self.repository
            .get(actor, store_id, id)
            .await?
            .ok_or_else(|| shipping_provider_account_not_found(id))
    }

    pub async fn create(
        &self,
        input: CreateShippingProviderAccountInput,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        require_provider_administrator(input.actor)?;
        self.require_supported(&input.provider, input.enabled)?;
        let account =
            ShippingProviderAccount::create(input.provider, input.display_name, input.enabled)?;
        let configuration = ShippingProviderAccountConfiguration {
            credential_secret_reference: ShippingSecretReference::new(
                input.credential_secret_reference,
            )?,
            origin: validate_origin(input.origin)?,
        };
        self.repository
            .create(
                input.actor,
                input.store_id,
                &account,
                &configuration,
                &input.idempotency,
            )
            .await
    }

    pub async fn update(
        &self,
        input: UpdateShippingProviderAccountInput,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        require_provider_administrator(input.actor)?;
        let mut detail = self
            .repository
            .get(input.actor, input.store_id, input.id)
            .await?
            .ok_or_else(|| shipping_provider_account_not_found(input.id))?;
        self.require_supported(detail.account.provider(), input.enabled)?;
        detail
            .account
            .update_administration(input.display_name, input.enabled)?;
        let configuration = ShippingProviderAccountConfiguration {
            credential_secret_reference: ShippingSecretReference::new(
                input.credential_secret_reference,
            )?,
            origin: validate_origin(input.origin)?,
        };
        self.repository
            .update(
                input.actor,
                input.store_id,
                &detail.account,
                &configuration,
                &input.idempotency,
            )
            .await
    }

    fn require_supported(&self, provider: &str, enabled: bool) -> Result<(), ApplicationError> {
        if !enabled || self.supported_providers.contains(provider) {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "shipping_provider_not_supported",
                message: "the Shipping Provider cannot be enabled by this deployment",
            })
        }
    }
}

impl ShippingManagement {
    pub fn new(repository: Arc<dyn ShippingServiceRepository>) -> Self {
        Self { repository }
    }

    pub async fn create(
        &self,
        input: CreateShippingServiceInput,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        require_operator(input.actor)?;
        let currency = CurrencyCode::parse(&input.currency)?;
        let service = ShippingService::create(
            input.code,
            input.name,
            Money::new(input.amount_minor, currency),
            input.estimated_min_days,
            input.estimated_max_days,
            input.destination_countries,
        )?;
        self.repository
            .create_shipping_service(input.actor, input.store_id, &service, &input.idempotency)
            .await
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingServiceDetail>, ApplicationError> {
        require_operator(actor)?;
        self.repository
            .list_shipping_services(actor, store_id)
            .await
    }

    pub async fn change_status(
        &self,
        input: ChangeShippingServiceStatusInput,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .change_shipping_service_status(
                input.actor,
                input.store_id,
                input.service_id,
                input.status,
                &input.idempotency,
            )
            .await
    }
}

impl FulfillmentManagement {
    pub fn new(repository: Arc<dyn FulfillmentRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(actor)?;
        self.repository
            .get_return(actor, store_id, return_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "return",
                id: return_id.as_uuid().to_string(),
            })
    }

    pub async fn create_fulfillment(
        &self,
        input: CreateFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .create_fulfillment(
                input.actor,
                input.store_id,
                input.order_id,
                input.allocations,
                &input.idempotency,
            )
            .await
    }

    pub async fn transition_fulfillment(
        &self,
        input: TransitionFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .transition_fulfillment(
                input.actor,
                input.store_id,
                input.fulfillment_id,
                input.target_status,
                input.carrier.as_deref(),
                input.tracking_number.as_deref(),
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn create_return(
        &self,
        input: CreateReturnInput,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .create_return(
                input.actor,
                input.store_id,
                input.order_id,
                input.lines,
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn transition_return(
        &self,
        input: TransitionReturnInput,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .transition_return(
                input.actor,
                input.store_id,
                input.return_id,
                input.target_status,
                input.receipt,
                input.now,
                &input.idempotency,
            )
            .await
    }
}

fn require_operator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator | MerchantRole::Manager
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_provider_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn shipping_provider_account_not_found(id: ShippingProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn validate_origin(mut origin: ShippingAddress) -> Result<ShippingAddress, ApplicationError> {
    origin.name = required_text("origin.name", origin.name, 120)?;
    origin.company = optional_text("origin.company", origin.company, 120)?;
    origin.address_line_1 = required_text("origin.address_line_1", origin.address_line_1, 200)?;
    origin.address_line_2 = optional_text("origin.address_line_2", origin.address_line_2, 200)?;
    origin.city = required_text("origin.city", origin.city, 120)?;
    origin.region = optional_text("origin.region", origin.region, 120)?;
    origin.postal_code = required_text("origin.postal_code", origin.postal_code, 32)?;
    origin.country_code = origin.country_code.trim().to_ascii_uppercase();
    if origin.country_code.len() != 2
        || !origin
            .country_code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase())
    {
        return Err(field_error(
            "origin.country_code",
            "must be an ISO 3166-1 alpha-2 country code",
        ));
    }
    origin.phone = optional_text("origin.phone", origin.phone, 32)?;
    origin.email = optional_text("origin.email", origin.email, 254)?;
    if origin
        .email
        .as_ref()
        .is_some_and(|value| !value.contains('@'))
    {
        return Err(field_error("origin.email", "must be a valid email address"));
    }
    Ok(origin)
}

fn required_text(
    field: &'static str,
    value: String,
    max: usize,
) -> Result<String, ApplicationError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max {
        Err(field_error(
            field,
            "must be non-empty and within its length limit",
        ))
    } else {
        Ok(value)
    }
}

fn optional_text(
    field: &'static str,
    value: Option<String>,
    max: usize,
) -> Result<Option<String>, ApplicationError> {
    value
        .map(|value| required_text(field, value, max))
        .transpose()
}

fn field_error(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
