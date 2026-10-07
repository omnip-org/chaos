import { toPurchaseAnalyticsInput } from "../domain.js";
import { compact } from "../internal/meta.js";
import { sha256Hex } from "../internal/sha256.js";
import { toMajorUnits } from "../money.js";
import type {
  CartLineMutation,
  ConfirmedPurchaseOrderInput,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutStart,
  OwnOrder,
} from "../types.js";
import { BrowserEventState, observeHistory } from "./browser-state.js";
import {
  AnalyticsDestinations,
  normalizeMetaText,
  type AnalyticsErrorHandler,
  type AnalyticsProviderOptions,
} from "./destinations.js";
import {
  addToCartEventData,
  initiateCheckoutEventData,
  purchaseEventData,
  viewContentEventData,
  type MetaCommerceEventData,
} from "./meta-payload.js";
import type {
  AddToCartAnalyticsInput,
  AnalyticsCommerceItem,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
  ViewContentAnalyticsInput,
} from "./types.js";

/** Projects the fixed Storefront event set directly to Meta Pixel and GA4. */
export interface PageViewInput {
  path?: string;
  title?: string;
}

export interface AnalyticsOptions {
  publishableKey: string;
  document?: Document;
  window?: Window & typeof globalThis;
  storage?: Storage;
  sessionStorage?: Storage;
  randomUUID?: () => string;
  now?: () => number;
  providers?: AnalyticsProviderOptions;
  /** Starts lifecycle and SPA page tracking. Defaults to true. */
  autoStart?: boolean;
  /**
   * Best-effort delivery-failure hook: called when a browser provider call
   * (Pixel/GA4) throws, so a store can log or alert instead of failing
   * silently. Never awaited and never allowed to throw back into the caller
   * — mirrors `MetaCapiConfig.onError` for the server-side CAPI sender.
   */
  onError?: AnalyticsErrorHandler;
}

export class ChaosStorefrontAnalytics {
  private readonly documentRef: Document;
  private readonly windowRef: Window & typeof globalThis;
  private readonly randomUUID: () => string;
  private readonly now: () => number;
  private readonly destinations: AnalyticsDestinations;
  private readonly eventState: BrowserEventState;
  private running = false;
  private readonly onRouteChange = () => this.pageView();
  private restoreHistory: (() => void) | null = null;
  /** The shopper id currently being (or last) hashed for `setShopperId`. */
  private externalIdSource: string | null = null;

