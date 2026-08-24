# ADR 0015: Use EasyPost as the First Shipping Provider Adapter

- Status: Superseded (2026-08-24) — Chaos does not integrate EasyPost or any
  carrier API. Fulfillment tracking is manual-only, recorded through MCP
  tools against `commerce.fulfillments` and the `manual` account in
  `integration.shipping_provider_accounts`.
  All EasyPost-specific code has been removed. Kept for historical context.
- Date: 2026-08-16

## Context

The manual Shipping Service baseline is sufficient to freeze a checkout price and delivery estimate, but it cannot obtain carrier rates, purchase a label, request cancellation, or reconcile tracking. These capabilities belong behind the Fulfillment application boundary rather than in Sales or a new logistics context.

The first production adapter must support multiple carriers without placing carrier-specific service names, units, label formats, credentials, or tracking states in domain models. It must also preserve exact money and tolerate an uncertain network outcome after a provider mutation.

## Decision

EasyPost is the first shipping provider adapter. The application `ShippingProvider` port exposes four business capabilities—rate quotation, label purchase, label cancellation where available, and tracking refresh—plus an internal Shipment reconciliation capability used before retrying an uncertain mutation. EasyPost request and response types remain in infrastructure.

Chaos uses millimetres, grams, ISO currency codes, and integer minor units at the port. The adapter converts dimensions to inches, weight to ounces, and decimal rate strings to minor units without floating-point money arithmetic. Provider identifiers are validated before use in URL paths, and label URLs must be HTTPS.

A quote creates an immutable EasyPost Shipment and sends the stable Chaos operation key as its `reference`. A selected EasyPost Rate is then used to buy the Shipment. Chaos persists the provider Shipment, Rate, Tracker, tracking number, label media type, and label URL as Fulfillment evidence before completing the operation. Retrying an uncertain purchase first reconciles the existing provider Shipment instead of blindly purchasing another label.

EasyPost cancellation is a label-refund request, not a promise that a parcel in transit is intercepted. Chaos maps `submitted`, `refunded`, `rejected`, and `not_applicable` into explicit provider-neutral cancellation states. Fulfillment cancellation rules remain authoritative and do not infer cancellation from a refund request alone.

Tracking refresh retrieves the EasyPost Tracker and maps its status into provider-neutral tracking observations. Only the Fulfillment application use case may translate a delivered observation into a Fulfillment transition. Unknown provider states fail closed for reconciliation rather than silently advancing commerce state.

Provider credentials remain behind the secret-resolution application port. Store configuration contains only a constrained secret reference and provider identity. The current adapter accepts encrypted `enc://` references created through MCP and may also resolve deployment-managed references without exposing plaintext to the Store configuration model.

## Consequences

- The Store can use one EasyPost integration for carrier rate comparison and label operations.
- Carrier-specific objects and imperial units do not leak into domain or public contracts.
- A cancellation request can remain submitted or be rejected and therefore requires reconciliation.
- International customs data, multi-parcel shipments, manifests, pickups, and carrier-account selection remain future capabilities.
- A future direct-carrier adapter can implement the same capability-specific port without changing Sales snapshots or Fulfillment invariants.

## Provider constraints

- EasyPost creates an immutable Shipment from origin, destination, and Parcel data and returns carrier Rates: [Shipment API](https://www.easypost.com/docs/shipments#create-a-shipment).
- Buying a Shipment requires a selected Rate and returns tracking and PostageLabel data: [buy a Shipment](https://www.easypost.com/docs/shipments#buy-a-shipment).
- Refunding a Shipment is carrier-dependent and may remain submitted before becoming refunded or rejected: [shipping refund](https://www.easypost.com/docs/shipments/shipping-refund).
- Tracker retrieval returns normalized status, status detail, estimated delivery, and tracking history: [Tracker API](https://www.easypost.com/docs/trackers).
