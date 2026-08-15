# ADR 0010: Recalculate Before Freezing Checkout

- Status: Accepted
- Date: 2026-08-16

## Context

Storefront shipping quotes, prices, inventory, addresses, tax rules, and promotions can change between browsing and settlement. Accepting client-calculated money or mutating an existing Checkout would make retries non-deterministic and weaken payment and Order evidence.

## Decision

A Cart is mutable and a Checkout is immutable. Shipping-option responses are advisory and contain a service identity rather than an authorization to reuse an amount. Creating a Checkout is the only recalculation boundary.

The Checkout transaction locks the active Cart and then resolves current state in this order:

1. revalidate Store, Sales Channel, Product publication, Variant, Price List, and current Price;
2. replace Cart commercial line snapshots with those current values;
3. resolve the tax destination from the shipping address for a shippable Cart, otherwise from the billing address;
4. evaluate active automatic Promotions and an optional submitted redemption code against the current settlement currency, subtotal, and half-open schedule, then select one winner by greatest discount, lowest priority, and stable ID;
5. allocate the selected Promotion once across line subtotals, require one active Store Tax Rule for the destination, and calculate tax from the discounted server-owned line amounts;
6. revalidate the selected Shipping Service against Store, destination country, settlement currency, and active status;
7. lock and reserve tracked inventory;
8. freeze lines, allocations, rule evidence, customer data, shipping selection, and aggregate totals atomically.

The tax rate uses integer basis points. Tax is rounded once at the Checkout level using half-up rounding and allocated to lines by taxable amount with stable line ordering and deterministic remainder distribution. Tax-exclusive prices add tax to the total. Tax-inclusive prices extract the included tax component without increasing the total. The current baseline taxes merchandise after discounts and does not tax shipping; a future shipping-tax policy must be explicit rather than changing this behavior silently.

Promotions are Store-owned and currency-specific. Automatic and code-triggered rules support percentage or fixed-amount values, an optional minimum subtotal, percentage caps, explicit priority, and half-open activation windows. Percentage discounts are rounded once at the Checkout level using half-up rounding. Fixed discounts are capped at the merchandise subtotal. The selected discount is allocated proportionally by line subtotal using stable line ordering. Promotions do not stack in this baseline: the greatest eligible discount wins, then lower priority, then stable Promotion ID. An absent eligible automatic Promotion means zero discount; a submitted invalid or ineligible code rejects Checkout creation.

After Checkout creation, later price, inventory, address, Tax Rule, Shipping Service, or promotion changes do not mutate it or its Order. An expired or abandoned Checkout is not reopened. The shopper creates a new Cart and Checkout to obtain a new calculation. Idempotent retries return the original frozen response.

## Consequences

- Clients never submit authoritative price, discount, tax, shipping amount, or total fields.
- Archiving a Tax Rule, Shipping Service, or selected Promotion before Checkout creation invalidates or changes the pending calculation; changing it afterward leaves historical evidence intact.
- Payment amount is derived only from the frozen Order total.
- Provider tax adapters may later supply a calculation through an application port, but their SDK types and payloads do not enter Sales or Pricing domain types.
- Promotion implementation must preserve the same ordering, allocation, snapshot, and retry rules.
