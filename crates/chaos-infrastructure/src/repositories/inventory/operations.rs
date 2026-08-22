// Inventory location management, on-hand adjustments, and balance listing.

#[async_trait]
impl InventoryRepository for PostgresInventoryRepository {
    async fn create_location(
        &self,
        actor: AdminActor,
        location: &InventoryLocation,
        request: &IdempotencyRequest,
    ) -> Result<InventoryLocationId, ApplicationError> {
        let store_id = actor.store_id();
        let mut transaction = self.begin_for_admin(&actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            store_id.as_uuid(),
            CREATE_LOCATION_OPERATION,
            request,
        )
        .await?
        {
            return replay_id(&snapshot).map(InventoryLocationId::from_uuid);
        }
        require_store(&mut transaction, location.store_id()).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_locations \
             (id, store_id, code, name) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(location.id().as_uuid())
        .bind(location.store_id().as_uuid())
        .bind(location.code().as_str())
        .bind(location.name())
        .execute(&mut *transaction)
        .await
        .map_err(map_location_write_error)?;
        complete_id(
            &mut transaction,
            store_id.as_uuid(),
            CREATE_LOCATION_OPERATION,
            request,
            location.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(location.id())
    }

    async fn list_locations(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryLocationId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryLocationItem>>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, LocationRow>(
            "SELECT id, code::text, name, archived_at, created_at, updated_at \
             FROM commerce.inventory_locations \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(InventoryLocationId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(location_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn adjust_inventory_item(
        &self,
        actor: AdminActor,
        adjustment: &InventoryAdjustment,
        request: &IdempotencyRequest,
    ) -> Result<InventoryItemView, ApplicationError> {
        let audit_user_id = actor.audit_user_id();
        let mut transaction = self.begin_for_admin(&actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            adjustment.store_id.as_uuid(),
            ADJUST_INVENTORY_OPERATION,
            request,
        )
        .await?
        {
            return replay_inventory_item(&snapshot);
        }
        require_store(&mut transaction, adjustment.store_id).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_items \
             (id, store_id, inventory_location_id, product_variant_id) \
             SELECT $1, $2, location.id, variant.id \
             FROM commerce.inventory_locations AS location \
             INNER JOIN commerce.product_variants AS variant \
               ON variant.store_id = location.store_id \
              AND variant.id = $4 AND variant.track_inventory \
             WHERE location.store_id = $2 \
               AND location.id = $3 AND location.archived_at IS NULL \
             ON CONFLICT (store_id, inventory_location_id, product_variant_id) \
             DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(adjustment.store_id.as_uuid())
        .bind(adjustment.inventory_location_id.as_uuid())
        .bind(adjustment.product_variant_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let locked = lock_inventory_item_by_location_variant(
            &mut transaction,
            adjustment.store_id,
            adjustment.inventory_location_id,
            adjustment.product_variant_id,
        )
        .await?
        .ok_or_else(invalid_inventory_selection)?;
        let balance = InventoryBalance::new(locked.1, locked.2)?.adjust(adjustment.delta_quantity)?;
        let updated_at = update_inventory_balance(&mut transaction, locked.0, balance).await?;
        insert_inventory_transaction(
            &mut transaction,
            adjustment.store_id,
            locked.0,
            None,
            None,
            adjustment.delta_quantity,
            0,
            balance,
            Some(&adjustment.note),
            Some(audit_user_id.as_uuid()),
        )
        .await?;
        let item = InventoryItemView {
            id: InventoryItemId::from_uuid(locked.0),
            inventory_location_id: adjustment.inventory_location_id,
            product_variant_id: adjustment.product_variant_id,
            on_hand_quantity: balance.on_hand(),
            reserved_quantity: balance.reserved(),
            available_quantity: balance.available(),
            updated_at,
        };
        complete_snapshot(
            &mut transaction,
            adjustment.store_id.as_uuid(),
            ADJUST_INVENTORY_OPERATION,
            request,
            inventory_snapshot(&item),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn list_inventory_items(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryItemId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryItemView>>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, InventoryItemRow>(
            "SELECT id, inventory_location_id, product_variant_id, on_hand_quantity, \
                    reserved_quantity, updated_at FROM commerce.inventory_items \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(InventoryItemId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(rows.into_iter().map(inventory_item).collect()))
    }

    async fn create_reservation(
        &self,
        actor: &MachineActor,
        reservation: &InventoryReservation,
        request: &IdempotencyRequest,
    ) -> Result<InventoryReservationId, ApplicationError> {
        let channel_id = actor
            .sales_channel_id
            .ok_or_else(invalid_inventory_selection)?;
        let mut transaction = self.begin_for_machine(actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            actor.store_id.as_uuid(),
            CREATE_RESERVATION_OPERATION,
            request,
        )
        .await?
        {
            return replay_id(&snapshot).map(InventoryReservationId::from_uuid);
        }
        require_active_machine_context(&mut transaction, actor, channel_id.as_uuid()).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_reservations \
             (id, store_id, sales_channel_id, expires_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(reservation.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(reservation.expires_at())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        let mut lines = reservation.lines().iter().collect::<Vec<_>>();
        lines.sort_by_key(|line| line.inventory_item_id());
        for line in lines {
            let locked = lock_inventory_item_by_id(
                &mut transaction,
                actor.store_id,
                line.inventory_item_id(),
            )
            .await?
            .ok_or_else(invalid_inventory_selection)?;
            let balance = InventoryBalance::new(locked.2, locked.3)?.reserve(line.quantity())?;
            update_inventory_balance(&mut transaction, locked.0, balance).await?;
            sqlx::query(
                "INSERT INTO commerce.inventory_reservation_lines \
                 (store_id, reservation_id, inventory_item_id, quantity) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(reservation.id().as_uuid())
            .bind(locked.0)
            .bind(line.quantity())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            insert_inventory_transaction(
                &mut transaction,
                actor.store_id,
                locked.0,
                Some("reservation"),
                Some(reservation.id().as_uuid()),
                0,
                line.quantity(),
                balance,
                None,
                None,
            )
            .await?;
        }
        complete_id(
            &mut transaction,
            actor.store_id.as_uuid(),
            CREATE_RESERVATION_OPERATION,
            request,
            reservation.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(reservation.id())
    }

    async fn transition_reservation(
        &self,
        actor: &MachineActor,
        reservation_id: InventoryReservationId,
        transition: InventoryReservationTransition,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<InventoryReservationDetail, ApplicationError> {
        let operation = match transition {
            InventoryReservationTransition::Release => RELEASE_RESERVATION_OPERATION,
            InventoryReservationTransition::Consume => CONSUME_RESERVATION_OPERATION,
        };
        let mut transaction = self.begin_for_machine(actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            actor.store_id.as_uuid(),
            operation,
            request,
        )
        .await?
        {
            return replay_reservation(&snapshot);
        }
        let row = lock_reservation(&mut transaction, actor, reservation_id).await?;
        if row.0 != "active" {
            return Err(reservation_not_active());
        }
        let effective_transition = if now >= row.1 {
            ReservationClosure::Expired
        } else {
            match transition {
                InventoryReservationTransition::Release => ReservationClosure::Released,
                InventoryReservationTransition::Consume => ReservationClosure::Consumed,
            }
        };
        close_reservation(
            &mut transaction,
            actor.store_id,
            reservation_id,
            effective_transition,
            now,
        )
        .await?;
        let detail = InventoryReservationDetail {
            id: reservation_id,
            status: effective_transition.status(),
            expires_at: row.1,
            closed_at: Some(now),
        };
        complete_snapshot(
            &mut transaction,
            actor.store_id.as_uuid(),
            operation,
            request,
            reservation_snapshot(&detail),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn expire_due_reservations(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<u16, ApplicationError> {
        let mut transaction = self.begin_for_store_actor(actor).await?;
        require_store(&mut transaction, store_id).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.inventory_reservations \
             WHERE store_id = $1 \
               AND status = 'active' AND expires_at <= $2 \
             ORDER BY expires_at ASC, id ASC FOR UPDATE SKIP LOCKED LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        for id in &ids {
            close_reservation(
                &mut transaction,
                store_id,
                InventoryReservationId::from_uuid(*id),
                ReservationClosure::Expired,
                now,
            )
            .await?;
        }
        transaction.commit().await.map_err(database_error)?;
        u16::try_from(ids.len()).map_err(|error| ApplicationError::Unexpected(error.into()))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReservationClosure {
    Released,
    Consumed,
    Expired,
}
