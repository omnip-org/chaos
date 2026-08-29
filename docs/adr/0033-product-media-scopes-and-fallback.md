# ADR 0033: Product Media Scopes and Variant Fallback

- Status: Accepted
- Date: 2026-08-29

## Context

A Product may have many Variants formed from independent Options such as
Length and Color. The same image can apply to every Variant, to one Option
Value, to several Option Values, or to one exact Variant. A nullable
`product_variant_id` on one Product link table cannot represent these cases:
the same physical asset can be attached only once per Product, and attaching
it again changes the previous row instead of creating an independent
variant-specific relation.

Media editing is also a frequent operation. Administrators need predictable
single-item upsert, target-level replacement, inspection, and removal without
duplicating object metadata or accidentally deleting an image still used by a
different target.

## Decision

Keep `commerce.media_assets` as the reusable physical-object record and use
three Product link tables:

- `product_media_assets` for Product fallback media;
- `product_option_value_media_assets` for one Product Option Value;
- `product_variant_media_assets` for one exact Product Variant.

Every link is Store-scoped, has its own `alt_text`, `position`, and
`archived_at`, and references the same `media_assets` row through the
composite Store foreign key. Position uniqueness is enforced independently
within each target. All three links may point to the same physical asset.

The effective gallery for a selected Variant uses this precedence:

1. exact Variant links, if any active ready links exist;
2. links matching any selected Option Value, if any exist;
3. Product links.

When multiple selected Option Values point at media, the result is the union
of matching links, deduplicated by `media_asset_id` and ordered by position
then asset ID. Exact Variant links are an override set rather than an
additional union, so a Variant can intentionally replace its inherited
gallery.

The Storefront Product API returns active ready Product media rules as one
flat `media` collection. Each item has `scope` and carries `option_id`,
`option_value_id`, or `product_variant_id` according to that scope. This keeps
the response compact when many Variants share one image and lets the first-
party JavaScript SDK expose `resolveProductMedia` for client-side selection.
Storefront cart lines resolve the same rule server-side, so checkout displays
the media applicable to the exact Variant in the cart.

The MCP API exposes one clear CRUD surface for each Product scope:

- `attach_*` performs single-item insert/update/reorder and clears a previous
  archive on the same link;
- `replace_*` atomically accepts the complete desired target state, making
  bulk add/update/remove/reorder one call; an empty list clears the target;
- `list_*` inspects either all Product scopes or one Option Value/Variant;
- `archive_*` removes one target link while preserving the shared physical
  object when another active link still references it.

All mutating tools retain explicit `confirm: true`. Replacements validate the
whole request before opening the database transaction and never leave a
partially applied target state.

## Consequences

- A color image can be reused by every length without copying bytes or rows
  into every Variant.
- A Variant can override inherited media without changing sibling Variants.
- Product pages need the SDK resolver (or an equivalent implementation) when
  they consume the raw Product API media rules.
- Product media reads perform a three-table union, and cart reads additionally
  load Variant selections to resolve the effective gallery.
- Removing a link may archive an otherwise unreferenced asset, but never
  archives a physical object that remains actively linked elsewhere.

## Rejected alternatives

### Keep a nullable `product_variant_id` on one Product table

This cannot represent Option Value scope and makes the Product/Variant/media
asset key ambiguous: one asset can be linked once per Product, so a later
Variant link overwrites the earlier Product-level or Variant-level meaning.

### Materialize one media row for every Variant

This duplicates relationships for combinatorial Products and makes routine
gallery edits expensive and error-prone. The selected Option Value scope is
the reusable middle layer.

### Return only the already resolved gallery from Product endpoints

That saves client logic but prevents a client from resolving a different
selection before a Variant is fully selected and makes the Product response
less useful for editors and custom storefronts. The raw rule collection plus
one SDK resolver keeps the contract explicit.
