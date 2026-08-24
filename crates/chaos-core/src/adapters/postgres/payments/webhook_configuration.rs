// Payment provider webhook configuration persistence.

#[async_trait]
impl StripeWebhookConfigurationRepository for PostgresStripeRepository {
    async fn webhook_configuration(
        &self,
        provider_account_id: StripeAccountId,
    ) -> Result<Vec<StripeWebhookConfiguration>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT provider_account_id, secret_reference \
             FROM commerce.resolve_provider_webhook_secret_references($1::integration.payment_provider, $2)",
        )
        .bind("stripe")
        .bind(provider_account_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|(provider_account_id, reference)| {
            Ok(StripeWebhookConfiguration {
                stripe_account_id: provider_account_id,
                secret_reference: PaymentSecretReference::new(
                    "webhook_secret_reference",
                    reference,
                )?,
            })
        })
        .collect()
    }
}
