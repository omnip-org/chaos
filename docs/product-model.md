# Product Model

## User

A person who operates Chaos. A user authenticates through one or more external identity providers and may participate in multiple Stores.

## External Identity

A verified `(provider, subject)` pair mapped to one User. Google and Apple are the initial provider kinds. Provider email is retained as verified profile data but is not the durable identity key.

## Store

The tenant and commerce ownership boundary. A Store owns configuration, memberships, Sales Channels, products, variants, publication state, prices, inventory, customers, orders, payment configuration, refunds, and fulfillment configuration.

## Store Membership

The relationship between a User and a Store. It grants the role used by human administration. The first supported roles are `owner` and `member`; finer permissions should be introduced only when a concrete operation requires them.

An owner may create and administer the Store, add Users as members, and grant or revoke the Owner role. A User may leave a Store. Membership changes are explicit; knowing a Store identifier never grants access. A Store may not be left without an Owner.

## Sales Channel

A publication and client-delivery surface within one Store, such as web, mobile, point of sale, marketplace, or custom. Products may be published to selected channels. A Sales Channel does not own products or orders independently of its Store.

## Product and Variant

A Product is Store-owned catalog content. Variants are purchasable combinations of Product options. Product lifecycle and channel publication are separate decisions: a Product must be active and published to a channel before that channel may serve it.

## Order

An immutable commercial record after creation, with controlled state transitions. It freezes the selected products, variants, quantities, money, tax, discount, customer contact, addresses, channel, and relevant provider evidence.

## Payment and Refund

A Payment records authorization and capture against one Order. A Refund references captured value and may never cause total successful or pending refunds to exceed the captured amount.

## Access Key

A private credential owned by one User and used only to authenticate MCP. The plaintext is shown once; only verification material is stored. An Access Key never grants Store access by itself. Every tool call selects a Store and checks the User's current membership and role.

The authenticated operation chain is `Access Key -> User -> Store Membership -> Store`. Request telemetry retains the Access Key, User, Store, and request identities so AI-driven mutations are attributable.

## Publishable Key

A Store-scoped credential for storefront or Sales Channel clients. It carries only explicitly allowed public capabilities and may select a Sales Channel. It cannot authenticate MCP or invoke Store administration.
