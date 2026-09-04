import { toPurchaseAnalyticsInput } from "./domain.js";
import { fnv1a32 } from "./internal/hash.js";
import { toMajorUnits } from "./money.js";
import type {
  AddToCartAnalyticsInput,
  BrowserAnalyticsEventName,
  CartLineMutation,
  ClientCommerceAnalyticsEventName,
  EmbeddedCheckoutCreation,
  InitiateCheckoutAnalyticsInput,
  OrderLookup,
  PreparedAnalyticsEvent,
  PurchaseAnalyticsInput,
} from "./types.js";

/**
 * First-party behavior collection. Events project directly to the
 * configured browser providers (Meta Pixel, GA4) — there is no chaos-owned
 * ledger or delivery queue; a store that also wants server-side Meta
 * Conversions API delivery uses the separate `@omnip-org/chaos-js/meta-capi`
 * subpath from its own server-side code.
 */

const MAX_ENGAGEMENT_INTERVAL_MS = 60_000;
const MAX_PROPERTIES_BYTES = 32_768;
const MAX_GA4_EVENT_NAME_BYTES = 40;
const MAX_META_BROWSER_ID_LENGTH = 2_048;
const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const COMMERCE_EVENT_NAMES = new Set([
  "add_to_cart",
  "initiate_checkout",
  "purchase",
]);
const CLIENT_COMMERCE_EVENT_NAMES = new Set([
  "add_to_cart",
  "initiate_checkout",
]);

export interface PageViewInput {
  path?: string;
  title?: string;
  referrerDomain?: string;
}

export interface AnalyticsOptions {
  publishableKey: string;
  document?: Document;
  window?: Window & typeof globalThis;
  storage?: Storage;
  sessionStorage?: Storage;
  randomUUID?: () => string;
  now?: () => number;
  /** Monotonic clock used only for elapsed-time measurement. */
  monotonicNow?: () => number;
  setInterval?: typeof setInterval;
  clearInterval?: typeof clearInterval;
  flushIntervalMs?: number;
  providers?: {
    metaPixel?: { pixelId: string };
    ga4?: { measurementId: string };
  };
  /** Starts lifecycle and SPA page tracking. Defaults to true. */
  autoStart?: boolean;
}

export class ChaosStorefrontAnalytics {
  private readonly publishableKey: string;
  private readonly documentRef: Document;
  private readonly windowRef: Window & typeof globalThis;
  private readonly storage?: Storage;
  private readonly sessionStorageRef?: Storage;
  private readonly randomUUID: () => string;
  private readonly now: () => number;
  private readonly monotonicNow: () => number;
  private readonly setIntervalImpl: typeof setInterval;
  private readonly clearIntervalImpl: typeof clearInterval;
  private readonly flushIntervalMs: number;
  private readonly providers: BrowserProviderAdapters;

  private readonly metaFbcStorageKey: string;
  private readonly providerEventStoragePrefix: string;
  private running = false;
  private timer: ReturnType<typeof setInterval> | null = null;
  private currentPageViewEventId: string | null = null;
  private currentPagePath: string | null = null;
  private activeStartedAt: number | null = null;
  private accumulatedActiveMs = 0;
  private readonly onActivityChange = () => this.updateActivityState();
  private readonly onPageHide = () => {
    this.flushViewDuration();
    this.activeStartedAt = null;
  };
  private readonly onPageShow = () => this.updateActivityState();
  private readonly onRouteChange = () => this.pageView();
  private restoreHistory: (() => void) | null = null;

