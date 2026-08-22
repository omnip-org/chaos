// Inventory context checks, parsing, validation, and shared database errors.

fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("OffsetDateTime must format as RFC 3339")
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid inventory idempotency snapshot"))
}

fn invalid_inventory_selection() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "inventory",
            reason: "location, variant, and inventory item must belong to the Store".into(),
        }],
    }
}

fn reservation_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "inventory_reservation_not_active",
        message: "the inventory reservation is no longer active",
    }
}

fn map_location_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("inventory_locations_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "inventory_location_code_taken",
            message: "the inventory location code is already in use for this Store",
        };
    }
    database_error(error)
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
