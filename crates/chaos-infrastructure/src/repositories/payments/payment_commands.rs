// Payment attempt creation, retrieval, provider command handling, and payment state updates.

#[async_trait]
impl StripePaymentRepository for PostgresPaymentRepository {
    async fn create_attempt(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
        return_url: Option<&str>,
        request: &IdempotencyRequest,
    ) -> Result<PaymentAttemptDetail, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let order = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT total_amount_minor, currency::text, status::text FROM commerce.orders \
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
        if order.2 != "pending" {
            return Err(payment_order_not_pending());
        }
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ATTEMPT_OPERATION,
            request,
        )
        .await?
        {
            return replay_attempt(snapshot);
        }
        let provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM commerce.provider_accounts \
             WHERE store_id = $1 AND provider = 'stripe_checkout' AND enabled \
               AND readiness_status = 'ready' AND readiness_valid_until > CURRENT_TIMESTAMP \
             ORDER BY id LIMIT 1",
        )
        .bind(actor.store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(provider_unavailable)?;
        let active_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM commerce.payment_attempts \
             WHERE store_id = $1 AND order_id = $2 \
               AND status IN ('pending', 'authorized', 'captured'))",
        )
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if active_exists {
            return Err(active_attempt_exists());
        }
        let currency = CurrencyCode::parse(&order.1)?;
        let attempt = PaymentAttempt::create(order_id, Money::new(order.0, currency))?;
        sqlx::query(
            "INSERT INTO commerce.payment_attempts \
             (id, store_id, order_id, shopper_id, provider_account_id, \
              amount_minor, currency) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(attempt.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(provider_account_id)
        .bind(attempt.amount().amount_minor())
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        append_event(
            &mut transaction,
            AnalyticsEventToAppend {
                store_id: actor.store_id.as_uuid(),
                shopper_id: shopper.shopper_id.as_uuid(),
                event_id: attempt.id().as_uuid(),
                event_name: "add_payment_info".into(),
                properties: json!({
                    "_source": "server",
                    "payment_attempt_id": attempt.id().as_uuid(),
                }),
                occurred_at: OffsetDateTime::now_utc(),
                received_at: OffsetDateTime::now_utc(),
            },
        )
        .await?;
        insert_outbox(
            &mut transaction,
            actor.store_id,
            "payment_attempt",
            attempt.id().as_uuid(),
            "payment.create_requested",
            attempt.amount().amount_minor(),
            currency,
            return_url,
        )
        .await?;
        let detail = load_attempt(
            &mut transaction,
            actor.store_id,
            Some(channel_id),
            Some(shopper.shopper_id.as_uuid()),
            attempt.id(),
        )
        .await?
        .ok_or_else(|| attempt_not_found(attempt.id()))?;
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ATTEMPT_OPERATION,
            request,
            201,
            attempt_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get_attempt(
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

    async fn create_refund(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        attempt_id: PaymentAttemptId,
        amount_minor: i64,
        request: &IdempotencyRequest,
    ) -> Result<RefundDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            CREATE_REFUND_OPERATION,
            request,
        )
        .await?
        {
            return replay_refund(snapshot);
        }
        let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>)>(
            "SELECT attempt.order_id, attempt.amount_minor, attempt.currency::text, \
                    attempt.status::text, attempt.stripe_checkout_session_id \
             FROM commerce.payment_attempts AS attempt \
             WHERE attempt.store_id = $1 \
               AND attempt.id = $2 FOR UPDATE OF attempt",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| attempt_not_found(attempt_id))?;
        let currency = CurrencyCode::parse(&row.2)?;
        let attempt = PaymentAttempt::rehydrate(
            attempt_id,
            OrderId::from_uuid(row.0),
            Money::new(row.1, currency),
            PaymentAttemptStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
            row.4,
        );
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM commerce.refunds \
             WHERE store_id = $1 AND payment_attempt_id = $2 \
               AND status IN ('pending', 'succeeded')",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refund = Refund::create(
            &attempt,
            Money::new(amount_minor, currency),
            already_refunded,
        )?;
        sqlx::query(
            "INSERT INTO commerce.refunds \
             (id, store_id, payment_attempt_id, amount_minor, currency) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(refund.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(amount_minor)
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let id = refund.id();
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
        let detail = load_refund(&mut transaction, store_id, id)
            .await?
            .ok_or_else(|| refund_not_found(id))?;
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            CREATE_REFUND_OPERATION,
            request,
            201,
            refund_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn ingest_webhook(&self, event: &StripeWebhookEvent) -> Result<bool, ApplicationError> {
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
            "INSERT INTO integration.provider_webhooks \
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

    async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let aggregate_id = job
            .payload
            .get("aggregate_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(corrupt_webhook_payload)?;
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
        if job.event_type.starts_with("payment.") {
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
            .await?;
        } else if job.event_type.starts_with("refund.") {
            apply_refund_event(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                RefundId::from_uuid(aggregate_id),
                &job.event_type,
                stripe_object_id,
                failure_code,
                now,
            )
            .await?;
        } else {
            return Err(corrupt_webhook_payload());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn prepare_stripe_command(
        &self,
        job: &QueueJob,
    ) -> Result<StripeCommand, ApplicationError> {
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let row = if job.event_type == "payment.create_requested" {
            sqlx::query_as::<_, (i64, String, Uuid, String, Option<String>, Uuid)>(
                "SELECT attempt.amount_minor, attempt.currency::text, \
                        account.id, account.credential_secret_reference, \
                        NULL::text, attempt.order_id \
                 FROM commerce.payment_attempts AS attempt \
                 INNER JOIN commerce.provider_accounts AS account \
                   ON account.store_id = attempt.store_id \
                  AND account.id = attempt.provider_account_id \
                 WHERE attempt.store_id = $1 \
                   AND attempt.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
        } else if job.event_type == "refund.create_requested" {
            sqlx::query_as::<_, (i64, String, Uuid, String, Option<String>, Uuid)>(
                "SELECT refund.amount_minor, refund.currency::text, \
                        account.id, account.credential_secret_reference, \
                        COALESCE(attempt.stripe_payment_intent_id, attempt.stripe_checkout_session_id), attempt.order_id \
                 FROM commerce.refunds AS refund \
                 INNER JOIN commerce.payment_attempts AS attempt \
                   ON attempt.store_id = refund.store_id \
                  AND attempt.id = refund.payment_attempt_id \
                 INNER JOIN commerce.provider_accounts AS account \
                   ON account.store_id = attempt.store_id \
                  AND account.id = attempt.provider_account_id \
                 WHERE refund.store_id = $1 \
                   AND refund.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
        } else {
            return Err(invalid_outbox_payload());
        }
        .ok_or_else(provider_unavailable)?;
        if row.0 != outbox_amount(job)? || row.1 != outbox_currency(job)? {
            return Err(invalid_outbox_payload());
        }
        if job.event_type == "refund.create_requested" && row.4.is_none() {
            return Err(ApplicationError::Conflict {
                code: "stripe_payment_reference_missing",
                message: "the captured Payment Attempt has no Stripe payment reference",
            });
        }
        let checkout_details = if job.event_type == "payment.create_requested" {
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
                "SELECT contact.email::text, contact.phone, shipping.full_name, \
                        shipping.address_line1, shipping.address_line2, shipping.locality, \
                        shipping.administrative_area, shipping.postal_code, \
                        NULLIF(btrim(shipping.country_code::text), '') \
                 FROM commerce.order_contacts AS contact \
                 LEFT JOIN commerce.order_addresses AS shipping \
                   ON shipping.store_id = contact.store_id \
                  AND shipping.order_id = contact.order_id \
                  AND shipping.kind = 'shipping' \
                 WHERE contact.store_id = $1 AND contact.order_id = $2",
            )
            .bind(job.store_id)
            .bind(row.5)
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
            .bind(row.5)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            let shippable = line_rows.iter().any(|line| line.5);
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
            let (shipping_countries, shipping_options) = if shippable {
                let countries = sqlx::query_scalar::<_, String>(
                    "SELECT DISTINCT region.country_code::text \
                     FROM commerce.shipping_service_regions AS region \
                     INNER JOIN commerce.shipping_services AS service \
                       ON service.store_id = region.store_id \
                      AND service.id = region.shipping_service_id \
                     WHERE service.store_id = $1 AND service.currency = $2 \
                       AND service.status = 'active' \
                     ORDER BY region.country_code::text",
                )
                .bind(job.store_id)
                .bind(&row.1)
                .fetch_all(&mut *transaction)
                .await
                .map_err(database_error)?;
                let services = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
                    "SELECT DISTINCT ON (service.id) service.id, service.code, service.name, \
                            service.amount_minor, service.currency::text, \
                     service.estimated_min_days, service.estimated_max_days \
                     FROM commerce.shipping_services AS service \
                     INNER JOIN commerce.shipping_service_regions AS region \
                       ON region.store_id = service.store_id \
                      AND region.shipping_service_id = service.id \
                     WHERE service.store_id = $1 AND service.currency = $2 \
                       AND service.status = 'active' \
                     ORDER BY service.id, service.amount_minor",
                )
                .bind(job.store_id)
                .bind(&row.1)
                .fetch_all(&mut *transaction)
                .await
                .map_err(database_error)?;
                let options = services
                    .into_iter()
                    .map(|service| {
                        Ok(PaymentShippingOption {
                            service_id: service.0,
                            code: service.1,
                            name: service.2,
                            amount_minor: service.3,
                            currency: CurrencyCode::parse(&service.4)?,
                            estimated_min_days: u16::try_from(service.5)
                                .map_err(unexpected_conversion)?,
                            estimated_max_days: u16::try_from(service.6)
                                .map_err(unexpected_conversion)?,
                        })
                    })
                    .collect::<Result<Vec<_>, ApplicationError>>()?;
                if countries.is_empty() || options.is_empty() {
                    return Err(checkout_configuration_unavailable());
                }
                (countries, options)
            } else {
                (Vec::new(), Vec::new())
            };
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
            amount_minor: row.0,
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

    async fn record_stripe_result(
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

    async fn fail_stripe_command(
        &self,
        job: &QueueJob,
        failure: &str,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if job.event_type != "payment.create_requested" {
            return Ok(());
        }
        let attempt_id = PaymentAttemptId::from_uuid(outbox_aggregate_id(job)?);
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>, String)>(
            "SELECT attempt.order_id, sales_order.checkout_id, \
                    sales_order.inventory_reservation_id, sales_order.status::text \
             FROM commerce.payment_attempts AS attempt \
             INNER JOIN commerce.orders AS sales_order \
               ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
             WHERE attempt.store_id = $1 AND attempt.id = $2 \
             FOR UPDATE OF attempt, sales_order",
        )
        .bind(job.store_id)
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| attempt_not_found(attempt_id))?;
        let failure_code = if failure.trim().is_empty() {
            "provider_command_failed"
        } else {
            failure.trim()
        };
        sqlx::query(
            "UPDATE commerce.payment_attempts \
             SET status = 'failed', failure_code = left($3, 2000), updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(job.store_id)
        .bind(attempt_id.as_uuid())
        .bind(failure_code)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        cancel_pending_order(
            &mut transaction,
            StoreId::from_uuid(job.store_id),
            OrderId::from_uuid(row.0),
            CheckoutId::from_uuid(row.1),
            row.2.map(InventoryReservationId::from_uuid),
            &row.3,
            now,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn client_action_command(
        &self,
        shopper: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<StripeClientActionCommand>, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT account.id, attempt.stripe_checkout_session_id, \
                    account.credential_secret_reference \
             FROM commerce.payment_attempts AS attempt \
             INNER JOIN commerce.provider_accounts AS account \
               ON account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
             INNER JOIN commerce.orders AS sales_order \
               ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
             WHERE attempt.store_id = $1 \
               AND attempt.id = $2 AND attempt.shopper_id = $3 \
               AND sales_order.sales_channel_id = $4 AND attempt.status IN ('pending', 'authorized') \
               AND attempt.stripe_checkout_session_id IS NOT NULL \
               AND account.credential_secret_reference IS NOT NULL",
        )
        .bind(actor.store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(|row| {
            Ok(StripeClientActionCommand {
                stripe_account_id: StripeAccountId::from_uuid(row.0),
                stripe_checkout_session_id: row.1,
                credential_secret_reference: PaymentSecretReference::new(
                    "credential_secret_reference",
                    row.2,
                )?,
            })
        })
        .transpose()
    }
}
