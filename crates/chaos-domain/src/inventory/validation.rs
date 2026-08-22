// Inventory quantity validation and domain errors.

fn require_positive_quantity(quantity: i64) -> Result<(), DomainError> {
    if quantity > 0 {
        return Ok(());
    }
    Err(validation("quantity", "must be greater than zero"))
}

fn quantity_overflow() -> DomainError {
    validation("quantity", "inventory arithmetic overflowed")
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}
