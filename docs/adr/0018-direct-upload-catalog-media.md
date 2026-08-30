# ADR 0018: Verified Direct-Upload Catalog Media

- Status: Accepted
- Date: 2026-08-16

## Context

Catalog images and videos are large binary objects with different scaling, availability, and security characteristics from transactional PostgreSQL data. Proxying uploads through the commerce API consumes application memory and bandwidth, while accepting arbitrary client URLs and fetching them server-side creates an SSRF boundary. A database row alone also cannot prove that the expected object was uploaded successfully.

## Decision

Catalog owns reusable Media Asset metadata and its typed attachment to a Product, Review, or Product metadata path in the same Store. Product, Review, and Product metadata attachment tables own target-specific fields such as position, alt text, and JSON Pointer path. Object bytes live behind a provider-neutral `MediaStorage` application port implemented by an S3-compatible infrastructure adapter.

Creation records a `pending` asset with a server-generated object key, normalized file name, allowlisted media type, bounded byte count, and expected SHA-256 digest. The application returns a short-lived presigned PUT request whose signed headers bind the content type, content length, and checksum. An expired upload request can be refreshed only while the asset remains pending. Target-specific placement and alt text are added later through typed attachment operations.

Completion performs a bounded object metadata request through the storage port and compares object key, media type, byte count, and checksum with the authoritative pending record. Only an exact match transitions the asset to `ready`. Missing or mismatched objects remain pending and return a conflict. `archived` is terminal and immediately removes the asset from Storefront reads. Runtime roles cannot delete Media roots; lifecycle state and attachment rows are the current source of truth, and no separate Media audit ledger is maintained.

The public URL is derived from the configured asset origin and server-owned object key; clients cannot submit it. The API never fetches client-controlled URLs. Storefront media is returned only when the parent Store, Sales Channel, Product, Product publication, and Media Asset are all active or ready as applicable.

The MCP surface follows the same split: `prepare_media_upload` accepts only
file metadata and returns a short-lived presigned PUT request; the MCP Host
uploads the original bytes directly to object storage; and
`complete_media_upload` verifies the stored object before the asset becomes
ready. `refresh_media_upload` reissues the PUT request while the asset is still
pending. A ready asset is then attached through a typed Product, Review, or
Product metadata tool. No MCP tool accepts inline Base64 media bytes.
The upload request is returned in the structured model-facing tool result under
`upload`, because the current MCP Host is controlled and needs to consume it
directly. Hosts must treat it as a short-lived bearer credential and must not
log it.

## Consequences

- Binary traffic bypasses the OLTP API and PostgreSQL.
- Upload retries do not create duplicate assets, and short-lived credentials can be refreshed safely.
- A successful client PUT is not treated as ready until storage metadata is verified.
- Storage-provider SDK types stay in infrastructure.
- Image transformation and CDN cache policy can evolve behind the asset origin without changing Catalog identity.
- Product, Review, and Product metadata images share one verified object lifecycle without allowing Review images to leak into Product gallery reads.
- Product metadata stores a stable `media_asset_id` reference, while Storefront reads resolve the current public URL from the ready Media Asset. A typed metadata link preserves lifecycle and prevents a managed reference from being silently overwritten.

## Rejected alternatives

### Store binary data in PostgreSQL

This couples database backups, replication, connection pools, and transaction latency to large media payloads.

### Fetch arbitrary remote URLs

Server-side imports require a separate hardened fetch service with DNS pinning, redirect controls, address filtering, and content scanning. That is not part of the initial Media upload path.

### Mark Media ready when the client says upload completed

Client acknowledgement does not prove object existence or integrity. The storage adapter must verify authoritative metadata.
