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
        let detail = load_attempt(
            &mut transaction,
            actor.store_id,
            Some(channel_id),
            Some(shopper.shopper_id.as_uuid()),
            order_id,
        )
        .await?
        .ok_or_else(|| order_not_found(order_id))?;
        if detail.status != PaymentAttemptStatus::Pending {
            return Err(payment_order_not_pending());
        }
        Ok(detail)
    }

    pub(crate) async fn prepare_checkout_command(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
        provider: &str,
        return_url: &str,
        idempotency_key: &str,
    ) -> Result<PaymentCommand, ApplicationError> {
        let attempt = self
            .get_attempt(actor, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        let job = direct_checkout_job(actor, &attempt, provider, return_url);
        let mut command = self.prepare_payment_command(&job).await?;
        command.idempotency_key = idempotency_key.to_owned();
        Ok(command)
    }

    pub(crate) async fn record_checkout_result(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
        result: &PaymentCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let attempt = self
            .get_attempt(actor, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        let job = direct_checkout_job(actor, &attempt, "stripe", "");
        self.record_payment_result(&job, result, now).await
    }

    pub(crate) async fn get_attempt(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        let detail = load_attempt(
            &mut transaction,
            actor.store_id,
            actor.sales_channel_id,
            Some(shopper.shopper_id.as_uuid()),
            order_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn create_refund(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        amount_minor: i64,
    ) -> Result<RefundDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let row = sqlx::query_as::<_, (i64, String, String, Uuid)>(
            "SELECT total_amount_minor, currency::text, payment_status::text, \
                    payment_provider_account_id \
             FROM commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        let currency = CurrencyCode::parse(&row.1)?;
        // The captured amount available to refund against is the Order's
        // total — only an Order that has been paid (in full, or already
        // partially refunded) is eligible for a further refund.
        let payment_status = match row.2.as_str() {
            "paid" | "partially_refunded" => PaymentAttemptStatus::Captured,
            _ => PaymentAttemptStatus::Failed,
        };
        // Pending refunds already claim their share of the captured amount,
        // so a second concurrent request cannot double-spend it before the
        // first one confirms via webhook.
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM commerce.refunds \
             WHERE store_id = $1 AND order_id = $2 AND status IN ('pending', 'succeeded')",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refund = Refund::create(
            order_id,
            payment_status,
            Money::new(row.0, currency),
            Money::new(amount_minor, currency),
            already_refunded,
        )?;
        let id = refund.id();
        sqlx::query(
            "INSERT INTO commerce.refunds \
             (id, store_id, order_id, currency, status, amount_minor, \
              payment_provider_account_id) \
             VALUES ($1, $2, $3, $4, 'pending', $5, $6)",
        )
        .bind(id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(currency.as_str())
        .bind(amount_minor)
        .bind(row.3)
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
            order_id,
            amount_minor,
            currency,
            status: RefundStatus::Pending,
            provider_reference_id: None,
            failure_code: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let failure_code = job
            .payload
            .get("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let normalized_event_type = job
            .normalized_event_type
            .as_deref()
            .ok_or_else(corrupt_webhook_payload)?;
        let provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT provider_account_id FROM integration.provider_webhook_inbox WHERE id = $1",
        )
        .bind(job.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let resolved_order_id = if normalized_event_type.starts_with("payment.") {
            let order_id = job
                .payload
                .get("order_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(OrderId::from_uuid)
                .ok_or_else(corrupt_webhook_payload)?;
            Some(apply_payment_event(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                order_id,
                provider_account_id,
                normalized_event_type,
                failure_code,
                &job.payload,
                now,
            )
            .await?)
        } else if normalized_event_type.starts_with("refund.") {
            let stripe_object_id = job
                .payload
                .get("object")
                .and_then(Value::as_str)
                .ok_or_else(corrupt_webhook_payload)?
                .to_owned();
            let refund_id = job
                .payload
                .get("refund_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(RefundId::from_uuid);
            Some(apply_refund_event(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                refund_id,
                provider_account_id,
                normalized_event_type,
                stripe_object_id,
                failure_code,
                &job.payload,
                now,
            )
            .await?)
        } else {
            return Err(corrupt_webhook_payload());
        };
        if let Some(order_id) = resolved_order_id {
            let updated: bool = sqlx::query_scalar(
                "SELECT integration.set_provider_webhook_aggregate($1, 'order', $2)",
            )
            .bind(job.id)
            .bind(order_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            if !updated {
                return Err(corrupt_webhook_payload());
            }
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn prepare_payment_command(
        &self,
        job: &QueueJob,
    ) -> Result<PaymentCommand, ApplicationError> {
        let provider = job.provider.as_deref().unwrap_or("stripe");
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let internal_event_type = job
            .internal_event_type
            .as_deref()
            .ok_or_else(invalid_outbox_payload)?;
        if !matches!(
            internal_event_type,
            "payment.checkout_session" | "refund.create_requested"
        ) {
            return Err(invalid_outbox_payload());
        }
        let is_refund = internal_event_type == "refund.create_requested";
        type ContextRow = (
            i64,
            String,
            Uuid,
            String,
            Option<String>,
            Uuid,
            Uuid,
            String,
            Uuid,
            Option<Uuid>,
        );
        let row: ContextRow = if is_refund {
            sqlx::query_as(
                "SELECT refund.amount_minor, refund.currency::text, \
                        account.id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id, \
                        sales_order.shopper_id, sales_order.sales_channel_id, \
                        sales_order.order_number, sales_order.id, refund.id \
                 FROM commerce.refunds AS refund \
                 INNER JOIN commerce.orders AS sales_order \
                   ON sales_order.store_id = refund.store_id AND sales_order.id = refund.order_id \
                 INNER JOIN integration.provider_accounts AS account \
                   ON account.store_id = refund.store_id \
                  AND account.id = refund.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE refund.store_id = $1 AND refund.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        } else {
            sqlx::query_as(
                "SELECT sales_order.subtotal_amount_minor, sales_order.currency::text, \
                        account.id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id, \
                        sales_order.shopper_id, sales_order.sales_channel_id, \
                        sales_order.order_number, sales_order.id, NULL::uuid \
                 FROM commerce.orders AS sales_order \
                 INNER JOIN integration.provider_accounts AS account \
                   ON account.store_id = sales_order.store_id \
                  AND account.id = sales_order.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE sales_order.store_id = $1 AND sales_order.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        };
        let command_amount = row.0;
        if !is_refund && (command_amount != outbox_amount(job)? || row.1 != outbox_currency(job)?) {
            return Err(invalid_outbox_payload());
        }
        if is_refund && command_amount != outbox_amount(job)? {
            return Err(invalid_outbox_payload());
        }
        if is_refund && row.4.is_none() {
            return Err(ApplicationError::Conflict {
                code: "payment_provider_reference_missing",
                message: "the Order has no payment provider reference",
            });
        }
        let checkout_details = if !is_refund {
            let order_id = aggregate_id;
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
            let line_rows = sqlx::query_as::<_, (String, String, Option<String>, i32, i64)>(
                "SELECT product_title, variant_title, sku, quantity, \
                        unit_price_amount_minor \
                 FROM commerce.order_lines WHERE store_id = $1 AND order_id = $2 \
                 ORDER BY position",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
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
            // uses it for tax. Every Order ships, so this always runs.
            let shipping_countries = sqlx::query_scalar::<_, String>(
                "SELECT country_code::text FROM commerce.store_shipping_countries \
                 WHERE store_id = $1 AND enabled ORDER BY country_code",
            )
            .bind(job.store_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
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
        Ok(PaymentCommand {
            provider_account_id: row.2,
            kind: if is_refund {
                PaymentCommandKind::CreateRefund
            } else {
                PaymentCommandKind::CreateCheckoutSession
            },
            aggregate_id: row.8,
            refund_id: row.9,
            amount_minor: command_amount,
            currency: CurrencyCode::parse(&row.1)?,
            idempotency_key: job.id.to_string(),
            credential_secret_reference: row.3,
            provider_payment_reference: row.4,
            checkout_details,
            return_url,
            order_context: OrderMetadataContext {
                store_id: job.store_id,
                shopper_id: row.5,
                sales_channel_id: row.6,
                order_number: row.7,
            },
        })
    }

    pub(crate) async fn record_payment_result(
        &self,
        job: &QueueJob,
        result: &PaymentCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_object_id.trim().is_empty()
            || result.provider_object_id.chars().count() > 255
        {
            return Err(stripe_invalid_response());
        }
        let aggregate_id = outbox_aggregate_id(job)?;
        let internal_event_type = job
            .internal_event_type
            .as_deref()
            .ok_or_else(invalid_outbox_payload)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let rows = if internal_event_type == "payment.checkout_session" {
            // The Checkout Session id itself is not persisted — only the
            // PaymentIntent id (set later, once Stripe reports it via
            // webhook) is kept for refunds/lookups — so this call only
            // needs to confirm the Order still exists to create against.
            let order = sqlx::query_as::<_, (Uuid, i64, String)>(
                "SELECT shopper_id, total_amount_minor, currency::text \
                 FROM commerce.orders \
                 WHERE store_id = $1 AND id = $2",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
            let Some((shopper_id, amount_minor, currency)) = order else {
                return Err(stripe_object_mismatch());
            };
            append_event(
                &mut transaction,
                AnalyticsEventToAppend {
                    store_id: job.store_id,
                    shopper_id,
                    event_id: aggregate_id,
                    event_name: "add_payment_info".into(),
                    properties: json!({
                        "_source": "server",
                        "order_id": aggregate_id,
                        "value_minor": amount_minor,
                        "currency": currency,
                        "provider": job.provider.as_deref().unwrap_or("stripe"),
                    }),
                    occurred_at: now,
                    received_at: now,
                },
            )
            .await?;
            1
        } else if internal_event_type == "refund.create_requested" {
            sqlx::query(
                "UPDATE commerce.refunds \
                 SET payment_provider_reference_id = COALESCE(payment_provider_reference_id, $3), \
                     updated_at = CASE WHEN payment_provider_reference_id IS NULL THEN $4 ELSE updated_at END \
                 WHERE store_id = $1 AND id = $2 \
                   AND (payment_provider_reference_id IS NULL OR payment_provider_reference_id = $3)",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.provider_object_id)
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
    provider: &str,
    return_url: &str,
) -> QueueJob {
    QueueJob {
        id: attempt.order_id.as_uuid(),
        store_id: actor.machine.store_id.as_uuid(),
        queue_name: "chaos_payment_commands".into(),
        internal_event_type: Some("payment.checkout_session".into()),
        provider_event_type: None,
        normalized_event_type: None,
        payload: json!({
            "aggregate_id": attempt.order_id.as_uuid(),
            "amount_minor": attempt.amount_minor,
            "currency": attempt.currency.as_str(),
            "return_url": return_url,
        }),
        attempts: 1,
        provider_account_id: None,
        capability: Some("payment".into()),
        provider: Some(provider.to_owned()),
    }
}
