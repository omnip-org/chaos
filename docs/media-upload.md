# Catalog media uploads

Catalog media uses a two-phase direct-upload flow. MCP carries the control
plane; the MCP Host carries the file bytes directly to the configured
S3-compatible object storage.

## Flow

1. Call `prepare_product_media_upload` with the Store, Product, file name,
   MIME type, exact byte size, lowercase SHA-256 digest, optional variant,
   alt text, position, and `confirm: true`.
2. Read the short-lived upload request from the tool result's
   `_meta.com.omniporg.chaos/media-upload` field.
3. The Host sends a `PUT` of the original file bytes to the returned URL and
   applies every returned header exactly as provided. The file must be the
   same file represented by the preparation metadata.
4. Call `complete_product_media_upload` with the returned `media_asset_id` and
   `confirm: true`. Chaos performs a storage metadata check and marks the asset
   `ready` only when the type, size, and SHA-256 match.

If the presigned request expires before the PUT, call
`refresh_product_media_upload` and repeat step 3. Refreshing is allowed only
while the asset is still pending.

The upload metadata contains a short-lived bearer credential. Hosts must keep
it out of logs, prompts, and user-visible text. The model-facing content only
contains the pending asset and the next-step instruction; the Host is
responsible for consuming the `_meta` value.

There is intentionally no inline-Base64 media tool. A Host that cannot access
the user's attachment or perform the direct PUT needs a separate upload
adapter; the remote Chaos MCP server cannot read a local file on its own.