  constructor(options: AnalyticsOptions) {
    if (!options?.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.publishableKey = options.publishableKey;
    this.documentRef = options.document ?? globalThis.document;
    this.windowRef =
      options.window ?? (globalThis as unknown as Window & typeof globalThis);
    this.storage = options.storage ?? this.windowRef?.localStorage;
    this.sessionStorageRef =
      options.sessionStorage ?? this.windowRef?.sessionStorage;
    this.randomUUID =
      options.randomUUID ??
      globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    this.now = options.now ?? Date.now;
    this.monotonicNow =
      options.monotonicNow ??
      globalThis.performance?.now.bind(globalThis.performance) ??
      this.now;
    const setIntervalImpl = options.setInterval ?? globalThis.setInterval;
    const clearIntervalImpl = options.clearInterval ?? globalThis.clearInterval;
    this.setIntervalImpl = setIntervalImpl.bind(globalThis);
    this.clearIntervalImpl = clearIntervalImpl.bind(globalThis);
    this.flushIntervalMs = options.flushIntervalMs ?? 15_000;
    if (!this.randomUUID || !this.documentRef || !this.windowRef) {
      throw new TypeError("randomUUID, document, and window are required");
    }
    if (this.flushIntervalMs < 1_000 || this.flushIntervalMs > 60_000) {
      throw new RangeError("flushIntervalMs must be between 1000 and 60000");
    }
    this.providers = new BrowserProviderAdapters(
      this.windowRef,
      this.documentRef,
      options.providers,
    );

    const storageNamespace = analyticsStorageNamespace(this.publishableKey);
    this.metaFbcStorageKey = `chaos.analytics.${storageNamespace}.meta.fbc.v2`;
    this.providerEventStoragePrefix = `chaos.analytics.${storageNamespace}.provider_event.v1.`;
    this.maintainFbcCookie();
    if (options.autoStart !== false) {
      this.start();
      this.pageView();
    }
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.documentRef.addEventListener(
      "visibilitychange",
      this.onActivityChange,
    );
    this.windowRef.addEventListener("focus", this.onActivityChange);
    this.windowRef.addEventListener("blur", this.onActivityChange);
    this.windowRef.addEventListener("pagehide", this.onPageHide);
    this.windowRef.addEventListener("pageshow", this.onPageShow);
    this.windowRef.addEventListener("popstate", this.onRouteChange);
    this.restoreHistory = observeHistory(this.windowRef, this.onRouteChange);
    this.updateActivityState();
    this.timer = this.setIntervalImpl(() => {
      this.flushViewDuration();
    }, this.flushIntervalMs);
  }

  stop(): void {
    if (this.running) {
      this.snapshotActiveTime();
      this.running = false;
      this.documentRef.removeEventListener(
        "visibilitychange",
        this.onActivityChange,
      );
      this.windowRef.removeEventListener("focus", this.onActivityChange);
      this.windowRef.removeEventListener("blur", this.onActivityChange);
      this.windowRef.removeEventListener("pagehide", this.onPageHide);
      this.windowRef.removeEventListener("pageshow", this.onPageShow);
      this.windowRef.removeEventListener("popstate", this.onRouteChange);
      this.restoreHistory?.();
      this.restoreHistory = null;
      if (this.timer !== null) this.clearIntervalImpl(this.timer);
      this.timer = null;
      this.activeStartedAt = null;
    }
    this.flushViewDuration();
  }

  pageView(input: PageViewInput = {}): string | null {
    const { path, title, referrerDomain } = input;
    this.flushViewDuration();
    this.maintainFbcCookie();
    const resolvedPath = path ?? this.documentRef.location?.pathname ?? "/";
    const resolvedTitle = title ?? nonEmpty(this.documentRef.title);
    const resolvedReferrer =
      referrerDomain ?? referrerHost(this.documentRef.referrer);
    const eventId = this.emit(
      "page_view",
      compact({
        path: resolvedPath,
        title: resolvedTitle,
        referrer_domain: resolvedReferrer,
      }),
    );
    this.accumulatedActiveMs = 0;
    this.currentPageViewEventId = eventId;
    this.currentPagePath = resolvedPath;
    this.activeStartedAt =
      this.isActive() && eventId ? this.monotonicNow() : null;
    return eventId;
  }

  /** Consumes SDK-owned server markers without exposing provider schemas to a storefront. */
  consumeMarkers(root: ParentNode = this.documentRef): void {
    const stripParams = new Set<string>();
    root
      .querySelectorAll<HTMLElement>("[data-chaos-analytics-event]")
      .forEach((marker) => {
        if (marker.dataset.chaosAnalyticsConsumed === "true") return;
        marker.dataset.chaosAnalyticsConsumed = "true";

        const eventName = marker.dataset.chaosAnalyticsEvent;
        const rawProperties = marker.dataset.chaosAnalyticsProperties;
        if (!eventName || !rawProperties) return;

        let properties: unknown;
        try {
          properties = JSON.parse(rawProperties);
        } catch {
          return;
        }
        if (!isRecord(properties)) return;

        try {
          this.recordMarker(eventName, properties);
        } catch {
          // A malformed optional marker must never break the page.
        }

        for (const parameter of marker.dataset.chaosAnalyticsStripParams?.split(",") ?? []) {
          if (parameter) stripParams.add(parameter);
        }
      });

    if (stripParams.size === 0) return;
    const url = new URL(this.windowRef.location.href);
    let changed = false;
    for (const parameter of stripParams) {
      if (url.searchParams.has(parameter)) {
        url.searchParams.delete(parameter);
        changed = true;
      }
    }
    if (changed) this.windowRef.history.replaceState(this.windowRef.history.state, "", url);
  }

  /** Record any store-defined behavior using the common event envelope. */
  track(
    eventName: BrowserAnalyticsEventName,
    properties: Record<string, unknown> = {},
  ): string | null {
    validateEventName(eventName);
    if (isCommerceEvent(eventName)) {
      throw new TypeError(
        `${eventName} must be sent through the analytics SDK after the commerce operation succeeds`,
      );
    }
    return this.emit(eventName, properties, {
      sendToMeta: isBrowserMetaEvent(eventName),
      sendToGa4: !COMMERCE_EVENT_NAMES.has(eventName),
    });
  }

  /**
   * Records a successful cart addition through the common event boundary.
   * `eventId` lets a caller that already sent this event through server-side
   * Meta CAPI (see `@omnip-org/chaos-js/meta-capi`) share the same event ID
   * for Meta's Pixel+CAPI deduplication, instead of minting a second one.
   */
  recordAddToCart(input: AddToCartAnalyticsInput, eventId?: string): string | null {
    validateMoney(input.valueMinor, input.currency);
    if (!isUuid(input.productId))
      throw new TypeError("productId must be a valid UUID");
    if (!isUuid(input.productVariantId))
      throw new TypeError("productVariantId must be a valid UUID");
    if (!Number.isSafeInteger(input.quantity) || input.quantity < 1)
      throw new RangeError("quantity must be a positive safe integer");
    if (!Number.isSafeInteger(input.priceMinor) || input.priceMinor < 0)
      throw new RangeError("priceMinor must be a non-negative safe integer");
    if (input.cartId !== undefined && !isUuid(input.cartId))
      throw new TypeError("cartId must be a valid UUID");

    return this.recordCommerceEvent(
      "add_to_cart",
      {
        ...(input.cartId ? { cart_id: input.cartId } : {}),
        product_id: input.productId,
        product_variant_id: input.productVariantId,
        quantity: input.quantity,
        value_minor: input.valueMinor,
        currency: input.currency.toUpperCase(),
        items: [
          {
            product_id: input.productId,
            product_variant_id: input.productVariantId,
            quantity: input.quantity,
            price_minor: input.priceMinor,
          },
        ],
      },
      eventId,
    );
  }

  /** Records a successful embedded checkout creation. See `recordAddToCart` for `eventId`. */
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
    for (const item of input.items) {
      if (!isUuid(item.productId))
        throw new TypeError("productId must be a valid UUID");
      if (!isUuid(item.productVariantId))
        throw new TypeError("productVariantId must be a valid UUID");
      if (!Number.isSafeInteger(item.quantity) || item.quantity < 1)
        throw new RangeError("quantity must be a positive safe integer");
      if (!Number.isSafeInteger(item.priceMinor) || item.priceMinor < 0)
        throw new RangeError("priceMinor must be a non-negative safe integer");
    }

    return this.recordCommerceEvent(
      "initiate_checkout",
      {
        cart_id: input.cartId,
        order_number: input.orderNumber,
        value_minor: input.valueMinor,
        currency: input.currency.toUpperCase(),
        items: input.items.map((item) => ({
          product_id: item.productId,
          product_variant_id: item.productVariantId,
          quantity: item.quantity,
          price_minor: item.priceMinor,
        })),
      },
      eventId,
    );
  }

