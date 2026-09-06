use std::sync::Arc;

use chaos_domain::{
    sales::OrderId,
    store::{StoreId, StoreRole},
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresStripeRepository,
    contracts::{
        AdminActor, MachineActor, PaymentClientAction, PaymentProviderRegistry,
        PaymentWebhookVerifierRegistry, RefundDetail, ShopperActor, StripeAccountConfiguration,
        StripeAccountDetail, StripeAccountPage, VerifiedWebhookEvent, WebhookInbox,
    },
    store::StoreActor,
};

pub struct CreateEmbeddedCheckoutInput {
    pub actor: ShopperActor,
    pub order_id: OrderId,
    pub return_url: String,
    pub now: OffsetDateTime,
}

pub struct EmbeddedCheckoutResult {
    pub order_number: String,
    pub source_cart_id: chaos_domain::sales::CartId,
    pub client_action: PaymentClientAction,
}

pub struct CreateRefundInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub amount_minor: i64,
    pub now: OffsetDateTime,
}

pub struct ReconcileRefundsInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub now: OffsetDateTime,
}

pub struct RefundReconciliationResult {
    pub order_id: OrderId,
    pub refunded_amount_minor: i64,
    pub refunds: Vec<RefundDetail>,
}

pub struct CreateStripeAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
}

pub struct UpdateStripeAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub id: StripeAccountId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
}

pub struct StripeAccountAdministration {
    repository: Arc<PostgresStripeRepository>,
}

impl StripeAccountAdministration {
    pub fn new(repository: Arc<PostgresStripeRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<StripeAccountPage, ApplicationError> {
        self.repository.list(actor, store_id, after, limit).await
    }

    pub async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: StripeAccountId,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        self.repository
            .get(actor, store_id, id)
            .await?
            .ok_or_else(|| stripe_account_not_found(id))
    }

    pub async fn create(
        &self,
        input: CreateStripeAccountInput,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        require_stripe_account_administrator(input.actor)?;
        let account = StripeAccount::create(input.display_name)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        let configuration = StripeAccountConfiguration {
            credential_secret_reference: credential,
            webhook_secret_reference: webhook,
        };
        self.repository
            .create(input.actor, input.store_id, &account, &configuration)
            .await
    }

    pub async fn update(
        &self,
        input: UpdateStripeAccountInput,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        require_stripe_account_administrator(input.actor)?;
        let mut detail = self
            .repository
            .get(input.actor, input.store_id, input.id)
            .await?
            .ok_or_else(|| stripe_account_not_found(input.id))?;
        detail.account.update_administration(input.display_name)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        let configuration = StripeAccountConfiguration {
            credential_secret_reference: credential,
            webhook_secret_reference: webhook,
        };
        self.repository
            .update(input.actor, input.store_id, &detail.account, &configuration)
            .await
    }
}

pub struct PaymentService {
    repository: Arc<PostgresStripeRepository>,
    webhook_inbox: Arc<dyn WebhookInbox>,
    webhook_verifiers: Arc<PaymentWebhookVerifierRegistry>,
    payment_providers: Arc<PaymentProviderRegistry>,
}

impl PaymentService {
    pub fn new(
        repository: Arc<PostgresStripeRepository>,
        webhook_inbox: Arc<dyn WebhookInbox>,
        webhook_verifiers: Arc<PaymentWebhookVerifierRegistry>,
        payment_providers: Arc<PaymentProviderRegistry>,
    ) -> Self {
        Self {
            repository,
            webhook_inbox,
            webhook_verifiers,
            payment_providers,
        }
    }

    pub async fn create_embedded_checkout(
        &self,
        input: CreateEmbeddedCheckoutInput,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        require_checkout_key(&input.actor.machine)?;
        self.open_embedded_checkout(
            input.actor,
            input.order_id,
            Some(input.return_url),
            input.now,
        )
        .await
    }

