// Payment provider account configuration persistence.

impl PostgresStripeRepository {
    pub(crate) async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<StripeAccountPage, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let rows = sqlx::query_as::<_, ProviderAccountRow>(
            "SELECT id, display_name, \
                    credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
             created_at, updated_at \
             FROM integration.payment_provider_accounts \
             WHERE store_id = $1 AND provider = 'stripe' \
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

    pub(crate) async fn get(
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

    pub(crate) async fn create(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &StripeAccount,
        configuration: &StripeAccountConfiguration,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        sqlx::query(
            "INSERT INTO integration.payment_provider_accounts \
             (id, store_id, provider, display_name, \
              credential_secret_reference, webhook_secret_reference) \
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(account.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind("stripe")
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(configuration.webhook_secret_reference.expose_reference())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        let value = load_stripe_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    pub(crate) async fn update(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &StripeAccount,
        configuration: &StripeAccountConfiguration,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let result = sqlx::query(
            "UPDATE integration.payment_provider_accounts SET display_name = $3, \
                    credential_secret_reference = $4, \
                    webhook_secret_reference = $5, \
                    updated_at = CURRENT_TIMESTAMP \
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
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(provider_account_not_found(account.id()));
        }
        let value = load_stripe_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}
