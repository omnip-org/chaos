# Analytics Operations

`integration.commerce_events` is an append-only analysis ledger partitioned by
daily `received_at` ranges. Migration `0008` registers it with `pg_partman`
with seven premade partitions and schedules the following `pg_cron` job:

```sql
SELECT partman.run_maintenance();
```

The partition set has no retention value. The application Worker does not
delete Analytics events, and `analytics_settings` has no retention setting.
Check the partition set and its default partition with:

```sql
SELECT *
FROM partman.show_partitions('integration.commerce_events', p_include_default := true);

SELECT *
FROM partman.check_default();
```

When old data must be removed, use a maintenance window and choose an
explicit interval. Provider delivery task rows must be deleted before the event
partitions so the deliberately decoupled delivery state does not retain
orphaned rows:

```sql
BEGIN;
LOCK TABLE integration.commerce_events IN ACCESS EXCLUSIVE MODE;

-- Replace the cutoff with the same timestamp used below.
DELETE FROM integration.analytics_event_deliveries AS delivery
USING integration.commerce_events AS event
WHERE delivery.store_id = event.store_id
  AND delivery.commerce_event_id = event.id
  AND event.received_at < TIMESTAMPTZ '2026-01-01 00:00:00+00';

-- The same cutoff is expressed as a retention interval for pg_partman.
SELECT partman.drop_partition_time(
    p_parent_table := 'integration.commerce_events',
    p_retention := INTERVAL '233 days',
    p_keep_table := false,
    p_reference_timestamp := TIMESTAMPTZ '2026-08-22 00:00:00+00'
);

COMMIT;
```

The example cutoff and interval are intentionally illustrative; calculate
them from the actual maintenance date. Review the affected partition names
and row counts before committing. Do not delete rows directly from the parent
as a substitute for dropping a complete old partition.
