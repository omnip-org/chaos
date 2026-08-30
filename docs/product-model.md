# Product Model

## User

A person who operates Chaos. A User authenticates through a configured external identity provider and may participate in multiple Stores. Explicit linking of another Provider identity is not supported yet.

## External Identity

A verified `(provider, subject)` pair mapped to one User. Google and Apple are the initial provider kinds. Provider email is retained as verified profile data but is not the durable identity key.

## Store

The tenant and commerce ownership boundary. A Store owns configuration, memberships, Sales Channels, products, variants, publication state, prices, inventory, Shoppers, orders, payment configuration, refunds, and fulfillment configuration.

## Store Membership

The relationship between a User and a Store. It grants the role used by human administration. The first supported roles are `owner` and `member`; finer permissions should be introduced only when a concrete operation requires them.

An owner may create and administer the Store, add Users as members, and grant or revoke the Owner role. A User may leave a Store. Membership changes are explicit; knowing a Store identifier never grants access. A Store may not be left without an Owner.

## Sales Channel

A publication and client-delivery surface within one Store, such as web, mobile, point of sale, marketplace, or custom. Products may be published to selected channels. Each Sales Channel stores the canonical storefront origin used for customer-facing links such as order tracking. A Sales Channel does not own products or orders independently of its Store.

## Product and Variant

A Product is Store-owned catalog content. Variants are purchasable combinations of
Product options. Option, Option Value, and Variant identifiers are stable across
catalog edits; removing one from the active configuration archives it instead of
reusing its identity. This lets media rules and external integrations retain
stable references while an editor changes the active set.

Product lifecycle and channel publication are separate decisions: a Product must
be active and published to a channel before that channel may serve it. Product
configuration changes, canonical content changes, publication changes, and
catalog media changes increment the Product revision. An active Product must
retain at least one active Variant. MCP writers can pass the revision returned by
a workspace read as `expected_revision` to reject stale updates.

## Order

An immutable commercial record after creation, with controlled state transitions. It freezes the selected products, variants, quantities, price snapshot, Shopper-linked contact, addresses, channel, and relevant provider evidence. Stripe owns Checkout tax, promotion, shipping, and final-total calculation; the resulting subtotal, discount, tax, shipping, and total are stored as provider-reported Order facts. A Shopper who creates an Order is the buyer; there is no separate Customer entity.

## Shopper

A Storefront visitor identity scoped to one Store and created when the website
opens a Shopper session. A signed possession token carries its UUID through Cart,
Checkout, Payment, Order, and Analytics operations. Sales Channel is request
context, not Shopper identity. The possession token has the compact format
`shopper.<shopper_id>.<signature>`; Store and Sales Channel are covered by the
HMAC signature but are not exposed as token fields. The signature is encoded with
unpadded Base64URL. Contact details are captured as Checkout and Order snapshots
rather than as a separate Customer profile.

## Cart and Checkout

A Cart is a mutable Storefront working set owned by a Shopper. Starting checkout
locks the source Cart as `locked`, creates exactly one pending Order, and returns
the payment client action needed to mount the provider form. The source Cart
cannot be edited or checked out again. A subsequent active Cart is obtained or
created by the SDK after the checkout transaction; it is not linked into the
Order model. The private `payment_client_action` on the locked Cart is the only
persisted payment-form recovery data. Retrying the same Cart checkout request
returns that action and never creates another provider session. Stripe collects the checkout address
and calculates tax, promotions, shipping, and the final total. Verified Stripe
webhooks reconcile those facts onto the Order, while Chaos retains inventory and
fulfillment state. The browser SDK prepares one attributed commerce envelope
before the cart or checkout request, but the business request remains
analytics-agnostic. After a successful response, the SDK sends the event through
the common `/analytics/events` endpoint with canonical response values and
projects the same event ID to browser providers. The browser-side
`InitiateCheckout` event is stored with its `order_id`; retrying a Cart checkout
does not emit a second initiation event. The server-side `Purchase` event later looks
up that exact event and combines its attribution with the final
provider-reconciled total. No attribution is stored on the Order, and Meta can
deduplicate the Pixel and CAPI copies using the shared event ID.

## Payment and Refund

A Payment records authorization and capture against one Order. A Refund references captured value and may never cause total successful or pending refunds to exceed the captured amount.

## MCP OAuth

MCP clients authenticate with an OAuth 2.1 authorization-code flow using PKCE.
The short-lived access token identifies the User for the MCP resource; every
Store-scoped operation still selects a Store and checks the User's current
membership and role.

## Publishable Key

A Store-scoped public credential for storefront or Channel clients. It is bound
to one active Channel when created and can enter the complete Store API.
Operation-specific Shopper credentials, tracking capabilities, resource
ownership, and business rules protect non-public data and mutations. It cannot
authenticate administration clients or invoke Store administration. Its
plaintext format is `pk_<24 Base58 characters>`.
