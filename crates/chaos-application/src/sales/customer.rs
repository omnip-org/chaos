use std::sync::Arc;

use chaos_domain::{
    merchant::ApiKeyScope,
    sales::{CustomerAddress, CustomerAddressId},
};

use crate::{
    ApplicationError,
    ports::{
        CustomerActor, CustomerAddressDetail, CustomerDetail, CustomerOrderPage,
        CustomerRepository, IdempotencyRequest, ShopperActor,
    },
};

use super::{PostalAddressInput, postal_address, require_storefront_scope};

pub struct AssociateCustomerInput {
    pub shopper: ShopperActor,
    pub user_id: chaos_domain::identity::UserId,
    pub idempotency: IdempotencyRequest,
}

pub struct UpdateCustomerInput {
    pub actor: CustomerActor,
    pub phone: Option<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateCustomerAddressInput {
    pub actor: CustomerActor,
    pub label: String,
    pub address: PostalAddressInput,
    pub idempotency: IdempotencyRequest,
}

pub struct DeleteCustomerAddressInput {
    pub actor: CustomerActor,
    pub address_id: CustomerAddressId,
    pub idempotency: IdempotencyRequest,
}

pub struct CustomerService {
    repository: Arc<dyn CustomerRepository>,
}

impl CustomerService {
    pub fn new(repository: Arc<dyn CustomerRepository>) -> Self {
        Self { repository }
    }

    pub async fn associate(
        &self,
        input: AssociateCustomerInput,
    ) -> Result<CustomerDetail, ApplicationError> {
        require_storefront_scope(&input.shopper.machine, ApiKeyScope::CartsWrite)?;
        self.repository
            .associate(&input.shopper, input.user_id, &input.idempotency)
            .await
    }

    pub async fn get(
        &self,
        actor: &CustomerActor,
    ) -> Result<Option<CustomerDetail>, ApplicationError> {
        require_storefront_scope(&actor.machine, ApiKeyScope::CartsWrite)?;
        self.repository.get(actor).await
    }

    pub async fn update(
        &self,
        input: UpdateCustomerInput,
    ) -> Result<CustomerDetail, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
        self.repository
            .update_phone(&input.actor, input.phone.as_deref(), &input.idempotency)
            .await
    }

    pub async fn create_address(
        &self,
        input: CreateCustomerAddressInput,
    ) -> Result<CustomerAddressDetail, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
        let address = CustomerAddress::create(input.label, postal_address(input.address)?)?;
        self.repository
            .create_address(&input.actor, &address, &input.idempotency)
            .await
    }

    pub async fn delete_address(
        &self,
        input: DeleteCustomerAddressInput,
    ) -> Result<chaos_domain::sales::CustomerId, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
        self.repository
            .delete_address(&input.actor, input.address_id, &input.idempotency)
            .await
    }

    pub async fn list_orders(
        &self,
        actor: &CustomerActor,
        after: Option<uuid::Uuid>,
        limit: u16,
    ) -> Result<CustomerOrderPage, ApplicationError> {
        require_storefront_scope(&actor.machine, ApiKeyScope::CheckoutWrite)?;
        self.repository.list_orders(actor, after, limit).await
    }
}