  /**
   * Records the increase produced by a successful shared cart mutation.
   * Reuses `input.event_id` when the mutation carries one (set by a
   * server-side helper that already sent this event through Meta CAPI).
   */
  recordCartMutation(input: CartLineMutation): string | null {
    const quantity = input.new_quantity - input.previous_quantity;
    if (input.removed || quantity < 1) return null;
    const line = input.cart.lines.find(
      (candidate) => candidate.product_variant_id === input.product_variant_id,
    );
    if (!line) return null;
    return this.recordAddToCart(
      {
        cartId: input.cart.id,
        productId: line.product_id,
        productVariantId: line.product_variant_id,
        quantity,
        priceMinor: line.unit_price_amount_minor,
        valueMinor: line.unit_price_amount_minor * quantity,
        currency: input.cart.currency,
      },
      input.event_id,
    );
  }

  /**
   * Records checkout initiation from the exact cart snapshot used by Chaos.
   * Reuses `input.event_id` — see `recordCartMutation`.
   */
  recordCheckoutCreation(input: EmbeddedCheckoutCreation): string | null {
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
      input.event_id,
    );
  }

  /**
   * Builds the internal commerce envelope shared by browser providers. It
   * does not project the event; the public high-level methods call it only
   * after the commerce operation succeeds.
   * @internal Used only by the SDK's high-level commerce methods.
   */
  prepareCommerceEvent(
    eventName: ClientCommerceAnalyticsEventName,
    properties: Record<string, unknown> = {},
    eventId?: string,
  ): PreparedAnalyticsEvent {
    validateEventName(eventName);
    const resolvedEventId = eventId ?? this.randomUUID();
    if (!isUuid(resolvedEventId)) {
      throw new TypeError("commerce event_id must be a valid UUID");
    }
    const canonicalEventId = resolvedEventId.toLowerCase();
    const event: PreparedAnalyticsEvent = {
      event_id: canonicalEventId,
      event_name: eventName,
      occurred_at: new Date(this.now()).toISOString(),
      properties: compact(properties),
    };
    validateEventProperties(event.properties);
    return event;
  }

  /**
   * Projects a prepared commerce event to browser providers after the
   * matching business operation succeeds, exactly once per event ID.
   * @internal Used only by the SDK's high-level commerce methods.
   */
  sendCommerceEvent(
    event: PreparedAnalyticsEvent,
    properties: Record<string, unknown> = {},
  ): string | null {
    if (!isClientCommerceEvent(event.event_name)) {
      throw new TypeError(
        "sendCommerceEvent only accepts client commerce events",
      );
    }
    if (!isUuid(event.event_id)) {
      throw new TypeError("commerce event_id must be a valid UUID");
    }
    const merged = compact({ ...event.properties, ...properties });
    validateEventProperties(merged);
    const eventId = event.event_id.toLowerCase();
    return this.projectProviderEventOnce(event.event_name, eventId, merged);
  }

  private recordCommerceEvent(
    eventName: ClientCommerceAnalyticsEventName,
    properties: Record<string, unknown>,
    eventId?: string,
  ): string | null {
    try {
      const event = this.prepareCommerceEvent(eventName, properties, eventId);
      return this.sendCommerceEvent(event);
    } catch {
      // Analytics is optional and must never turn a successful commerce
      // operation into a failed UI action.
      return null;
    }
  }

  viewContent({
    productId,
    productVariantId,
  }: {
    productId: string;
    productVariantId?: string;
  }): string | null {
    return this.emit(
      "view_content",
      compact({
        product_id: productId,
        product_variant_id: productVariantId,
      }),
    );
  }

  search({
    query,
    resultCount,
  }: {
    query: string;
    resultCount?: number;
  }): string | null {
    return this.emit(
      "search",
      compact({ query, result_count: resultCount }),
    );
  }

  /** Projects a server-confirmed Purchase to browser providers exactly once per Order. */
  purchase(input: PurchaseAnalyticsInput): string | null {
    validateMoney(input.valueMinor, input.currency);
    const currency = input.currency.toUpperCase();
    if (!/^[A-Z]{3}$/.test(currency))
      throw new TypeError("currency must be an ISO 4217 code");
    if (!isUuid(input.orderId))
      throw new TypeError("orderId must be a valid UUID");
    const orderId = input.orderId.toLowerCase();
    return this.projectProviderEventOnce(
      "purchase",
      orderId,
      compact({
        order_id: orderId,
        value_minor: input.valueMinor,
        currency,
        items: input.items.map((item) => ({
          product_id: item.productId,
          product_variant_id: item.productVariantId,
          quantity: item.quantity,
          price_minor: item.priceMinor,
        })),
      }),
    );
  }

  /** Projects a confirmed, paid order without making the caller rebuild event fields. */
  recordConfirmedOrder(order: Pick<OrderLookup, "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines">): string | null {
    const input = toPurchaseAnalyticsInput(order);
    return input ? this.purchase(input) : null;
  }

  private recordMarker(
    eventName: string,
    properties: Record<string, unknown>,
  ): void {
    if (eventName === "view_content") {
      const productId = properties.product_id;
      if (typeof productId !== "string") return;
      const productVariantId = properties.product_variant_id;
      this.viewContent({
        productId,
        ...(typeof productVariantId === "string" ? { productVariantId } : {}),
      });
      return;
    }
    if (eventName === "search") {
      const query = properties.query;
      if (typeof query === "string" && query.trim()) {
        this.search({ query: query.trim() });
      }
      return;
    }
    this.track(eventName, properties);
  }

  private projectProviderEventOnce(
    eventName: string,
    eventId: string,
    properties: Record<string, unknown>,
  ): string | null {
    const storageKey = `${this.providerEventStoragePrefix}${eventName}.${eventId}`;
    if (this.storage?.getItem(storageKey)) return null;
    try {
      this.providers.track(eventName, eventId, properties);
    } catch {
      // Browser provider failures are best-effort; keep the event ID stable
      // for a future retry.
    }
    this.storage?.setItem(storageKey, new Date(this.now()).toISOString());
    return eventId;
  }

  flushViewDuration(): number {
    this.snapshotActiveTime();
    if (!this.currentPageViewEventId) {
      this.accumulatedActiveMs = 0;
      return 0;
    }
    let emitted = 0;
    while (this.accumulatedActiveMs >= 1) {
      const activeMilliseconds = Math.min(
        Math.floor(this.accumulatedActiveMs),
        MAX_ENGAGEMENT_INTERVAL_MS,
      );
      this.emit(
        "view_duration",
        compact({
          page_view_event_id: this.currentPageViewEventId,
          path: this.currentPagePath ?? undefined,
          active_milliseconds: activeMilliseconds,
        }),
        { sendToMeta: false },
      );
      this.accumulatedActiveMs -= activeMilliseconds;
      emitted += activeMilliseconds;
    }
    return emitted;
  }

  /** Projects one generic event straight to the configured browser providers. */
  private emit(
    eventName: string,
    properties: Record<string, unknown>,
    options: { sendToMeta?: boolean; sendToGa4?: boolean } = {},
  ): string | null {
    validateEventName(eventName);
    validateEventProperties(properties);
    const eventId = this.randomUUID();
    try {
      const providerOptions: { meta?: boolean; ga4?: boolean } = {};
      if (options.sendToMeta !== undefined)
        providerOptions.meta = options.sendToMeta;
      if (options.sendToGa4 !== undefined)
        providerOptions.ga4 = options.sendToGa4;
      this.providers.track(eventName, eventId, properties, {
        ...providerOptions,
      });
    } catch {
      // Browser provider failures must not turn a successful API operation
      // into a failed commerce operation.
    }
    return eventId;
  }

  /**
   * Keeps the `_fbc` cookie in sync with a `fbclid` on the current URL, so a
   * server-side Meta CAPI call later in the same visit can read a fresh
   * value from the request's cookies. A no-op without a current `fbclid` —
   * an existing valid cookie is left untouched.
   */
  private maintainFbcCookie(): void {
    const fbclid = boundedText(
      new URLSearchParams(this.documentRef.location?.search ?? "").get(
        "fbclid",
      ) ?? undefined,
      MAX_META_BROWSER_ID_LENGTH,
    );
    if (!fbclid) return;
    const cookieFbc = readCookie(this.documentRef, "_fbc");
    const fbc = this.resolveFbc(fbclid);
    if (fbc && fbc !== cookieFbc) writeCookie(this.documentRef, "_fbc", fbc);
  }

  private updateActivityState(): void {
    const active = this.isActive();
    this.snapshotActiveTime();
    this.activeStartedAt = active ? this.monotonicNow() : null;
  }

  private snapshotActiveTime(): void {
    if (this.activeStartedAt === null) return;
    const now = this.monotonicNow();
    this.accumulatedActiveMs += Math.max(0, now - this.activeStartedAt);
    this.activeStartedAt = now;
  }

  private isActive(): boolean {
    return (
      this.documentRef.visibilityState === "visible" &&
      this.documentRef.hasFocus()
    );
  }

  /**
   * Pairs a `fbclid` with a stable `fb.1.<first-seen-timestamp>.<fbclid>`
   * value so the timestamp reflects the original click, not a later reload.
   */
  private resolveFbc(fbclid: string | undefined): string | undefined {
    if (!fbclid || /\s/.test(fbclid)) return undefined;
    const stored = readStoredJson(
      this.sessionStorageRef,
      this.metaFbcStorageKey,
    );
    if (
      stored &&
      typeof stored === "object" &&
      !Array.isArray(stored) &&
      (stored as Record<string, unknown>).fbclid === fbclid &&
      typeof (stored as Record<string, unknown>).fbc === "string" &&
      validFbc((stored as Record<string, string>).fbc)
    ) {
      return (stored as Record<string, string>).fbc;
    }
    const fbc = `fb.1.${Math.floor(this.now())}.${fbclid}`;
    if (!validFbc(fbc)) return undefined;
    writeStoredJson(this.sessionStorageRef, this.metaFbcStorageKey, {
      fbclid,
      fbc,
    });
    return fbc;
  }
}

