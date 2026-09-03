# ADR 0032: Store Storefront Origins on Sales Channels

- Status: Accepted (order link path revised by ADR 0034)
- Date: 2026-08-25

## Context

Chaos can host multiple Stores and multiple Sales Channels in one deployment.
Order confirmation emails contain a customer-facing link to the order, but a
deployment-level storefront URL cannot identify which Store or Sales Channel
should receive that link. An Order already records its `channel_id`, and
Storefront Publishable Keys resolve an active Store and Sales Channel.

## Decision

Each Sales Channel stores one normalized `origin`, such as
`https://shop.example.com/`. The origin must be an absolute HTTP(S) origin with
no credentials, path, query, or fragment and is unique across Sales Channels.

The default web Sales Channel receives its origin when a Store is created.
Creating or updating any Sales Channel requires an origin because every channel
that can create an Order must provide a browser destination for the guest order
lookup page.

The order-confirmation email worker joins the Order to its Sales Channel and
builds `/orders/lookup?order_number=...&email=...` from that channel's origin
(see ADR 0034). There is no deployment-level storefront URL or fallback.

This decision does not add Host-header routing. Storefront clients still select
the Store and Sales Channel through their Publishable Key; a deployment that
serves multiple domains must configure the appropriate client key per domain.

## Consequences

- A single Worker can send correct order links for every Store and Channel.
- Store provisioning and Sales Channel administration require a canonical web origin.
- Changing an origin changes links generated after the update; deployments should
  keep old domains redirecting when previously delivered links must remain usable.
- `PUBLIC_BASE_URL` remains the platform/API origin used for provider metadata and
  is unrelated to storefront origins.