    async fn open_embedded_checkout(
        &self,
        actor: ShopperActor,
        order_id: OrderId,
        return_url: Option<String>,
        now: OffsetDateTime,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        let payment = self
            .repository
            .get_order_checkout_payment(&actor, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        ensure_order_payment_open(&payment)?;
        if let Some(client_action) = payment.client_action {
            return Ok(EmbeddedCheckoutResult {
                order_number: payment.order_number,
                source_cart_id: payment.source_cart_id,
                client_action,
            });
        }
        let return_url = return_url.ok_or_else(checkout_return_url_required)?;

        let provider = self
            .payment_providers
            .get(&payment.provider)
            .ok_or_else(payment_provider_not_supported)?;
        let command = self
            .repository
            .prepare_checkout_command(&actor, &payment, &return_url)
            .await?;
        let result = provider.execute(command).await?;
        if result.client_action.is_none() {
            self.repository
                .fail_checkout_order(&actor, order_id, "checkout_client_action_missing", now)
                .await?;
            return Err(checkout_client_action_missing());
        }
        self.repository
            .record_checkout_result(&actor, order_id, &result, now)
            .await?;
        let payment = self
            .repository
            .get_order_checkout_payment(&actor, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        let Some(client_action) = payment.client_action else {
            // A verified webhook can win the race between provider execution
            // and this reload. In that case the action was intentionally
            // cleared because the Order is terminal; report that state rather
            // than hiding it behind a misleading missing-secret error.
            ensure_order_payment_open(&payment)?;
            return Err(checkout_client_action_missing());
        };
        Ok(EmbeddedCheckoutResult {
            order_number: payment.order_number,
            source_cart_id: payment.source_cart_id,
            client_action,
        })
    }

    pub async fn create_refund(
        &self,
        input: CreateRefundInput,
    ) -> Result<RefundDetail, ApplicationError> {
        require_payment_operator(&input.actor)?;
        let store_id = input.store_id.as_uuid();
        let detail = self
            .repository
            .create_refund(
                input.actor,
                input.store_id,
                input.order_id,
                input.amount_minor,
            )
            .await?;
        // The refund now exists as a pending row; issue it at the provider in
        // the same request. On provider failure the row stays pending and the
        // error surfaces to the caller — `reconcile_refunds` is the recovery
        // path.
        // ponytail: sync provider call, no retry — add a retry queue only if
        // provider flakiness actually bites.
        let command_payload = json!({
            "store_id": store_id,
            "aggregate_id": detail.id.as_uuid(),
            "amount_minor": detail.amount_minor,
            "currency": detail.currency.as_str(),
            "return_url": None::<&str>,
            "provider": "stripe",
        });
        let mut command = self
            .repository
            .prepare_payment_command(store_id, true, Some("stripe"), &command_payload)
            .await?;
        command.idempotency_key = detail.id.as_uuid().to_string();
        let gateway = self
            .payment_providers
            .get("stripe")
            .ok_or_else(payment_provider_not_supported)?;
        let result = gateway.execute(command).await?;
        self.repository
            .record_payment_result(store_id, &command_payload, &result, input.now)
            .await?;
        Ok(detail)
    }

    pub async fn reconcile_refunds(
        &self,
        input: ReconcileRefundsInput,
    ) -> Result<RefundReconciliationResult, ApplicationError> {
        require_payment_operator(&input.actor)?;
        let context = self
            .repository
            .prepare_refund_reconciliation(&input.actor, input.store_id, input.order_id)
            .await?;
        let provider = self
            .payment_providers
            .get("stripe")
            .ok_or_else(payment_provider_not_supported)?;
        let observations = provider
            .list_refunds(
                &context.credential_secret_reference,
                &context.payment_provider_reference,
            )
            .await?;
        let (refunded_amount_minor, refunds) = self
            .repository
            .apply_refund_reconciliation(&context, &observations, input.now)
            .await?;
        Ok(RefundReconciliationResult {
            order_id: input.order_id,
            refunded_amount_minor,
            refunds,
        })
    }

    /// Verifies the wire payload and appends it to the provider webhook audit,
    /// which enqueues a `provider.webhook.received` job. The event is applied
    /// asynchronously by `crate::webhooks::ProviderWebhookWorker`, so the HTTP
    /// caller only ever gets a fast verify + persist. A verification failure
    /// propagates as a non-2xx and the provider retries.
    pub async fn receive_provider_webhook(
        &self,
        provider: &str,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let verifier =
            self.webhook_verifiers
                .get(provider)
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "payment_provider",
                    id: provider.to_owned(),
                })?;
        let event = verifier
            .verify(provider_account_id, signature, payload, received_at)
            .await?;
        let envelope = VerifiedWebhookEvent {
            provider_account_id: event.provider_account_id,
            capability: "payment".into(),
            provider: provider.to_owned(),
            provider_event_id: event.provider_event_id,
            provider_event_type: event.provider_event_type,
            normalized_event_type: event.normalized_event_type,
            payload: event.payload,
            verified_at: event.verified_at,
        };
        if !self.webhook_inbox.record(&envelope).await? {
            tracing::debug!(provider, "duplicate payment webhook ignored");
        }
        Ok(())
    }
}

fn require_checkout_key(actor: &MachineActor) -> Result<(), ApplicationError> {
    if actor.channel_id.is_some() {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_payment_operator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn payment_provider_not_supported() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_not_supported",
        message: "the configured Payment provider has no adapter",
    }
}

fn ensure_order_payment_open(
    payment: &crate::adapters::postgres::OrderCheckoutPayment,
) -> Result<(), ApplicationError> {
    if payment.order_status == "pending" && payment.payment_status == "pending" {
        return Ok(());
    }
    Err(ApplicationError::Conflict {
        code: "order_not_pending",
        message: "the Order is no longer waiting for payment",
    })
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn checkout_return_url_required() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_return_url_required",
        message: "return_url is required when the payment handoff has not been created yet",
    }
}

fn checkout_client_action_missing() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "payment_client_action",
        source: anyhow::anyhow!("the Payment provider returned no client action"),
    }
}

fn require_stripe_account_administrator(actor: StoreActor) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn stripe_account_not_found(id: StripeAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "stripe_account",
        id: id.as_uuid().to_string(),
    }
}