type ProviderOptions = AnalyticsOptions["providers"];
type GtagDataLayerEntry = unknown[] | IArguments;
type ProviderFunction = ((...args: unknown[]) => void) & {
  callMethod?: (...args: unknown[]) => void;
  queue?: unknown[][];
  loaded?: boolean;
  version?: string;
};
type ProviderWindow = Window &
  typeof globalThis & {
    dataLayer?: GtagDataLayerEntry[];
    gtag?: (...args: unknown[]) => void;
    fbq?: ProviderFunction;
    _fbq?: ProviderFunction;
  };

class BrowserProviderAdapters {
  private readonly windowRef: ProviderWindow;
  private readonly documentRef: Document;
  private readonly options: ProviderOptions;
  private metaStarted = false;
  private ga4Started = false;

  constructor(
    windowRef: Window & typeof globalThis,
    documentRef: Document,
    options: ProviderOptions,
  ) {
    this.windowRef = windowRef as ProviderWindow;
    this.documentRef = documentRef;
    this.options = options;
    validateProviderOptions(options);
    if (this.options?.ga4) this.startGa4();
    if (this.options?.metaPixel) this.startMeta();
  }

  track(
    eventName: string,
    eventId: string,
    properties: Record<string, unknown>,
    options: { meta?: boolean; ga4?: boolean } = {},
  ): void {
    if ((options.meta ?? true) && this.metaStarted && isMetaEvent(eventName)) {
      const mapped = metaEvent(eventName, properties);
      this.windowRef.fbq?.("track", mapped.name, mapped.parameters, {
        eventID: eventId,
      });
    }
    if ((options.ga4 ?? true) && this.ga4Started) {
      const mapped = ga4Event(eventName, eventId, properties);
      if (isValidGa4EventName(mapped.name)) {
        this.windowRef.gtag?.("event", mapped.name, mapped.parameters);
      }
    }
  }

