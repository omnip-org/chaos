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

A publication and client-delivery surface within one Store, such as web, mobile, point of sale, marketplace, or custom. Products may be published to selected channels. A Sales Channel does not own products or orders independently of its Store.

## Product and Variant

A Product is Store-owned catalog content. Variants are purchasable combinations of Product options. Product lifecycle and channel publication are separate decisions: a Product must be active and published to a channel before that channel may serve it.

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

A Cart is a mutable Storefront working set owned by a Shopper. It remains active
while a Stripe Embedded Checkout session is pending. Chaos creates one Order and
one Stripe Checkout Session per payment attempt; Stripe collects the checkout
address and calculates tax, promotions, shipping, and the final total. Verified
Stripe webhooks reconcile those facts onto the Order, while Chaos retains
inventory and fulfillment state.

## Payment and Refund

A Payment records authorization and capture against one Order. A Refund references captured value and may never cause total successful or pending refunds to exceed the captured amount.

## Access Key

A private credential owned by one User and used by trusted clients such as MCP, CLI, or server-side integrations. The plaintext is shown once; only verification material is stored. An Access Key never grants Store access by itself. Every Store-scoped request selects a Store and checks the User's current membership and role. Its plaintext format is `ak_<43 Base58 characters>`.

The authenticated operation chain is `Access Key -> User -> Store Membership -> Store`. Request telemetry retains the Access Key, User, Store, and request identities so AI-driven mutations are attributable.

## Publishable Key

A Store-scoped public credential for storefront or Sales Channel clients. It resolves an active Sales Channel and can enter the complete Store API. Operation-specific Shopper credentials, tracking capabilities, resource ownership, and business rules protect non-public data and mutations. It cannot authenticate trusted administration clients or invoke Store administration. Its plaintext format is `pk_<24 Base58 characters>`.
