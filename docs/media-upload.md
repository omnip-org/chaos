# Catalog media uploads

MCP carries the Media control plane and the MCP Host carries file bytes directly
to the configured S3-compatible object storage. `media_assets` stores one
reusable, verified physical object. Product gallery, Review, and Product
metadata tables store typed business attachments; they do not duplicate the
object or its integrity metadata.

## Generic upload flow

1. Call `prepare_media_upload` with the Store, file name, MIME type, exact byte
   size, lowercase SHA-256 digest, and `confirm: true`.
2. Read the short-lived upload request from `structuredContent.upload` in the
   tool result.
3. The Host sends a `PUT` of the original file bytes to the returned URL and
   applies every returned header exactly as provided. The file must be the
   same file represented by the preparation metadata.
4. Call `complete_media_upload` with the returned `media_asset_id` and
   `confirm: true`. Chaos performs a storage metadata check and marks the asset
   `ready` only when the type, size, and SHA-256 match.
5. Attach the ready asset with exactly one of the typed tools:
   `attach_product_media`, `attach_review_media`, or
   `attach_product_meta_media`.

If the presigned request expires before the PUT, call
`refresh_media_upload` and repeat step 3. A Media Asset can be attached to
multiple targets; it is archived only after every active attachment is removed.
`get_media_asset` returns the reusable asset state, while
`archive_media_asset` handles an unreferenced asset.

The upload request contains a short-lived bearer credential and is intentionally
included in the model-visible tool result so the current MCP Host can use it.
Hosts should still avoid logging it and must upload before it expires.

## Product metadata images

`attach_product_meta_media` accepts an RFC 6901 JSON Pointer such as
`/landing_page/hero/image`. It atomically writes this value into Product
metadata:

```json
{
  "landing_page": {
    "hero": {
      "image": {
        "media_asset_id": "...",
        "alt_text": "..."
      }
    }
  }
}
```

When the target already contains an object, the tool changes only the managed
`media_asset_id` and `alt_text` fields, preserving presentation fields such as
`crop` or `width`.

The Storefront response resolves an active ready reference with `url` and
`media_type` at read time. The database keeps a typed
`product_meta_media_assets` link so replacement and removal can archive the
old object safely. Use `list_product_meta_media` to inspect links and
`archive_product_meta_media` to remove one. Direct wholesale Product metadata
updates cannot remove or alter the managed media reference fields; archive or
replace them through the Media tool first.

## Manually imported Review feedback

For feedback received through a private message, email, phone call, or another
external channel:

1. Call `create_manual_review` with the Product, explicit customer rating,
   transcribed content, source channel, and confirmed publication consent.
   The review starts `pending`; it is not automatically a verified purchase.
2. Prepare and upload each image through the generic flow above.
3. Call `attach_review_media` for each completed image.
4. Only after all images are ready should a moderator call `approve_review`.

Review images remain in the Review attachment table and never appear in the
Product gallery.

There is intentionally no inline-Base64 media tool. A Host that cannot access
the user's attachment or perform the direct PUT needs a separate upload
adapter; the remote Chaos MCP server cannot read a local file on its own.