  private startMeta(): void {
    if (this.metaStarted || !this.options?.metaPixel) return;
    this.metaStarted = true;
    if (!this.windowRef.fbq) {
      const fbq: ProviderFunction = (...args: unknown[]) => {
        if (fbq.callMethod) fbq.callMethod(...args);
        else fbq.queue?.push(args);
      };
      fbq.queue = [];
      fbq.loaded = true;
      fbq.version = "2.0";
      this.windowRef.fbq = fbq;
      this.windowRef._fbq = fbq;
      loadProviderScript(
        this.documentRef,
        "chaos-meta-pixel",
        "https://connect.facebook.net/en_US/fbevents.js",
      );
    }
    this.windowRef.fbq("init", this.options.metaPixel.pixelId);
  }

  private startGa4(): void {
    if (this.ga4Started || !this.options?.ga4) return;
    this.ga4Started = true;
    const dataLayer = (this.windowRef.dataLayer ??= []);
    this.windowRef.gtag ??= function gtag() {
      dataLayer.push(arguments);
    };
    this.windowRef.gtag("js", new Date());
    this.windowRef.gtag("config", this.options.ga4.measurementId, {
      send_page_view: false,
    });
    loadProviderScript(
      this.documentRef,
      "chaos-google-tag",
      `https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(this.options.ga4.measurementId)}`,
    );
  }
}

