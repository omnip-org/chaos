// Fulfillment and shipping service persistence commands.

#[async_trait]
impl FulfillmentRepository for PostgresFulfillmentRepository {
    async fn get_return(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        return_id: ReturnId,
    ) -> Result<Option<ReturnDetail>, ApplicationError> {
        let account_id = actor.store_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        let detail = load_return(&mut transaction, account_id, store_id, return_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_fulfillment(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        allocations: Vec<FulfillmentAllocationInput>,
        request: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let account_id = actor.store_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if let Some(value) = reserve(
            &mut transaction,
            account_id,
            "fulfillments.create.v1",
            request,
        )
        .await?
        {
            return replay_fulfillment(value);
        }
        lock_confirmed_order(&mut transaction, account_id, store_id, order_id).await?;
        let fulfillment = Fulfillment::create(
            order_id,
            allocations
                .into_iter()
                .map(|line| FulfillmentAllocation {
                    product_variant_id: line.product_variant_id,
                    quantity: line.quantity,
                })
                .collect(),
        )?;
        validate_fulfillment_quantities(
            &mut transaction,
            account_id,
            store_id,
            order_id,
            fulfillment.allocations(),
        )
        .await?;
        sqlx::query(
            "INSERT INTO commerce.fulfillments \
             (id, store_id, order_id) VALUES ($1, $2, $3)",
        )
        .bind(fulfillment.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        for line in fulfillment.allocations() {
            sqlx::query(
                "INSERT INTO commerce.fulfillment_lines \
                 (store_id, fulfillment_id, product_variant_id, quantity) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(store_id.as_uuid())
            .bind(fulfillment.id().as_uuid())
            .bind(line.product_variant_id.as_uuid())
            .bind(i32::try_from(line.quantity).map_err(unexpected_conversion)?)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = load_fulfillment(&mut transaction, account_id, store_id, fulfillment.id())
            .await?
            .ok_or_else(|| fulfillment_not_found(fulfillment.id()))?;
        complete(
            &mut transaction,
            account_id,
            "fulfillments.create.v1",
            request,
            fulfillment_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_fulfillment(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        target_status: FulfillmentStatus,
        carrier: Option<&str>,
        tracking_number: Option<&str>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let account_id = actor.store_id().as_uuid();
        let operation = match target_status {
            FulfillmentStatus::Shipped => "fulfillments.ship.v1",
            FulfillmentStatus::Delivered => "fulfillments.deliver.v1",
            FulfillmentStatus::Cancelled => "fulfillments.cancel.v1",
            FulfillmentStatus::Pending => return Err(invalid_target()),
        };
        let mut transaction = self.begin(&actor).await?;
        if let Some(value) = reserve(&mut transaction, account_id, operation, request).await? {
            return replay_fulfillment(value);
        }
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT order_id, status::text FROM commerce.fulfillments \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| fulfillment_not_found(fulfillment_id))?;
        let allocations =
            load_allocations(&mut transaction, account_id, store_id, fulfillment_id).await?;
        let mut fulfillment = Fulfillment::rehydrate(
            fulfillment_id,
            OrderId::from_uuid(row.0),
            FulfillmentStatus::parse(&row.1).ok_or_else(corrupt_state)?,
            allocations,
        );
        match target_status {
            FulfillmentStatus::Shipped => {
                fulfillment.ship()?;
                require_tracking(carrier, tracking_number)?;
            }
            FulfillmentStatus::Delivered => fulfillment.deliver()?,
            FulfillmentStatus::Cancelled => fulfillment.cancel()?,
            FulfillmentStatus::Pending => return Err(invalid_target()),
        }
        sqlx::query(
            "UPDATE commerce.fulfillments SET status = $3::commerce.fulfillment_status, \
                    carrier = COALESCE($4, carrier), tracking_number = COALESCE($5, tracking_number), \
                    shipped_at = CASE WHEN $3 = 'shipped' THEN $6 ELSE shipped_at END, \
                    delivered_at = CASE WHEN $3 = 'delivered' THEN $6 ELSE delivered_at END, \
                    cancelled_at = CASE WHEN $3 = 'cancelled' THEN $6 ELSE cancelled_at END, \
                    updated_at = $6 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .bind(target_status.as_str())
        .bind(carrier)
        .bind(tracking_number)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO integration.event_outbox \
             (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, 'fulfillment', $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .bind(format!("fulfillment.{}", target_status.as_str()))
        .bind(serde_json::json!({
            "fulfillment_id": fulfillment_id.as_uuid(),
            "order_id": fulfillment.order_id().as_uuid(),
            "status": target_status.as_str(),
        }))
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_fulfillment(&mut transaction, account_id, store_id, fulfillment_id)
            .await?
            .ok_or_else(|| fulfillment_not_found(fulfillment_id))?;
        complete(
            &mut transaction,
            account_id,
            operation,
            request,
            fulfillment_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_return(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        lines: Vec<ReturnLineInput>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError> {
        let account_id = actor.store_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if let Some(value) =
            reserve(&mut transaction, account_id, "returns.create.v1", request).await?
        {
            return replay_return(value);
        }
        lock_confirmed_order(&mut transaction, account_id, store_id, order_id).await?;
        validate_return_lines(&lines)?;
        validate_return_quantities(&mut transaction, account_id, store_id, order_id, &lines)
            .await?;
        let (currency, refund_lines, refund_amount_minor) =
            allocate_return_refund(&mut transaction, account_id, store_id, order_id, &lines)
                .await?;
        let returned = Return::create(order_id);
        sqlx::query(
            "INSERT INTO commerce.returns \
             (id, store_id, order_id, refund_amount_minor, currency, \
              requested_at) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(returned.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(refund_amount_minor)
        .bind(currency.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        for line in &refund_lines {
            sqlx::query(
                "INSERT INTO commerce.return_lines \
                 (store_id, return_id, product_variant_id, quantity, \
                  refund_amount_minor) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(store_id.as_uuid())
            .bind(returned.id().as_uuid())
            .bind(line.product_variant_id.as_uuid())
            .bind(i32::try_from(line.quantity).map_err(unexpected_conversion)?)
            .bind(line.refund_amount_minor)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = load_return(&mut transaction, account_id, store_id, returned.id())
            .await?
            .ok_or_else(|| return_not_found(returned.id()))?;
        complete(
            &mut transaction,
            account_id,
            "returns.create.v1",
            request,
            return_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_return(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        return_id: ReturnId,
        target_status: ReturnStatus,
        receipt: Vec<ReturnReceiptInput>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError> {
        let account_id = actor.store_id().as_uuid();
        let audit_user_id = actor.audit_user_id().as_uuid();
        let operation = match target_status {
            ReturnStatus::Authorized => "returns.authorize.v1",
            ReturnStatus::Rejected => "returns.reject.v1",
            ReturnStatus::Received => "returns.receive.v1",
            ReturnStatus::Completed => "returns.complete.v1",
            ReturnStatus::Requested => return Err(invalid_target()),
        };
        let mut transaction = self.begin(&actor).await?;
        if let Some(value) = reserve(&mut transaction, account_id, operation, request).await? {
            return replay_return(value);
        }
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT order_id, status::text FROM commerce.returns \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| return_not_found(return_id))?;
        let mut returned = Return::rehydrate(
            return_id,
            OrderId::from_uuid(row.0),
            ReturnStatus::parse(&row.1).ok_or_else(corrupt_state)?,
        );
        match target_status {
            ReturnStatus::Authorized => returned.authorize()?,
            ReturnStatus::Rejected => returned.reject()?,
            ReturnStatus::Received => {
                returned.receive()?;
                receive_return(
                    &mut transaction,
                    account_id,
                    store_id,
                    return_id,
                    &receipt,
                    audit_user_id,
                    now,
                )
                .await?;
            }
            ReturnStatus::Completed => {
                returned.complete()?;
                insert_return_outbox(
                    &mut transaction,
                    account_id,
                    store_id,
                    return_id,
                    returned.order_id(),
                )
                .await?;
            }
            ReturnStatus::Requested => return Err(invalid_target()),
        }
        sqlx::query(
            "UPDATE commerce.returns SET status = $3::commerce.return_status, \
                    authorized_at = CASE WHEN $3 = 'authorized' THEN $4 ELSE authorized_at END, \
                    received_at = CASE WHEN $3 = 'received' THEN $4 ELSE received_at END, \
                    completed_at = CASE WHEN $3 = 'completed' THEN $4 ELSE completed_at END, \
                    rejected_at = CASE WHEN $3 = 'rejected' THEN $4 ELSE rejected_at END, \
                    updated_at = $4 WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .bind(target_status.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_return(&mut transaction, account_id, store_id, return_id)
            .await?
            .ok_or_else(|| return_not_found(return_id))?;
        complete(
            &mut transaction,
            account_id,
            operation,
            request,
            return_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}
