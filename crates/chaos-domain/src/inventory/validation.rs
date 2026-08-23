// Inventory quantity validation and domain errors.

fn quantity_overflow() -> DomainError {
    validation("quantity", "inventory arithmetic overflowed")
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}