function validateProviderOptions(options: ProviderOptions): void {
  if (options?.metaPixel && !/^[0-9]{5,32}$/.test(options.metaPixel.pixelId)) {
    throw new TypeError("providers.metaPixel.pixelId must contain 5-32 digits");
  }
  if (options?.ga4 && !/^G-[A-Z0-9]{4,20}$/.test(options.ga4.measurementId)) {
    throw new TypeError(
      "providers.ga4.measurementId must be a GA4 measurement ID",
    );
  }
}

function isNonEmptyText(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function loadProviderScript(
  documentRef: Document,
  id: string,
  source: string,
): void {
  if (
    documentRef.getElementById?.(id) ||
    !documentRef.createElement ||
    !documentRef.head
  )
    return;
  const script = documentRef.createElement("script");
  script.id = id;
  script.async = true;
  script.src = source;
  documentRef.head.appendChild(script);
}

function metaEvent(
  eventName: string,
  properties: Record<string, unknown>,
): { name: string; parameters: Record<string, unknown> } {
  const names: Record<string, string> = {
    page_view: "PageView",
    view_content: "ViewContent",
    search: "Search",
    add_to_cart: "AddToCart",
    initiate_checkout: "InitiateCheckout",
    purchase: "Purchase",
  };
  const contentIds = commerceItemIds(properties);
  return {
    name: names[eventName] ?? eventName,
    parameters: compact({
      content_ids: contentIds.length > 0 ? contentIds : undefined,
      content_type: contentIds.length > 0 ? "product" : undefined,
      search_string: properties.query,
      quantity: properties.quantity,
      page_path: properties.path,
      value: providerValue(properties.value_minor, properties.currency),
      currency: properties.currency,
      contents: providerItems(properties.items, properties.currency),
      num_items: providerItemCount(properties.items) ?? properties.quantity,
    }),
  };
}

function isMetaEvent(eventName: string): boolean {
  return [
    "page_view",
    "view_content",
    "search",
    "add_to_cart",
    "initiate_checkout",
    "purchase",
  ].includes(eventName);
}

function isBrowserMetaEvent(eventName: string): boolean {
  return ["page_view", "view_content", "search"].includes(eventName);
}

function isCommerceEvent(eventName: string): boolean {
  return COMMERCE_EVENT_NAMES.has(eventName);
}

function isClientCommerceEvent(
  eventName: string,
): eventName is ClientCommerceAnalyticsEventName {
  return CLIENT_COMMERCE_EVENT_NAMES.has(eventName);
}

function ga4Event(
  eventName: string,
  eventId: string,
  properties: Record<string, unknown>,
): { name: string; parameters: Record<string, unknown> } {
  const names: Record<string, string> = {
    view_content: "view_item",
    add_to_cart: "add_to_cart",
    initiate_checkout: "begin_checkout",
    purchase: "purchase",
  };
  const itemId = commerceItemId(properties);
  return {
    name: names[eventName] ?? eventName,
    parameters: compact({
      event_id: eventId,
      page_path: properties.path,
      page_title: properties.title,
      search_term: properties.query,
      engagement_time_msec: properties.active_milliseconds,
      transaction_id: properties.order_id ?? properties.order_number,
      value: providerValue(properties.value_minor, properties.currency),
      currency: properties.currency,
      items:
        ga4Items(properties.items, properties.currency) ??
        (itemId
          ? [compact({ item_id: itemId, quantity: properties.quantity })]
          : undefined),
    }),
  };
}

function providerValue(
  valueMinor: unknown,
  currency: unknown,
): number | undefined {
  if (typeof valueMinor !== "number" || typeof currency !== "string")
    return undefined;
  if (!Number.isSafeInteger(valueMinor) || valueMinor < 0) return undefined;
  try {
    return toMajorUnits(valueMinor, currency);
  } catch {
    return undefined;
  }
}

function validateMoney(valueMinor: number, currency: string): void {
  if (!Number.isSafeInteger(valueMinor) || valueMinor < 0) {
    throw new RangeError("valueMinor must be a non-negative safe integer");
  }
  if (!/^[A-Za-z]{3}$/.test(currency))
    throw new TypeError("currency must be an ISO 4217 code");
}

function providerItems(items: unknown, currency: unknown): unknown {
  if (!Array.isArray(items)) return undefined;
  return items.map((item) => {
    const value = item as Record<string, unknown>;
    return compact({
      id: value.product_variant_id ?? value.product_id,
      quantity: value.quantity,
      item_price: providerValue(value.price_minor, currency),
    });
  });
}

function providerItemCount(items: unknown): number | undefined {
  if (!Array.isArray(items)) return undefined;
  const quantities = items.map(
    (item) => (item as Record<string, unknown>).quantity,
  );
  if (!quantities.every((quantity) => typeof quantity === "number"))
    return undefined;
  return quantities.reduce(
    (total, quantity) => total + (quantity as number),
    0,
  );
}

function ga4Items(items: unknown, currency: unknown): unknown {
  if (!Array.isArray(items)) return undefined;
  return items.map((item) => {
    const value = item as Record<string, unknown>;
    return compact({
      item_id: value.product_variant_id ?? value.product_id,
      quantity: value.quantity,
      price: providerValue(value.price_minor, currency),
    });
  });
}

function commerceItemId(properties: Record<string, unknown>): unknown {
  return properties.product_variant_id ?? properties.product_id;
}

function commerceItemIds(properties: Record<string, unknown>): string[] {
  if (Array.isArray(properties.items)) {
    const ids = properties.items
      .map((item) => {
        const value = item as Record<string, unknown>;
        return value.product_variant_id ?? value.product_id;
      })
      .filter(
        (itemId): itemId is string =>
          typeof itemId === "string" && itemId.length > 0,
      );
    if (ids.length > 0) return ids;
  }
  const itemId = commerceItemId(properties);
  return typeof itemId === "string" && itemId.length > 0 ? [itemId] : [];
}

function analyticsStorageNamespace(publishableKey: string): string {
  return fnv1a32(publishableKey).toString(36);
}

function observeHistory(
  windowRef: Window & typeof globalThis,
  listener: () => void,
): () => void {
  const history = windowRef.history;
  if (!history?.pushState || !history?.replaceState) return () => {};
  let state = historyObservers.get(history);
  if (!state) {
    const pushState = history.pushState.bind(history);
    const replaceState = history.replaceState.bind(history);
    const listeners = new Set<() => void>();
    const notify = () => {
      for (const registeredListener of [...listeners]) registeredListener();
    };
    const pushWrapper: History["pushState"] = (...args) => {
      pushState(...args);
      notify();
    };
    const replaceWrapper: History["replaceState"] = (...args) => {
      replaceState(...args);
      notify();
    };
    state = { listeners, pushState, replaceState, pushWrapper, replaceWrapper };
    historyObservers.set(history, state);
    history.pushState = pushWrapper;
    history.replaceState = replaceWrapper;
  }
  state.listeners.add(listener);
  return () => {
    const current = historyObservers.get(history);
    if (!current) return;
    current.listeners.delete(listener);
    if (current.listeners.size === 0) {
      if (history.pushState === current.pushWrapper)
        history.pushState = current.pushState;
      if (history.replaceState === current.replaceWrapper)
        history.replaceState = current.replaceState;
      historyObservers.delete(history);
    }
  };
}

interface HistoryObserverState {
  listeners: Set<() => void>;
  pushState: History["pushState"];
  replaceState: History["replaceState"];
  pushWrapper: History["pushState"];
  replaceWrapper: History["replaceState"];
}

const historyObservers = new WeakMap<History, HistoryObserverState>();

export function createStorefrontAnalytics(
  options: AnalyticsOptions,
): ChaosStorefrontAnalytics {
  return new ChaosStorefrontAnalytics(options);
}

function isUuid(value: string | null | undefined): boolean {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
      value,
    )
  );
}

