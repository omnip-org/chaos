import { toPurchaseAnalyticsInput } from "../domain.js";
import type { OrderLookup } from "../types.js";
import {
  sendAddToCartCapi,
  sendInitiateCheckoutCapi,
  sendPurchaseCapi,
  type MetaCapiConfig,
  type MetaCapiContext,
} from "./capi.js";
import type { AddToCartAnalyticsInput, InitiateCheckoutAnalyticsInput } from "./types.js";

export type { MetaCapiConfig, MetaCapiContext };

export interface ServerEventsOptions {
  /** The store's own Meta access token, from this deployment's environment variables. */
  meta: MetaCapiConfig;
  randomUUID?: () => string;
}

/**
 * Server-only Meta CAPI dispatcher — the CAPI-sending counterpart to the
 * browser's `ChaosStorefrontAnalytics` (`@omnip-org/chaos-js`'s
 * `events/browser.ts`). Pass an instance as `events` to
 * `StorefrontServerClient` (see `ssr/server.ts`'s `ServerEventsPort`)
 * so its cart and checkout facades send Meta CAPI automatically, sharing
 * their event IDs with the matching browser Pixel projections.
 *
 * Lives behind the `/meta-capi` subpath, never the main entry, because it
 * holds the store's Meta access token.
 */
export class ChaosServerEvents {
  private readonly meta: MetaCapiConfig;
  private readonly randomUUID: () => string;

  constructor(options: ServerEventsOptions) {
    if (!options?.meta) throw new TypeError("meta is required");
    this.meta = options.meta;
    this.randomUUID =
      options.randomUUID ?? globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    if (!this.randomUUID) {
      throw new TypeError(
        "randomUUID is required (pass options.randomUUID in environments without globalThis.crypto)",
      );
    }
  }

  /** Sends AddToCart to Meta CAPI, minting an event ID when the caller has none to share with a browser Pixel projection. */
  async recordAddToCart(
    input: AddToCartAnalyticsInput,
    context?: MetaCapiContext,
    eventId?: string,
  ): Promise<string> {
    const resolvedId = eventId ?? this.randomUUID();
    await sendAddToCartCapi(this.meta, { eventId: resolvedId, ...(context ? { context } : {}), input });
    return resolvedId;
  }

  /** Sends InitiateCheckout to Meta CAPI. See `recordAddToCart`. */
  async recordInitiateCheckout(
    input: InitiateCheckoutAnalyticsInput,
    context?: MetaCapiContext,
    eventId?: string,
  ): Promise<string> {
    const resolvedId = eventId ?? this.randomUUID();
    await sendInitiateCheckoutCapi(this.meta, { eventId: resolvedId, ...(context ? { context } : {}), input });
    return resolvedId;
  }

  /**
   * Projects a confirmed, paid order using its deterministic order-derived
   * event ID — the same ID a browser `recordConfirmedPurchase()` call uses
   * for the same order, so Meta dedupes the pair. No-op for an order that
   * isn't confirmed and paid.
   */
  async recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
    context?: MetaCapiContext,
  ): Promise<void> {
    const input = toPurchaseAnalyticsInput(order);
    if (!input) return;
    await sendPurchaseCapi(this.meta, {
      eventId: input.orderId.toLowerCase(),
      ...(context ? { context } : {}),
      input,
    });
  }
}
