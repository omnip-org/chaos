# PostgreSQL Extensions

The development PostgreSQL image is based on PostgreSQL 18.4 and installs extension packages from the PGDG and Pigsty repositories.

## Installed packages

| Extension | Package | Intended capability |
|---|---|---|
| `pg_cron` | `postgresql-18-cron` | Database-local scheduling for maintenance tasks |
| `pgmq` | `postgresql-18-pgmq` | Durable PostgreSQL-backed message queues |
| `pg_partman` | `postgresql-18-partman` | Lifecycle management for declarative partitions |

The initial schema migration activates all three extensions. Their objects remain isolated in extension-owned schemas and are not granted to the application runtime role by default.

## Activation requirements

- `pg_cron` is preloaded for the `chaos` database and uses PostgreSQL background workers. Scheduling permissions require a dedicated migration and a reviewed job-execution role.
- `pgmq` does not require a background worker. Integration migrations create the queues listed in ADR 0029 and remove queues for retired capabilities. Runtime roles do not receive direct access to the `pgmq` schema; narrowly granted `integration` routines claim and finish messages while updating authoritative records in the same transaction.
- `pg_partman` builds on native declarative partitioning and is installed in the dedicated `partman` schema. Partition ownership and maintenance permissions require a dedicated role before the first managed partition set is created.

Extensions must not bypass bounded-context ownership. Business code accesses extension APIs through application ports, and extension-owned schemas are never used as substitutes for Store isolation.

## Versioning and production

The PostgreSQL base image, Pigsty signing-key checksum, and extension package versions are pinned in the Dockerfile. Before the first production release, pin the base image by digest and publish the verified image to an internal registry.

Extension upgrades require their own migration and rollback review. Never assume that replacing the container image also upgrades extension objects inside an existing database.

Verify installed extensions with:

```sql
SELECT name, default_version, installed_version
FROM pg_catalog.pg_available_extensions
WHERE name IN ('pg_cron', 'pgmq', 'pg_partman')
ORDER BY name;
```