function referrerHost(value: string | undefined): string | undefined {
  if (!value) return undefined;
  try {
    return new URL(value).host || undefined;
  } catch {
    return undefined;
  }
}

function readCookie(documentRef: Document, name: string): string | undefined {
  const cookie = documentRef.cookie;
  if (typeof cookie !== "string") return undefined;
  const value = cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`));
  if (!value) return undefined;
  const raw = value.slice(name.length + 1);
  if (!raw) return undefined;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

function writeCookie(documentRef: Document, name: string, value: string): void {
  try {
    const secure =
      documentRef.location?.protocol === "https:" ? "; Secure" : "";
    documentRef.cookie = `${name}=${encodeURIComponent(value)}; Max-Age=${META_FBC_MAX_AGE_SECONDS}; Path=/; SameSite=Lax${secure}`;
  } catch {
    // Cookie storage is optional; the event still carries the matching value.
  }
}

function nonEmpty(value: string | undefined): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;

function boundedText(
  value: string | undefined,
  maximumLength: number,
): string | undefined {
  return typeof value === "string" &&
    value.length >= 1 &&
    value.length <= maximumLength &&
    !CONTROL_CHARACTERS.test(value)
    ? value
    : undefined;
}

function compact<T extends Record<string, unknown>>(
  value: T,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readStoredJson(storage: Storage | undefined, key: string): unknown {
  try {
    const value = storage?.getItem(key);
    return value ? JSON.parse(value) : undefined;
  } catch {
    return undefined;
  }
}

function writeStoredJson(
  storage: Storage | undefined,
  key: string,
  value: unknown,
): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage is optional; the fbc pairing is best-effort.
  }
}

function validateEventName(eventName: string): void {
  if (!isValidEventName(eventName)) {
    throw new TypeError(
      "eventName must be 1-64 lowercase snake_case characters",
    );
  }
}

function isValidEventName(value: unknown): value is string {
  return typeof value === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(value);
}

function validFbc(value: string | undefined): value is string {
  if (!value || value.length > MAX_META_BROWSER_ID_LENGTH) return false;
  const match = /^fb\.\d+\.(\d{13})\.[^\s]+$/.exec(value);
  return match !== null && Number.isSafeInteger(Number(match[1]));
}

function validateEventProperties(properties: Record<string, unknown>): void {
  if (!isValidEventProperties(properties)) {
    throw new TypeError(
      "event properties must be a JSON object no larger than 32768 bytes",
    );
  }
}

function isValidEventProperties(
  value: unknown,
): value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  try {
    const serialized = JSON.stringify(value);
    return (
      typeof serialized === "string" &&
      new TextEncoder().encode(serialized).byteLength <= MAX_PROPERTIES_BYTES
    );
  } catch {
    return false;
  }
}

function isValidGa4EventName(eventName: string): boolean {
  return (
    eventName.length <= MAX_GA4_EVENT_NAME_BYTES &&
    /^[A-Za-z][A-Za-z0-9_]*$/.test(eventName)
  );
}
