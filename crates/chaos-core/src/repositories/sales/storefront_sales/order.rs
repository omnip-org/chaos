pub(super) async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    crate::repositories::sales::order_detail::load(
        transaction,
        actor.store_id,
        actor.sales_channel_id,
        order_id,
    )
    .await
}
