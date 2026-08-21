# ADR 0024: Add Stripe Embedded Checkout Alongside PaymentIntents

- Status: Accepted
- Date: 2026-08-18

## Context

ADR 0013 committed to Stripe PaymentIntents confirmed client-side with Stripe.js Elements. That remains the right default for storefronts that want a fully custom payment UI. The reference storefront instead uses Stripe Embedded Checkout so the payment form remains inside the storefront while card-data handling stays within Stripe-hosted UI. Stripe models this as a Checkout Session with its own object id prefix (`cs_`) and webhook event family (`checkout.session.*`).

The existing `PaymentClientAction` contract assumed a single response shape: a PaymentIntent client secret for `stripe.confirmPayment()`. Embedded Checkout also needs a short-lived client secret, but the browser passes it to Stripe's `EmbeddedCheckoutProvider` instead. A widened action kind keeps this distinction explicit without introducing another Store API resource.

## Decision

Add a second `PaymentProvider` adapter, `stripe_checkout`, alongside the existing `stripe` adapter. Both use the Stripe account owning the configured API key, represented by a Store-unique internal `external_account_reference` beginning with `platform:`. Stripe Connect accounts and `Stripe-Account` routing are not supported. The adapters share credential handling and a pinned API version, but differ in which Stripe resource they create and how they report the resulting client action:

- `stripe` creates PaymentIntents (`POST v1/payment_intents`) and returns `PaymentClientAction { type: "confirm_payment", client_token: <PaymentIntent client secret> }`, unchanged from ADR 0013.
- `stripe_checkout` creates Checkout Sessions (`POST v1/checkout/sessions`, `mode=payment`, `ui_mode=embedded_page`, one aggregate order-total line item) and returns `PaymentClientAction { type: "mount_embedded_checkout", client_token: <Checkout Session client secret> }`. The storefront passes this secret to Stripe's Embedded Checkout component and never logs, caches, or places it in a URL.

A Store selects the flow by configuring a `payment_provider_accounts` row with `provider = "stripe_checkout"` instead of `"stripe"` — the same mechanism already used to configure any Provider account, requiring no new domain concept. `provider` was already a free-form string with no enum constraint.

`return_url` is supplied by the storefront when creating the Payment Attempt (`stripe_checkout` requires it; `stripe` ignores it). It is carried through the durable outbox event rather than stored on `payment_attempts`, because it is consumed only once when the Worker creates the Checkout Session. HTTPS is required except for HTTP loopback URLs used during local development.

Webhook verification is available at the provider-specific endpoints `POST /webhooks/v1/payments/stripe` and `POST /webhooks/v1/payments/stripe_checkout`. This keeps webhook-secret resolution aligned with the configured provider account. The supported account webhook envelope omits a Connect `account` field, and verification uses the configured Store-unique `platform:` reference. `map_stripe_event` handles `checkout.session.completed`, `checkout.session.async_payment_succeeded`, `checkout.session.async_payment_failed`, and `checkout.session.expired`, alongside the existing `payment_intent.*`/`refund.*` events, feeding the same downstream Payment Attempt state machine. No new status or persistence column is required: `provider_reference` stores the Checkout Session id (`cs_...`) as it stores a PaymentIntent id (`pi_...`).

Checkout Sessions have no separate authorization step — a fully paid session goes straight from nothing to `checkout.session.completed`. This maps onto the existing auto-authorize-then-capture shortcut that already exists for PaymentIntents whose `payment_intent.succeeded` event arrives while the Attempt is still `pending` (used when a provider's automatic capture skips a distinct authorization event). `checkout.session.completed` only triggers this when the event's `payment_status` is `paid` or `no_payment_required`; a `payment_status` of `unpaid` means an async payment method was selected and the checkout form was submitted, but funds have not settled — that case intentionally emits no state transition and waits for the `checkout.session.async_payment_succeeded`/`async_payment_failed` follow-up event.

## Consequences

- Stripe-specific wire types and authentication remain in infrastructure; the shared HTTP plumbing (credential resolution, form POST, authenticated GET) is factored into a small internal `StripeHttp` helper both adapters hold, avoiding duplicated request/response handling between the two adapters.
- `PaymentClientAction.kind`/OpenAPI `type` has two legal values with different consumers. Callers must branch on `type` and treat both token forms as short-lived secrets.
- `return_url` is shopper-browser supplied. Storefronts must construct it from their own trusted origin; the API permits only HTTPS or local loopback HTTP.
- Two Stripe-backed provider accounts can be configured per Store (one `stripe`, one `stripe_checkout`) since `payment_provider_accounts` allows one row per distinct `provider` value. Both use the shared account-readiness check against the account owning the configured API keys.
