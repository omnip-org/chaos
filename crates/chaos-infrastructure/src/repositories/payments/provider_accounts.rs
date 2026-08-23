// Payment provider account configuration and onboarding persistence.

#[async_trait]
impl StripeAccountRepository for PostgresPaymentRepository {
    async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<StripeAccountPage, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let rows = sqlx::query_as::<_, ProviderAccountRow>(
            "SELECT id, provider, display_name, enabled, \
                    credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                    readiness_status, readiness_checked_at, readiness_valid_until, \
                    COALESCE(readiness_snapshot->'blocker_codes', '[]'::jsonb), \
                    credential_rotation_expires_at, webhook_rotation_expires_at, \
                    created_at, updated_at \
             FROM commerce.payment_provider_accounts \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id < $2) \
             ORDER BY id DESC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let has_more = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(stripe_account_detail)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(StripeAccountPage { items, has_more })
    }

    async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: StripeAccountId,
    ) -> Result<Option<StripeAccountDetail>, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let value = load_stripe_account(&mut transaction, store_id, id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn create(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &StripeAccount,
        configuration: &StripeAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            return replay_stripe_account(&mut transaction, store_id, snapshot).await;
        }
        let readiness = configuration.readiness.as_ref();
        sqlx::query(
            "INSERT INTO commerce.payment_provider_accounts \
             (id, store_id, provider, display_name, \
              credential_secret_reference, webhook_secret_reference, \
              readiness_status, readiness_snapshot, readiness_checked_at, \
              readiness_valid_until, readiness_reconcile_at, \
              enabled, created_by_user_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(account.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind("stripe_checkout")
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(configuration.webhook_secret_reference.expose_reference())
        .bind(readiness.map_or(
            StripeReadinessStatus::Unchecked.as_str(),
            |value| readiness_status(value).as_str(),
        ))
        .bind(readiness.map(|value| &value.configuration))
        .bind(readiness.map(|value| value.checked_at))
        .bind(
            readiness
                .filter(|value| value.ready)
                .map(|value| value.checked_at + time::Duration::hours(24)),
        )
        .bind(
            readiness
                .filter(|value| value.ready)
                .map(|value| value.checked_at + time::Duration::hours(6)),
        )
        .bind(account.enabled())
        .bind(actor.user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        complete_stripe_account(
            &mut transaction,
            store_id,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_stripe_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn update(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &StripeAccount,
        configuration: &StripeAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            return replay_stripe_account(&mut transaction, store_id, snapshot).await;
        }
        let readiness = configuration.readiness.as_ref();
        let result = sqlx::query(
            "UPDATE commerce.payment_provider_accounts SET display_name = $3, \
                    previous_credential_secret_reference = CASE \
                        WHEN credential_secret_reference IS NOT NULL \
                             AND credential_secret_reference IS DISTINCT FROM $4 \
                        THEN credential_secret_reference ELSE previous_credential_secret_reference END, \
                    credential_rotation_expires_at = CASE \
                        WHEN credential_secret_reference IS NOT NULL \
                             AND credential_secret_reference IS DISTINCT FROM $4 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE credential_rotation_expires_at END, \
                    credential_secret_reference = $4, \
                    previous_webhook_secret_reference = CASE \
                        WHEN webhook_secret_reference IS NOT NULL \
                             AND webhook_secret_reference IS DISTINCT FROM $5 \
                        THEN webhook_secret_reference ELSE previous_webhook_secret_reference END, \
                    webhook_rotation_expires_at = CASE \
                        WHEN webhook_secret_reference IS NOT NULL \
                             AND webhook_secret_reference IS DISTINCT FROM $5 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE webhook_rotation_expires_at END, \
                    webhook_secret_reference = $5, \
                    readiness_status = CASE \
                        WHEN $7::text IS NOT NULL THEN $7 \
                        WHEN credential_secret_reference IS DISTINCT FROM $4 \
                        THEN 'unchecked' \
                        ELSE readiness_status END, \
                    readiness_snapshot = CASE \
                        WHEN $7::text IS NOT NULL THEN $8::jsonb \
                        WHEN credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL \
                        ELSE readiness_snapshot END, \
                    readiness_checked_at = CASE \
                        WHEN $7::text IS NOT NULL THEN $9::timestamptz \
                        WHEN credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL \
                        ELSE readiness_checked_at END, \
                    readiness_valid_until = CASE \
                        WHEN $7::text = 'ready' THEN $9::timestamptz + INTERVAL '24 hours' \
                        WHEN $7::text IS NOT NULL THEN NULL \
                        WHEN credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL \
                        ELSE readiness_valid_until END, \
                    readiness_reconcile_at = CASE \
                        WHEN $7::text = 'ready' THEN $9::timestamptz + INTERVAL '6 hours' \
                        WHEN $7::text IS NOT NULL THEN NULL \
                        WHEN credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL \
                        ELSE readiness_reconcile_at END, \
                    readiness_locked_by = CASE \
                        WHEN $7::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL ELSE readiness_locked_by END, \
                    readiness_locked_at = CASE \
                        WHEN $7::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL ELSE readiness_locked_at END, \
                    readiness_reconcile_attempts = CASE \
                        WHEN $7::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $4 \
                        THEN 0 ELSE readiness_reconcile_attempts END, \
                    readiness_last_error = CASE \
                        WHEN $7::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $4 \
                        THEN NULL ELSE readiness_last_error END, \
                    enabled = $6, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(account.id().as_uuid())
        .bind(account.display_name())
        .bind(
            configuration
                .credential_secret_reference
                .expose_reference(),
        )
        .bind(configuration.webhook_secret_reference.expose_reference())
        .bind(account.enabled())
        .bind(readiness.map(|value| readiness_status(value).as_str()))
        .bind(readiness.map(|value| &value.configuration))
        .bind(readiness.map(|value| value.checked_at))
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(provider_account_not_found(account.id()));
        }
        complete_stripe_account(
            &mut transaction,
            store_id,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_stripe_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}
