// Provider webhook configuration and readiness queue persistence.

#[async_trait]
impl StripeWebhookConfigurationRepository for PostgresStripeRepository {
    async fn webhook_configurations(
        &self,
        store_id: StoreId,
    ) -> Result<Vec<StripeWebhookConfiguration>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT provider_account_id, secret_reference \
             FROM commerce.resolve_store_provider_webhook_secret_references($1, $2)",
        )
        .bind("stripe_checkout")
        .bind(store_id.as_uuid())
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

#[async_trait]
impl StripeReadinessQueue for PostgresStripeRepository {
    async fn claim_stripe_readiness(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<StripeReadinessJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, String, i32)>(
            "SELECT provider_account_id, store_id, provider, \
                    credential_secret_reference, attempts \
             FROM commerce.claim_provider_readiness_checks($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(StripeReadinessJob {
                stripe_account_id: StripeAccountId::from_uuid(row.0),
                store_id: StoreId::from_uuid(row.1),
                credential_secret_reference: PaymentSecretReference::new(
                    "credential_secret_reference",
                    row.3,
                )?,
                attempts: u32::try_from(row.4)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            })
        })
        .collect()
    }

    async fn finish_stripe_readiness(
        &self,
        worker_id: Uuid,
        stripe_account_id: StripeAccountId,
        result: Result<StripeReadiness, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, ready, snapshot, checked_at, failure) = match result {
            Ok(readiness) => (
                true,
                readiness.ready,
                readiness.configuration,
                readiness.checked_at,
                String::new(),
            ),
            Err(failure) => (false, false, Value::Null, now, failure),
        };
        let finished: Option<bool> = sqlx::query_scalar(
            "SELECT commerce.finish_provider_readiness_check($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(stripe_account_id.as_uuid())
        .bind(worker_id)
        .bind(succeeded)
        .bind(ready)
        .bind(snapshot)
        .bind(checked_at)
        .bind(failure)
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(queue_job_not_found())
        }
    }
}
