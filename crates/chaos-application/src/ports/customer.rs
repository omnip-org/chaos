use async_trait::async_trait;
use chaos_domain::sales::{Customer, CustomerAddress, CustomerAddressId, CustomerId};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{CustomerActor, IdempotencyRequest, ShopperActor},
};

use super::OrderDetail;

pub struct CustomerAddressDetail {
    pub address: CustomerAddress,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CustomerDetail {
    pub customer: Customer,
    pub addresses: Vec<CustomerAddressDetail>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CustomerOrderPage {
    pub items: Vec<OrderDetail>,
    pub has_more: bool,
}

#[async_trait]
pub trait CustomerRepository: Send + Sync {
    async fn associate(
        &self,
        shopper: &ShopperActor,
        user_id: chaos_domain::identity::UserId,
        request: &IdempotencyRequest,
    ) -> Result<CustomerDetail, ApplicationError>;
    async fn get(&self, actor: &CustomerActor) -> Result<Option<CustomerDetail>, ApplicationError>;
    async fn update_phone(
        &self,
        actor: &CustomerActor,
        phone: Option<&str>,
        request: &IdempotencyRequest,
    ) -> Result<CustomerDetail, ApplicationError>;
    async fn create_address(
        &self,
        actor: &CustomerActor,
        address: &CustomerAddress,
        request: &IdempotencyRequest,
    ) -> Result<CustomerAddressDetail, ApplicationError>;
    async fn delete_address(
        &self,
        actor: &CustomerActor,
        address_id: CustomerAddressId,
        request: &IdempotencyRequest,
    ) -> Result<CustomerId, ApplicationError>;
    async fn list_orders(
        &self,
        actor: &CustomerActor,
        after: Option<uuid::Uuid>,
        limit: u16,
    ) -> Result<CustomerOrderPage, ApplicationError>;
}
