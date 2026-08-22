// Inventory domain invariant tests.

use time::Duration;

use super::*;

#[test]
fn inventory_balance_prevents_overselling_and_invalid_adjustments() {
    let balance = InventoryBalance::new(10, 3).unwrap();
    assert_eq!(balance.available(), 7);
    assert_eq!(balance.reserve(7).unwrap().reserved(), 10);
    assert!(balance.reserve(8).is_err());
    assert!(balance.adjust(-8).is_err());
    assert!(balance.adjust(0).is_err());
}

#[test]
fn reservation_release_and_consumption_update_balances() {
    let reserved = InventoryBalance::new(10, 0).unwrap().reserve(4).unwrap();
    assert_eq!(
        reserved.release(4).unwrap(),
        InventoryBalance::new(10, 0).unwrap()
    );
    assert_eq!(
        reserved.consume(4).unwrap(),
        InventoryBalance::new(6, 0).unwrap()
    );
    assert!(reserved.consume(5).is_err());
}

#[test]
fn reservation_state_transitions_are_time_aware_and_terminal() {
    let now = OffsetDateTime::UNIX_EPOCH;
    let line = InventoryReservationLine::new(InventoryItemId::new(), 1).unwrap();
    let mut reservation =
        InventoryReservation::create(StoreId::new(), now, now + Duration::minutes(10), vec![line])
            .unwrap();

    assert!(reservation.expire(now + Duration::minutes(9)).is_err());
    reservation.expire(now + Duration::minutes(10)).unwrap();
    assert_eq!(reservation.status(), InventoryReservationStatus::Expired);
    assert!(reservation.release().is_err());
    assert!(reservation.consume(now).is_err());
}

#[test]
fn reservation_rejects_duplicate_inventory_items() {
    let inventory_item_id = InventoryItemId::new();
    let line = InventoryReservationLine::new(inventory_item_id, 1).unwrap();
    assert!(
        InventoryReservation::create(
            StoreId::new(),
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + Duration::minutes(1),
            vec![line.clone(), line],
        )
        .is_err()
    );
}
