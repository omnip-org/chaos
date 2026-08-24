// Payment attempt creation, retrieval, provider command handling, and payment state updates.

impl PostgresStripeRepository {
    pub(crate) async fn create_attempt(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<PaymentAttemptDetail, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        // total_amount_minor is not yet known here: it is a Stripe-reported
        // fact filled in by the checkout webhook once Stripe applies tax and
        // shipping. subtotal_amount_minor is the pre-tax reference amount
        // Chaos already knows at checkout start.
        let order = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT subtotal_amount_minor, currency::text, status::text, payment_status::text \
                    FROM commerce.orders \
             WHERE store_id = $1 AND sales_channel_id = $2 \
               AND id = $3 AND shopper_id = $4 FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        if order.2 != "pending" || matches!(order.3.as_str(), "paid" | "partially_refunded" | "refunded") {
            return Err(payment_order_not_pending());
        }
        let currency = CurrencyCode::parse(&order.1)?;
        let attempt = PaymentAttempt::create(order_id, Money::new(order.0, currency))?;
        sqlx::query(
            "INSERT INTO commerce.payment_attempts \
             (id, store_id, order_id, currency, status, amount_minor) \
             VALUES ($1, $2, $3, $4, 'pending', $5)",
        )
        .bind(attempt.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(currency.as_str())
        .bind(order.0)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_attempt(
            &mut transaction,
            actor.store_id,
            Some(channel_id),
            Some(shopper.shopper_id.as_uuid()),
            attempt.id(),
        )
        .await?
        .ok_or_else(|| attempt_not_found(attempt.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn prepare_checkout_command(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
        return_url: &str,
        idempotency_key: &str,
    ) -> Result<StripeCommand, ApplicationError> {
        let attempt = self
            .get_attempt(actor, attempt_id)
            .await?
            .ok_or_else(|| attempt_not_found(attempt_id))?;
        let job = direct_checkout_job(actor, &attempt, return_url);
        let mut command = self.prepare_stripe_command(&job).await?;
        command.idempotency_key = idempotency_key.to_owned();
        Ok(command)
    }

    pub(crate) async fn record_checkout_result(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
        result: &StripeCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let attempt = self
            .get_attempt(actor, attempt_id)
            .await?
            .ok_or_else(|| attempt_not_found(attempt_id))?;
        let job = direct_checkout_job(actor, &attempt, "");
        self.record_stripe_result(&job, result, now).await
    }

    pub(crate) async fn get_attempt(
        &self,
        shopper: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        let detail = load_attempt(
            &mut transaction,
            actor.store_id,
            actor.sales_channel_id,
            Some(shopper.shopper_id.as_uuid()),
            attempt_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn create_refund(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        attempt_id: PaymentAttemptId,
        amount_minor: i64,
    ) -> Result<RefundDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let row = sqlx::query_as::<_, (i64, String, String, Uuid)>(
            "SELECT amount_minor, currency::text, status::text, order_id \
             FROM commerce.payment_attempts WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| attempt_not_found(attempt_id))?;
        let currency = CurrencyCode::parse(&row.1)?;
        let order_id = row.3;
        let attempt = PaymentAttempt::rehydrate(
            attempt_id,
            OrderId::from_uuid(order_id),
            Money::new(row.0, currency),
            PaymentAttemptStatus::parse(&row.2).ok_or_else(corrupt_payment_state)?,
            None,
        );
        // Pending refunds already claim their share of the captured amount,
        // so a second concurrent request cannot double-spend it before the
        // first one confirms via webhook.
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM commerce.refunds \
             WHERE store_id = $1 AND payment_attempt_id = $2 AND status IN ('pending', 'succeeded')",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refund = Refund::create(&attempt, Money::new(amount_minor, currency), already_refunded)?;
        let id = refund.id();
        sqlx::query(
            "INSERT INTO commerce.refunds \
             (id, store_id, payment_attempt_id, order_id, currency, status, amount_minor, \
              created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7)",
        )
        .bind(id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(order_id)
        .bind(currency.as_str())
        .bind(amount_minor)
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        insert_outbox(
            &mut transaction,
            store_id,
            "refund",
            id.as_uuid(),
            "refund.create_requested",
            amount_minor,
            currency,
            None,
        )
        .await?;
        let detail = RefundDetail {
            id,
            payment_attempt_id: attempt_id,
            amount_minor,
            currency,
            status: RefundStatus::Pending,
            stripe_refund_id: None,
            failure_code: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn ingest_webhook(&self, event: &StripeWebhookEvent) -> Result<bool, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let account = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT provider_account_id, store_id \
             FROM commerce.resolve_provider_account('stripe_checkout', $1)",
        )
        .bind(event.stripe_account_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(provider_unavailable)?;
        set_config(&mut transaction, "app.store_id", account.1).await?;
        let result = sqlx::query(
            "INSERT INTO commerce.provider_webhooks \
             (id, store_id, provider, provider_account_id, provider_event_id, event_type, \
              payload, verified_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (provider_account_id, provider_event_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(account.1)
        .bind("stripe_checkout")
        .bind(event.stripe_account_id)
        .bind(&event.stripe_event_id)
        .bind(&event.event_type)
        .bind(&event.payload)
        .bind(event.verified_at)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let stripe_object_id = job
            .payload
            .get("object")
            .and_then(Value::as_str)
            .ok_or_else(corrupt_webhook_payload)?
            .to_owned();
        let failure_code = job
            .payload
            .get("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let order_id = if job.event_type.starts_with("payment.") {
            let aggregate_id = job
                .payload
                .get("aggregate_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .filter(|value| !value.is_nil())
                .ok_or_else(corrupt_webhook_payload)?;
            apply_payment_event(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                PaymentAttemptId::from_uuid(aggregate_id),
                &job.event_type,
                stripe_object_id,
                failure_code,
                &job.payload,
                now,
            )
            .await?
        } else if job.event_type.starts_with("refund.") {
            let refund_id = job
                .payload
                .get("aggregate_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .filter(|value| !value.is_nil())
                .map(RefundId::from_uuid);
            apply_refund_event(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                refund_id,
                &job.event_type,
                stripe_object_id,
                failure_code,
                &job.payload,
                now,
            )
            .await?
        } else {
            return Err(corrupt_webhook_payload());
        };
        // Backfills the raw webhook snapshot's Order link now that the
        // event has been resolved to one; a webhook that fails before this
        // point leaves provider_webhooks.order_id NULL. provider_webhooks is
        // otherwise append-only for chaos_runtime, so this goes through a
        // narrow SECURITY DEFINER function rather than a direct UPDATE.
        sqlx::query_scalar::<_, bool>("SELECT commerce.set_webhook_order_id($1, $2)")
            .bind(job.id)
            .bind(order_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn prepare_stripe_command(
        &self,
        job: &QueueJob,
    ) -> Result<StripeCommand, ApplicationError> {
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        if !matches!(job.event_type.as_str(), "payment.create_requested" | "refund.create_requested") {
            return Err(invalid_outbox_payload());
        }
        let is_refund = job.event_type == "refund.create_requested";
        type ContextRow = (i64, String, Uuid, String, Option<String>, Uuid);
        let row: ContextRow = if is_refund {
            sqlx::query_as(
                "SELECT refund.amount_minor, refund.currency::text, \
                        account.id, account.credential_secret_reference, \
                        attempt.stripe_payment_intent_id, refund.order_id \
                 FROM commerce.refunds AS refund \
                 INNER JOIN commerce.payment_attempts AS attempt \
                   ON attempt.store_id = refund.store_id AND attempt.id = refund.payment_attempt_id \
                 INNER JOIN commerce.payment_provider_accounts AS account \
                   ON account.store_id = refund.store_id \
                  AND account.provider = 'stripe_checkout' AND account.enabled \
                  AND account.readiness_status = 'ready' \
                  AND account.readiness_valid_until > CURRENT_TIMESTAMP \
                 WHERE refund.store_id = $1 AND refund.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        } else {
            sqlx::query_as(
                "SELECT attempt.amount_minor, attempt.currency::text, \
                        account.id, account.credential_secret_reference, \
                        attempt.stripe_payment_intent_id, attempt.order_id \
                 FROM commerce.payment_attempts AS attempt \
                 INNER JOIN commerce.payment_provider_accounts AS account \
                   ON account.store_id = attempt.store_id \
                  AND account.provider = 'stripe_checkout' AND account.enabled \
                  AND account.readiness_status = 'ready' \
                  AND account.readiness_valid_until > CURRENT_TIMESTAMP \
                 WHERE attempt.store_id = $1 AND attempt.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        };
        let command_amount = row.0;
        if command_amount != outbox_amount(job)? || row.1 != outbox_currency(job)? {
            return Err(invalid_outbox_payload());
        }
        if is_refund && row.4.is_none() {
            return Err(ApplicationError::Conflict {
                code: "stripe_payment_reference_missing",
                message: "the captured Payment Attempt has no Stripe payment reference",
            });
        }
        let checkout_details = if !is_refund {
            let order_id = row.5;
            let contact = sqlx::query_as::<
                _,
                (
                    String,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
            >(
                "SELECT contact_email::text, contact_phone, shipping_full_name, \
                        shipping_address_line1, shipping_address_line2, shipping_locality, \
                        shipping_administrative_area, shipping_postal_code, \
                        NULLIF(btrim(shipping_country_code::text), '') \
                 FROM commerce.orders WHERE store_id = $1 AND id = $2",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(corrupt_state)?;
            let shipping_address = contact
                .2
                .map(|name| {
                    let Some(line1) = contact.3 else {
                        return Err(corrupt_state());
                    };
                    let Some(city) = contact.5 else {
                        return Err(corrupt_state());
                    };
                    let Some(country_code) = contact.8 else {
                        return Err(corrupt_state());
                    };
                    Ok(PaymentShippingAddress {
                        name,
                        line1,
                        line2: contact.4,
                        city,
                        state: contact.6,
                        postal_code: contact.7,
                        country_code,
                    })
                })
                .transpose()?;
            let line_rows = sqlx::query_as::<_, (String, String, Option<String>, i32, i64, bool)>(
                "SELECT product_title, variant_title, sku, quantity, \
                        unit_price_amount_minor, requires_shipping \
                 FROM commerce.order_lines WHERE store_id = $1 AND order_id = $2 \
                 ORDER BY position",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            let has_shippable_items = line_rows.iter().any(|line| line.5);
            let line_items = line_rows
                .into_iter()
                .map(|line| {
                    Ok::<_, ApplicationError>(PaymentLineItem {
                        name: if line.1.trim().is_empty() {
                            line.0
                        } else {
                            format!("{} — {}", line.0, line.1)
                        },
                        sku: line.2,
                        quantity: u32::try_from(line.3).map_err(unexpected_conversion)?,
                        unit_amount_minor: line.4,
                    })
                })
                .collect::<Result<Vec<_>, ApplicationError>>()?;
            // The Store's explicit shipping range is the only allowed
            // destination. Stripe collects the destination in Checkout and
            // uses it for tax.
            let shipping_countries = if has_shippable_items {
                sqlx::query_scalar::<_, String>(
                    "SELECT country_code::text FROM commerce.store_shipping_countries \
                     WHERE store_id = $1 AND enabled ORDER BY country_code",
                )
                .bind(job.store_id)
                .fetch_all(&mut *transaction)
                .await
                .map_err(database_error)?
            } else {
                Vec::new()
            };
            // Shipping rates and destination rules belong to Stripe Checkout.
            // Chaos only stores the address and the provider's final shipping amount.
            let shipping_options = Vec::new();
            Some(PaymentCheckoutDetails {
                customer_email: contact.0,
                customer_phone: contact.1,
                shipping_address,
                line_items,
                shipping_countries,
                shipping_options,
                automatic_tax: true,
            })
        } else {
            None
        };
        transaction.commit().await.map_err(database_error)?;
        let return_url = outbox_return_url(job);
        Ok(StripeCommand {
            stripe_account_id: StripeAccountId::from_uuid(row.2),
            event_type: job.event_type.clone(),
            aggregate_id,
            amount_minor: command_amount,
            currency: CurrencyCode::parse(&row.1)?,
            idempotency_key: job.id.to_string(),
            credential_secret_reference: PaymentSecretReference::new(
                "credential_secret_reference",
                row.3,
            )?,
            stripe_payment_reference: row.4,
            checkout_details,
            return_url,
        })
    }

    pub(crate) async fn record_stripe_result(
        &self,
        job: &QueueJob,
        result: &StripeCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.stripe_object_id.trim().is_empty()
            || result.stripe_object_id.chars().count() > 255
        {
            return Err(stripe_invalid_response());
        }
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let rows = if job.event_type == "payment.create_requested" {
            sqlx::query(
                "UPDATE commerce.payment_attempts \
                 SET stripe_checkout_session_id = COALESCE(stripe_checkout_session_id, $3), \
                     updated_at = CASE WHEN stripe_checkout_session_id IS NULL THEN $4 ELSE updated_at END \
                 WHERE store_id = $1 AND id = $2 \
                   AND (stripe_checkout_session_id IS NULL OR stripe_checkout_session_id = $3)",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.stripe_object_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected()
        } else if job.event_type == "refund.create_requested" {
            sqlx::query(
                "UPDATE commerce.refunds \
                 SET stripe_refund_id = COALESCE(stripe_refund_id, $3), \
                     updated_at = CASE WHEN stripe_refund_id IS NULL THEN $4 ELSE updated_at END \
                 WHERE store_id = $1 AND id = $2 \
                   AND (stripe_refund_id IS NULL OR stripe_refund_id = $3)",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.stripe_object_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected()
        } else {
            return Err(invalid_outbox_payload());
        };
        if rows != 1 {
            return Err(stripe_object_mismatch());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

}

fn direct_checkout_job(
    actor: &ShopperActor,
    attempt: &PaymentAttemptDetail,
    return_url: &str,
) -> QueueJob {
    QueueJob {
        id: attempt.id.as_uuid(),
        store_id: actor.machine.store_id.as_uuid(),
        event_type: "payment.create_requested".into(),
        payload: json!({
            "aggregate_id": attempt.id.as_uuid(),
            "amount_minor": attempt.amount_minor,
            "currency": attempt.currency.as_str(),
            "return_url": return_url,
        }),
        attempts: 1,
    }
}
