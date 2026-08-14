# HTTP API Contract

## Success responses

Objects and collections are wrapped in `data`. The optional `meta` object is emitted only when pagination or other metadata exists.

```json
{
  "data": {
    "id": "019c...",
    "name": "ACME"
  }
}
```

Cursor pagination uses the following shape:

```json
{
  "data": [],
  "meta": {
    "page": {
      "has_more": false,
      "next_cursor": null
    }
  }
}
```

Successful creation returns HTTP 201. A synchronous deletion with no response body returns HTTP 204. Never replace meaningful HTTP statuses with `HTTP 200 + a custom numeric status in the body`.

## Error responses

```json
{
  "error": {
    "code": "validation_failed",
    "message": "one or more fields are invalid",
    "details": [
      { "field": "slug", "reason": "must be lowercase" }
    ]
  }
}
```

- `code` is a stable machine-readable identifier. It must not be renamed after release without a compatibility plan.
- `message` is safe and human-readable but is not a stable client contract. Clients localize errors by `code`.
- `details` is included only when structured context exists. Empty arrays are omitted.
- Internal errors, SQL, stack traces, provider payloads, and secrets never appear in responses. They are recorded only in server-side telemetry.
- `x-request-id` correlates client reports, logs, and traces. The service generates or propagates it.

## Error mapping

| Application error | HTTP | Default code |
|---|---:|---|
| Validation | 422 | `validation_failed` |
| Unauthorized | 401 | `unauthorized` |
| Forbidden | 403 | `forbidden` |
| NotFound | 404 | `not_found` |
| Conflict | 409 | A stable use-case-specific code |
| Unavailable | 503 | `service_unavailable` |
| Unexpected | 500 | `internal_error` |

The domain layer emits domain errors. The application layer translates them into use-case semantics. The HTTP layer is the only layer that chooses HTTP statuses and JSON representations.