  constructor(options: AnalyticsOptions) {
    if (!options?.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.documentRef = options.document ?? globalThis.document;
    this.windowRef =
      options.window ?? (globalThis as unknown as Window & typeof globalThis);
    this.randomUUID =
      options.randomUUID ??
      globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    this.now = options.now ?? Date.now;
    if (!this.randomUUID || !this.documentRef || !this.windowRef) {
      throw new TypeError("randomUUID, document, and window are required");
    }
    this.destinations = new AnalyticsDestinations(
      this.windowRef,
      this.documentRef,
      options.providers,
      options.onError,
    );
    this.eventState = new BrowserEventState(
      options.publishableKey,
      this.documentRef,
      options.storage ?? this.windowRef?.localStorage,
      options.sessionStorage ?? this.windowRef?.sessionStorage,
      this.now,
    );
    if (options.autoStart !== false) {
      this.start();
      this.pageView();
    }
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.windowRef.addEventListener("popstate", this.onRouteChange);
    this.restoreHistory = observeHistory(this.windowRef, this.onRouteChange);
  }

  stop(): void {
    if (!this.running) return;
    this.running = false;
    this.windowRef.removeEventListener("popstate", this.onRouteChange);
    this.restoreHistory?.();
    this.restoreHistory = null;
  }

  /**
   * Feeds the shopper id to both providers' cross-session identity features.
   * GA4's User-ID takes the raw id directly — unlike Meta, it's an opaque
   * join key, not hashed PII, so it's set synchronously with no extra work.
   * Meta CAPI hashes the same canonical `shopper_id` into `external_id` (see
   * `meta_user_data` in `adapters/integrations/analytics/meta.rs`); this
   * hashes it the same way for Pixel's Advanced Matching so the browser and
   * server copies of an event resolve to the same Meta identity. Both are
   * best-effort: a missing/unavailable Web Crypto API just leaves Pixel
   * without its half.
   */
  setShopperId(shopperId: string | undefined): void {
    if (!shopperId || shopperId === this.externalIdSource) return;
    this.externalIdSource = shopperId;
    this.destinations.setGa4UserId(shopperId);
    sha256Hex(shopperId)
      .then((hash) => {
        if (this.externalIdSource === shopperId) {
          this.destinations.setExternalId(hash);
        }
      })
      .catch(() => {
        // Web Crypto unavailable or hashing failed; Pixel just runs without it.
      });
  }

  clearShopperId(): void {
    if (!this.externalIdSource) return;
    this.externalIdSource = null;
    this.destinations.setGa4UserId(null);
    this.destinations.setExternalId(null);
  }

  /** Adds the order identity already saved by Chaos to Meta Pixel matching. */
  async setMetaOrderIdentity(order: OwnOrder): Promise<void> {
    if (!this.destinations.hasPixel) return;
    const name = order.shipping_full_name ?? order.billing_full_name ?? "";
    const [firstName, ...lastName] = name.trim().split(/\s+/);
    const values: Record<string, string | undefined> = {
      em: order.contact_email?.trim().toLowerCase(),
      ph: order.contact_phone?.replace(/\D/g, ""),
      fn: normalizeMetaText(firstName),
      ln: normalizeMetaText(lastName.join(" ")),
      ct: normalizeMetaText(
        order.shipping_locality ?? order.billing_locality ?? undefined,
      ),
      st: normalizeMetaText(
        order.shipping_administrative_area ??
          order.billing_administrative_area ??
          undefined,
      ),
      zp: normalizeMetaText(
        order.shipping_postal_code ?? order.billing_postal_code ?? undefined,
      ),
      country: normalizeMetaText(
        order.shipping_country_code ?? order.billing_country_code ?? undefined,
      ),
    };
    try {
      const entries = await Promise.all(
        Object.entries(values)
          .filter((entry): entry is [string, string] => Boolean(entry[1]))
          .map(async ([key, value]) => [key, await sha256Hex(value)] as const),
      );
      this.destinations.setMetaCustomerData(Object.fromEntries(entries));
    } catch {
      // Web Crypto is optional; a Purchase must still reach Pixel and GA4.
    }
  }

  pageView(input: PageViewInput = {}): string {
    this.eventState.maintainFbcCookie();
    const path = input.path ?? this.documentRef.location?.pathname ?? "/";
    const title = input.title ?? nonEmpty(this.documentRef.title);
    const eventId = this.randomUUID();
    this.destinations.ga4(
      "page_view",
      compact({ event_id: eventId, page_path: path, page_title: title }),
    );
    return eventId;
  }

  /** Records a successful cart addition in the browser. */
  recordAddToCart(input: AddToCartAnalyticsInput, eventId?: string): string | null {
    validateMoney(input.valueMinor, input.currency);
    validateCommerceItem(input);
    if (input.cartId !== undefined && !isUuid(input.cartId))
      throw new TypeError("cartId must be a valid UUID");

    const resolvedId = canonicalEventId(eventId, this.randomUUID());
    try {
      return this.eventState.recordOnce("add_to_cart", resolvedId, () => {
        const eventData = addToCartEventData(input);
        this.destinations.pixel("AddToCart", resolvedId, eventData);
        this.destinations.ga4("add_to_cart", {
          event_id: resolvedId,
          value: eventData.value,
          currency: eventData.currency,
          items: toGa4Items(eventData.contents),
        });
      });
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
  }

  /** Records a successful embedded checkout creation in the browser. */
  recordInitiateCheckout(
    input: InitiateCheckoutAnalyticsInput,
    eventId?: string,
  ): string | null {
    validateMoney(input.valueMinor, input.currency);
    if (!isUuid(input.cartId))
      throw new TypeError("cartId must be a valid UUID");
    if (!isNonEmptyText(input.orderNumber))
      throw new TypeError("orderNumber must be a non-empty string");
    if (!Array.isArray(input.items) || input.items.length === 0)
      throw new TypeError("items must contain at least one checkout item");
    for (const item of input.items) validateCommerceItem(item);

    const resolvedId = canonicalEventId(eventId, this.randomUUID());
    try {
      return this.eventState.recordOnce("initiate_checkout", resolvedId, () => {
        const eventData = initiateCheckoutEventData(input);
        this.destinations.pixel("InitiateCheckout", resolvedId, eventData);
        this.destinations.ga4("begin_checkout", {
          event_id: resolvedId,
          transaction_id: input.orderNumber,
          value: eventData.value,
          currency: eventData.currency,
          items: toGa4Items(eventData.contents),
        });
      });
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
  }

  /** Records the increase produced by a successful cart mutation. */
  recordCartMutation(input: CartLineMutation): string | null {
    const quantity = input.new_quantity - input.previous_quantity;
    if (input.removed || quantity < 1) return null;
    const line = input.cart.lines.find(
      (candidate) => candidate.product_variant_id === input.product_variant_id,
    );
    if (!line) return null;
    return this.recordAddToCart({
      cartId: input.cart.id,
      productId: line.product_id,
      productVariantId: line.product_variant_id,
      quantity,
      priceMinor: line.unit_price_amount_minor,
      valueMinor: line.unit_price_amount_minor * quantity,
      currency: input.cart.currency,
    });
  }

  /** Records checkout initiation from the exact Cart snapshot used by Chaos. */
  recordCheckoutCreation(
    input: EmbeddedCheckoutStart | EmbeddedCheckoutCreation,
  ): string | null {
    return this.recordInitiateCheckout(
      {
        cartId: input.source_cart.id,
        orderNumber: input.checkout.order_number,
        valueMinor: input.source_cart.subtotal_amount_minor,
        currency: input.source_cart.currency,
        items: input.source_cart.lines.map((line) => ({
          productId: line.product_id,
          productVariantId: line.product_variant_id,
          quantity: line.quantity,
          priceMinor: line.unit_price_amount_minor,
        })),
      },
    );
  }

  /**
   * Records a product view. Pass `productVariantId` once the shopper has
   * picked a specific variant (e.g. a color swatch) — otherwise ViewContent's
   * `content_ids` stay at the product level, while AddToCart/Purchase report
   * variant-level ids, breaking Meta's catalog matching for dynamic ads and
   * "viewed but not bought" retargeting.
   */
  viewContent(input: ViewContentAnalyticsInput): string {
    validateMoney(input.priceMinor, input.currency);
    const eventId = this.randomUUID();
    const eventData = viewContentEventData(input);
    this.destinations.pixel("ViewContent", eventId, eventData);
    this.destinations.ga4("view_item", {
      event_id: eventId,
      value: eventData.value,
      currency: eventData.currency,
      items: toGa4Items(eventData.contents),
    });
    return eventId;
  }

  search({ query }: { query: string }): string {
    const eventId = this.randomUUID();
    this.destinations.pixel("Search", eventId, { search_string: query });
    this.destinations.ga4("search", { event_id: eventId, search_term: query });
    return eventId;
  }

  /** Projects a server-confirmed Purchase to browser providers exactly once per Order. */
  recordPurchase(input: PurchaseAnalyticsInput): string | null {
    validateMoney(input.valueMinor, input.currency);
    if (!isUuid(input.orderId))
      throw new TypeError("orderId must be a valid UUID");
    const orderId = input.orderId.toLowerCase();

    try {
      const eventData = purchaseEventData(input);
      let sent = false;
      if (this.destinations.hasPixel) {
        sent =
          this.eventState.recordProviderOnce(
            "meta",
            "purchase",
            orderId,
            () => this.destinations.pixel("Purchase", orderId, eventData),
          ) || sent;
      }
      if (this.destinations.hasGa4) {
        sent =
          this.eventState.recordProviderOnce(
            "ga4",
            "purchase",
            orderId,
            () =>
              this.destinations.ga4("purchase", {
                event_id: orderId,
                transaction_id: orderId,
                value: toMajorUnits(
                  input.ga4ValueMinor ?? input.valueMinor,
                  input.currency,
                ),
                currency: eventData.currency,
                ...(input.taxMinor !== undefined
                  ? { tax: toMajorUnits(input.taxMinor, input.currency) }
                  : {}),
                ...(input.shippingMinor !== undefined
                  ? {
                      shipping: toMajorUnits(
                        input.shippingMinor,
                        input.currency,
                      ),
                    }
                  : {}),
                items: discountedGa4PurchaseItems(input),
              }),
          ) || sent;
      }
      return sent ? orderId : null;
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
  }

  /** Projects a confirmed, paid order without making the caller rebuild event fields. */
  recordConfirmedPurchase(order: ConfirmedPurchaseOrderInput): string | null {
    const input = toPurchaseAnalyticsInput(order);
    return input ? this.recordPurchase(input) : null;
  }
}

/** Resolves and normalizes the UUID shared by browser commerce projections. */
function canonicalEventId(explicit: string | undefined, fallback: string): string {
  const resolved = explicit ?? fallback;
  if (!isUuid(resolved)) {
    throw new TypeError("commerce event_id must be a valid UUID");
  }
  return resolved.toLowerCase();
}

function isNonEmptyText(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function validateMoney(valueMinor: number, currency: string): void {
  if (!Number.isSafeInteger(valueMinor) || valueMinor < 0) {
    throw new RangeError("valueMinor must be a non-negative safe integer");
  }
  if (!/^[A-Za-z]{3}$/.test(currency))
    throw new TypeError("currency must be an ISO 4217 code");
}

function validateCommerceItem(item: AnalyticsCommerceItem): void {
  if (!isUuid(item.productId))
    throw new TypeError("productId must be a valid UUID");
  if (!isUuid(item.productVariantId))
    throw new TypeError("productVariantId must be a valid UUID");
  if (!Number.isSafeInteger(item.quantity) || item.quantity < 1)
    throw new RangeError("quantity must be a positive safe integer");
  if (!Number.isSafeInteger(item.priceMinor) || item.priceMinor < 0)
    throw new RangeError("priceMinor must be a non-negative safe integer");
}

function toGa4Items(contents: MetaCommerceEventData["contents"]): Array<{
  item_id: string;
  quantity: number;
  price: number;
}> {
  return contents.map((content) => ({
    item_id: content.id,
    quantity: content.quantity,
    price: content.item_price,
  }));
}

/** Allocates an order discount in minor units so item revenue equals GA4 value. */
function discountedGa4PurchaseItems(input: PurchaseAnalyticsInput): Array<{
  item_id: string;
  quantity: number;
  price: number;
  discount: number;
}> {
  const gross = input.items.reduce(
    (sum, item) => sum + BigInt(item.priceMinor) * BigInt(item.quantity),
    0n,
  );
  const discount = gross - BigInt(input.ga4ValueMinor ?? Number(gross));
  if (gross === 0n || discount < 0n || discount > gross) {
    return input.items.map((item) => ({
      item_id: item.productVariantId,
      quantity: item.quantity,
      price: toMajorUnits(item.priceMinor, input.currency),
      discount: 0,
    }));
  }
  let remaining = discount;
  const lastPositive = input.items.reduce(
    (last, item, index) => item.priceMinor > 0 ? index : last,
    -1,
  );
  const result = [] as Array<{
    item_id: string;
    quantity: number;
    price: number;
    discount: number;
  }>;
  input.items.forEach((item, index) => {
    const lineGross = BigInt(item.priceMinor) * BigInt(item.quantity);
    const lineDiscount =
      index === lastPositive ? remaining : (discount * lineGross) / gross;
    remaining -= lineDiscount;
    const each = Number(lineDiscount / BigInt(item.quantity));
    const extra = Number(lineDiscount % BigInt(item.quantity));
    for (const [quantity, unitDiscount] of [
      [extra, each + 1],
      [item.quantity - extra, each],
    ] as Array<[number, number]>) {
      if (quantity === 0) continue;
      result.push({
        item_id: item.productVariantId,
        quantity,
        price: toMajorUnits(item.priceMinor - unitDiscount, input.currency),
        discount: toMajorUnits(unitDiscount, input.currency),
      });
    }
  });
  return result;
}

function isUuid(value: string | null | undefined): boolean {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
      value,
    )
  );
}

function nonEmpty(value: string | undefined): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
