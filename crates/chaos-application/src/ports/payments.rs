use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    merchant::StoreId,
    payments::{PaymentAttemptId, PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, merchant::MerchantActor};

use super::{IdempotencyRequest, MachineActor};

pub struct PaymentAttemptDetail {
    pub id: PaymentAttemptId,
    pub order_id: OrderId,
    pub provider: String,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: PaymentAttemptStatus,
    pub provider_reference: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct RefundDetail {
    pub id: RefundId,
    pub payment_attempt_id: PaymentAttemptId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: RefundStatus,
    pub provider_reference: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct VerifiedWebhookEvent {
    pub provider: String,
    pub provider_event_id: String,
    pub event_type: String,
    pub external_account_reference: String,
    pub object_reference: String,
    pub failure_code: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

pub struct QueueJob {
    pub id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

pub struct ProviderCommand {
    pub event_type: String,
    pub aggregate_id: Uuid,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
}

pub struct ProviderCommandResult {
    pub provider_reference: String,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError>;
}

pub trait PaymentWebhookVerifier: Send + Sync {
    fn verify(
        &self,
        provider: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError>;
}

#[async_trait]
pub trait PaymentRepository: Send + Sync {
    async fn create_attempt(
        &self,
        actor: &MachineActor,
        order_id: OrderId,
        provider: &str,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentAttemptDetail, ApplicationError>;

    async fn get_attempt(
        &self,
        actor: &MachineActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError>;

    async fn create_refund(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        attempt_id: PaymentAttemptId,
        amount_minor: i64,
        idempotency: &IdempotencyRequest,
    ) -> Result<RefundDetail, ApplicationError>;

    async fn ingest_webhook(&self, event: &VerifiedWebhookEvent) -> Result<bool, ApplicationError>;

    async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_outbox(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn claim_webhooks(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn finish_outbox(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_webhook(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
