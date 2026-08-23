use time::OffsetDateTime;

/// Supplies UTC wall-clock time at application boundaries.
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}
