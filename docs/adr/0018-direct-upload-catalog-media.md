# ADR 0018: Verified Direct-Upload Catalog Media

- Status: Accepted
- Date: 2026-08-16

## Context

Catalog images and videos are large binary objects with different scaling, availability, and security characteristics from transactional PostgreSQL data. Proxying uploads through the commerce API consumes application memory and bandwidth, while accepting arbitrary merchant URLs and fetching them server-side creates an SSRF boundary. A database row alone also cannot prove that the expected object was uploaded successfully.

## Decision

Catalog owns Media Asset metadata and its attachment to one Product and optional Variant in the same Store. Object bytes live behind a provider-neutral `MediaStorage` application port implemented by an S3-compatible infrastructure adapter.

Creation records a `pending_upload` asset with a server-generated object key, normalized file name, allowlisted media type, bounded byte count, expected SHA-256 digest, alt text, and stable position. The application returns a short-lived presigned PUT request whose signed headers bind the content type, content length, and checksum. An expired upload request can be refreshed only while the asset remains pending.

Completion performs a bounded object metadata request through the storage port and compares object key, media type, byte count, and checksum with the authoritative pending record. Only an exact match transitions the asset to `ready`. Missing or mismatched objects remain pending and return a conflict. `archived` is terminal and immediately removes the asset from Storefront reads. Runtime writes append immutable Media events and cannot delete Media roots or audit evidence.

The public URL is derived from the configured asset origin and server-owned object key; clients cannot submit it. The API never fetches merchant-controlled URLs. Storefront media is returned only when the parent Store, Sales Channel, Product, Product publication, and Media Asset are all active or ready as applicable.

## Consequences

- Binary traffic bypasses the OLTP API and PostgreSQL.
- Upload retries do not create duplicate assets, and short-lived credentials can be refreshed safely.
- A successful client PUT is not treated as ready until storage metadata is verified.
- Storage-provider SDK types stay in infrastructure.
- Image transformation and CDN cache policy can evolve behind the asset origin without changing Catalog identity.

## Rejected alternatives

### Store binary data in PostgreSQL

This couples database backups, replication, connection pools, and transaction latency to large media payloads.

### Fetch arbitrary remote URLs

Server-side imports require a separate hardened fetch service with DNS pinning, redirect controls, address filtering, and content scanning. That is not part of the initial Media upload path.

### Mark Media ready when the client says upload completed

Client acknowledgement does not prove object existence or integrity. The storage adapter must verify authoritative metadata.
