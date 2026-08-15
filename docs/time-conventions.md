# Time Conventions

Chaos represents persisted instants with PostgreSQL `TIMESTAMPTZ` and Rust `time::OffsetDateTime`.

- Store absolute instants as `TIMESTAMPTZ`; do not use `TIMESTAMP` for business events.
- Let PostgreSQL generate persistence metadata such as `created_at` with `CURRENT_TIMESTAMP`.
- Pass business decision times explicitly into domain and application operations.
- Obtain production wall-clock time only through the application `Clock` port.
- Truncate production instants to PostgreSQL's microsecond precision before they enter application workflows.
- Serialize public API timestamps as RFC 3339 through the shared HTTP time type.
- Keep durations monotonic with `std::time::Instant` when measuring elapsed process time.
- Do not use local system time for persistence, comparisons, retries, expiry, or signatures.
- Introduce calendar or named-time-zone types only for explicit Store-local business rules.

These boundaries isolate the date-time library from HTTP contracts, clock acquisition, and database storage. Replacing the Rust date-time implementation should therefore be a boundary migration rather than an API or schema change.
