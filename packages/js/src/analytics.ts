import { ChaosApiError, throwForResponse } from "./errors.js";
import type {
  AddToCartAnalyticsInput,
  AnalyticsCollectionResult,
  AnalyticsPurchaseItem,
  BrowserAnalyticsEventName,
  ClientCommerceAnalyticsEventName,
  InitiateCheckoutAnalyticsInput,
  PreparedAnalyticsEvent,
} from "./types.js";

/**
 * First-party behavior collection. Events use one stable envelope and keep
 * event-specific values inside properties so the collector can evolve without
 * a database migration for every new behavior.
 */

const MAX_BATCH_SIZE = 20;
const MAX_QUEUE_SIZE = 100;
const MAX_ENGAGEMENT_INTERVAL_MS = 60_000;
const MAX_QUEUE_AGE_MS = 23 * 60 * 60 * 1_000;
const MAX_PROPERTIES_BYTES = 32_768;
const MAX_GA4_EVENT_NAME_BYTES = 40;
const MAX_META_BROWSER_ID_LENGTH = 2_048;
const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const MAX_UTM_VALUE_LENGTH = 2_048;
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
  /** Returns the signed shopper identity used to associate events with commerce activity. */
  getShopperToken: () => string | Promise<string>;
  endpoint?: string;
  fetch?: typeof fetch;
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

interface QueuedEvent {
  event_id: string;
  event_name: string;
  occurred_at: string;
  properties: Record<string, unknown>;
}

interface TrafficTouchpoint {
  source?: string;
  medium?: string;
  campaign?: string;
  campaign_id?: string;
  term?: string;
  content?: string;
  referrer_domain?: string;
  fbclid?: string;
  gclid?: string;
}

interface TrafficAttribution {
  first: TrafficTouchpoint;
  session: TrafficTouchpoint;
  last_non_direct?: TrafficTouchpoint;
}

export class ChaosStorefrontAnalytics {
  private readonly endpoint: string;
  private readonly publishableKey: string;
  private readonly getShopperToken: () => string | Promise<string>;
  private readonly fetchImpl: typeof fetch;
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

  private sessionId: string;
  private readonly sessionStorageKey: string;
  private readonly queueStorageKey: string;
  private readonly firstTouchStorageKey: string;
  private readonly lastNonDirectStorageKey: string;
  private readonly sessionTouchStorageKey: string;
  private readonly metaFbcStorageKey: string;
  private readonly providerEventStoragePrefix: string;
  private traffic: TrafficAttribution | undefined;
  private queue: QueuedEvent[] = [];
  private inFlight: Promise<AnalyticsCollectionResult | null> | null = null;
  private running = false;
  private timer: ReturnType<typeof setInterval> | null = null;
  private currentPageViewEventId: string | null = null;
  private activeStartedAt: number | null = null;
  private accumulatedActiveMs = 0;
  private readonly onActivityChange = () => this.updateActivityState();
  private readonly onPageHide = () => {
    this.flushViewDuration();
    this.activeStartedAt = null;
    void this.flush({ keepalive: true }).catch(() => {});
  };
  private readonly onPageShow = () => this.updateActivityState();
  private readonly onRouteChange = () => this.pageView();
  private restoreHistory: (() => void) | null = null;

  constructor(options: AnalyticsOptions) {
    if (!options?.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.endpoint = options.endpoint ?? "/storefront/v1/analytics/events";
    this.publishableKey = options.publishableKey;
    this.getShopperToken = options.getShopperToken;
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
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
    if (
      !this.fetchImpl ||
      !this.randomUUID ||
      !this.documentRef ||
      !this.windowRef
    ) {
      throw new TypeError(
        "fetch, randomUUID, document, and window are required",
      );
    }
    if (this.flushIntervalMs < 1_000 || this.flushIntervalMs > 60_000) {
      throw new RangeError("flushIntervalMs must be between 1000 and 60000");
    }
    this.providers = new BrowserProviderAdapters(
      this.windowRef,
      this.documentRef,
      options.providers,
    );

    const storageNamespace = analyticsStorageNamespace(
      this.endpoint,
      this.publishableKey,
    );
    this.queueStorageKey = `chaos.analytics.${storageNamespace}.queue.v2`;
    this.sessionStorageKey = `chaos.analytics.${storageNamespace}.session_id`;
    this.firstTouchStorageKey = `chaos.analytics.${storageNamespace}.traffic.first.v1`;
    this.lastNonDirectStorageKey = `chaos.analytics.${storageNamespace}.traffic.last_non_direct.v1`;
    this.sessionTouchStorageKey = `chaos.analytics.${storageNamespace}.traffic.session.v1`;
    this.metaFbcStorageKey = `chaos.analytics.${storageNamespace}.meta.fbc.v2`;
    this.providerEventStoragePrefix = `chaos.analytics.${storageNamespace}.provider_event.v1.`;
    this.sessionId = this.randomUUID();
    this.enableCollectionStorage();
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
      void this.flush().catch(() => {});
    }, this.flushIntervalMs);
  }

  async stop(): Promise<void> {
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
    await this.flush({ keepalive: true });
  }

  pageView(input: PageViewInput = {}): string | null {
    const { path, title, referrerDomain } = input;
    this.flushViewDuration();
    const resolvedPath = path ?? this.documentRef.location?.pathname ?? "/";
    const resolvedTitle = title ?? nonEmpty(this.documentRef.title);
    const resolvedReferrer =
      referrerDomain ?? referrerHost(this.documentRef.referrer);
    const eventId = this.enqueue(
      "page_view",
      compact({
        path: resolvedPath,
        title: resolvedTitle,
        referrer_domain: resolvedReferrer,
      }),
    );
    this.accumulatedActiveMs = 0;
    this.currentPageViewEventId = eventId;
    this.activeStartedAt =
      this.isActive() && eventId ? this.monotonicNow() : null;
    return eventId;
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
    return this.enqueue(eventName, properties, {
      sendToMeta: isBrowserMetaEvent(eventName),
      sendToGa4: !COMMERCE_EVENT_NAMES.has(eventName),
    });
  }

  /** Records a successful cart addition through the common event boundary. */
  recordAddToCart(input: AddToCartAnalyticsInput): string | null {
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

    return this.recordCommerceEvent("add_to_cart", {
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
    });
  }

  /** Records a successful embedded checkout creation. */
  recordInitiateCheckout(input: InitiateCheckoutAnalyticsInput): string | null {
    validateMoney(input.valueMinor, input.currency);
    if (!isUuid(input.cartId))
      throw new TypeError("cartId must be a valid UUID");
    if (!isUuid(input.orderId))
      throw new TypeError("orderId must be a valid UUID");
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

    return this.recordCommerceEvent("initiate_checkout", {
      cart_id: input.cartId,
      order_id: input.orderId,
      value_minor: input.valueMinor,
      currency: input.currency.toUpperCase(),
      items: input.items.map((item) => ({
        product_id: item.productId,
        product_variant_id: item.productVariantId,
        quantity: item.quantity,
        price_minor: item.priceMinor,
      })),
    });
  }

  /**
   * Creates the internal commerce envelope shared by browser providers and the
   * common analytics endpoint. It does not enqueue or project the event; the
   * public high-level methods call it only after the commerce operation
   * succeeds.
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
      properties: this.contextualProperties(properties),
    };
    validateEventProperties(event.properties);
    return event;
  }

  /**
   * Records a prepared commerce event through the common analytics endpoint
   * after the matching business operation succeeds. The prepared attribution
   * context is always retained; the optional properties are values returned or
   * derived from that successful operation. Browser providers receive the same
   * event ID immediately while the first-party event is queued for delivery.
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
    const contextual = event.properties;
    const merged = compact({
      ...contextual,
      ...properties,
      _meta: contextual._meta,
      session_id: contextual.session_id,
      traffic: contextual.traffic,
    });
    validateEventProperties(merged);
    const eventId = event.event_id.toLowerCase();
    this.queue.push({
      event_id: eventId,
      event_name: event.event_name,
      occurred_at: event.occurred_at,
      properties: merged,
    });
    this.projectProviderEventOnce(event.event_name, eventId, merged);
    this.trimQueue();
    this.persistQueue();
    void this.flush().catch(() => {});
    return eventId;
  }

  private recordCommerceEvent(
    eventName: ClientCommerceAnalyticsEventName,
    properties: Record<string, unknown>,
  ): string | null {
    try {
      const event = this.prepareCommerceEvent(eventName, properties);
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
    return this.enqueue(
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
    return this.enqueue(
      "search",
      compact({ query, result_count: resultCount }),
    );
  }

  /** Projects a server-confirmed Purchase to browser providers exactly once per Order. */
  purchase(input: {
    orderId: string;
    valueMinor: number;
    currency: string;
    items: AnalyticsPurchaseItem[];
  }): string | null {
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
      this.contextualProperties({
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
      // Browser provider failures are best-effort; keep the first-party
      // conversion available to the collector.
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
      this.enqueue(
        "view_duration",
        {
          page_view_event_id: this.currentPageViewEventId,
          active_milliseconds: activeMilliseconds,
        },
        { sendToMeta: false },
      );
      this.accumulatedActiveMs -= activeMilliseconds;
      emitted += activeMilliseconds;
    }
    return emitted;
  }

  flush(
    options: { keepalive?: boolean } = {},
  ): Promise<AnalyticsCollectionResult | null> {
    if (this.inFlight) return this.inFlight;
    this.pruneExpiredQueue();
    if (this.queue.length === 0) return Promise.resolve(null);
    this.inFlight = this.drainQueue(Boolean(options.keepalive)).finally(() => {
      this.inFlight = null;
    });
    return this.inFlight;
  }

  private async drainQueue(
    keepalive: boolean,
  ): Promise<AnalyticsCollectionResult | null> {
    let result: AnalyticsCollectionResult | null = null;
    while (this.queue.length > 0) {
      const batch = this.queue.splice(0, MAX_BATCH_SIZE);
      result = mergeCollectionResults(
        result,
        await this.sendBatch(batch, keepalive),
      );
    }
    return result;
  }

  private async sendBatch(
    batch: QueuedEvent[],
    keepalive: boolean,
  ): Promise<AnalyticsCollectionResult | null> {
    try {
      const response = await this.fetchImpl(this.endpoint, {
        method: "POST",
        headers: {
          authorization: `Bearer ${this.publishableKey}`,
          "content-type": "application/json",
          "x-chaos-shopper-token": await this.getShopperToken(),
        },
        body: JSON.stringify({ events: batch }),
        keepalive,
      });
      if (!response.ok) {
        await throwForResponse(response);
      }
      this.persistQueue();
      const envelope = (await response.json()) as {
        data: AnalyticsCollectionResult;
      };
      return envelope.data;
    } catch (error) {
      if (isSplittableClientError(error) && batch.length > 1) {
        const splitAt = Math.ceil(batch.length / 2);
        const first = await this.sendBatch(batch.slice(0, splitAt), keepalive);
        const second = await this.sendBatch(batch.slice(splitAt), keepalive);
        return mergeCollectionResults(first, second);
      }
      if (isPermanentClientError(error)) {
        // A client-side validation error belongs to this event batch. Drop
        // only the offending single event after bisection, so valid events in
        // the same batch are not lost with it.
        this.persistQueue();
        return null;
      }
      this.queue.unshift(...batch);
      this.trimQueue();
      this.persistQueue();
      throw error;
    }
  }

  private enqueue(
    eventName: string,
    properties: Record<string, unknown>,
    options: { sendToMeta?: boolean; sendToGa4?: boolean } = {},
  ): string | null {
    validateEventName(eventName);
    const eventId = this.randomUUID();
    const event: QueuedEvent = {
      event_id: eventId,
      event_name: eventName,
      occurred_at: new Date(this.now()).toISOString(),
      properties: this.contextualProperties(properties),
    };
    validateEventProperties(event.properties);
    this.queue.push(event);
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
    this.trimQueue();
    this.persistQueue();
    if (this.queue.length >= MAX_BATCH_SIZE) {
      void this.flush().catch(() => {});
    }
    return eventId;
  }

  private contextualProperties(
    properties: Record<string, unknown>,
  ): Record<string, unknown> {
    return compact({
      ...properties,
      _meta: this.metaContext(),
      session_id: this.sessionId,
      ...(this.traffic ? { traffic: this.traffic } : {}),
    });
  }

  private trimQueue(): void {
    if (this.queue.length > MAX_QUEUE_SIZE) {
      const removedPageViews = new Set(
        this.queue
          .splice(0, this.queue.length - MAX_QUEUE_SIZE)
          .filter((event) => event.event_name === "page_view")
          .map((event) => event.event_id),
      );
      if (removedPageViews.size > 0) {
        this.queue = this.queue.filter(
          (event) =>
            event.event_name !== "view_duration" ||
            !removedPageViews.has(String(event.properties.page_view_event_id)),
        );
      }
    }
  }

  private persistQueue(): void {
    writeStoredJson(this.sessionStorageRef, this.queueStorageKey, this.queue);
  }

  private enableCollectionStorage(): void {
    this.sessionId = persistentIdentifier(
      this.sessionStorageRef,
      this.sessionStorageKey,
      this.randomUUID,
    );
    this.restoreQueue();
    this.resolveTraffic();
  }

  private restoreQueue(): void {
    if (this.queue.length > 0) return;
    const restored = readStoredJson(
      this.sessionStorageRef,
      this.queueStorageKey,
    );
    if (Array.isArray(restored)) {
      this.queue = restored.filter(validQueuedEvent).slice(-MAX_QUEUE_SIZE);
      this.pruneExpiredQueue();
    }
  }

  private pruneExpiredQueue(): void {
    const oldestAccepted = this.now() - MAX_QUEUE_AGE_MS;
    this.queue = this.queue.filter(
      (event) => Date.parse(event.occurred_at) >= oldestAccepted,
    );
    this.persistQueue();
  }

  private resolveTraffic(): void {
    const captured = captureTrafficTouchpoint(
      this.documentRef.location?.search,
      this.documentRef.referrer,
    );
    const storedSession = readTrafficTouchpoint(
      this.sessionStorageRef,
      this.sessionTouchStorageKey,
    );
    const session = storedSession
      ? (compact({
          ...storedSession,
          fbclid: captured.fbclid,
          gclid: captured.gclid,
        }) as TrafficTouchpoint)
      : captured;
    writeStoredJson(
      this.sessionStorageRef,
      this.sessionTouchStorageKey,
      session,
    );
    const first =
      readTrafficTouchpoint(this.storage, this.firstTouchStorageKey) ?? session;
    writeStoredJson(this.storage, this.firstTouchStorageKey, first);
    const existingLast = readTrafficTouchpoint(
      this.storage,
      this.lastNonDirectStorageKey,
    );
    const lastNonDirect = isNonDirectTouchpoint(session)
      ? session
      : existingLast;
    if (lastNonDirect)
      writeStoredJson(
        this.storage,
        this.lastNonDirectStorageKey,
        lastNonDirect,
      );
    this.traffic = {
      first,
      session,
      ...(lastNonDirect ? { last_non_direct: lastNonDirect } : {}),
    };
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

  private metaContext(): Record<string, unknown> {
    // Only build fbc from a click observed in the current landing/session.
    // Reusing a historical last-non-direct fbclid with a new timestamp would
    // create an invalid click context. A current landing fbclid takes
    // precedence over a previous _fbc cookie; without a current click, the
    // persisted cookie remains valid across sessions.
    const fbclid = this.traffic?.session.fbclid;
    const rawCookieFbc = readCookie(this.documentRef, "_fbc");
    const cookieFbc = validFbc(rawCookieFbc) ? rawCookieFbc : undefined;
    const fbc = fbclid ? this.resolveFbc(fbclid) : cookieFbc;
    const boundedFbc = boundedText(fbc, MAX_META_BROWSER_ID_LENGTH);
    if (boundedFbc && boundedFbc !== cookieFbc)
      writeCookie(this.documentRef, "_fbc", boundedFbc);
    const rawFbp = readCookie(this.documentRef, "_fbp");
    return compact({
      source_url: currentSourceUrl(this.documentRef, this.windowRef),
      fbc: boundedFbc,
      fbp: validFbc(rawFbp) ? rawFbp : undefined,
      client_user_agent: boundedText(this.windowRef.navigator?.userAgent, 512),
    });
  }

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
type ProviderFunction = ((...args: unknown[]) => void) & {
  callMethod?: (...args: unknown[]) => void;
  queue?: unknown[][];
  loaded?: boolean;
  version?: string;
};
type ProviderWindow = Window &
  typeof globalThis & {
    dataLayer?: unknown[][];
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
    this.windowRef.dataLayer ??= [];
    this.windowRef.gtag ??= (...args: unknown[]) =>
      this.windowRef.dataLayer?.push(args);
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
      transaction_id: properties.order_id,
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
  const normalizedCurrency = currency.toUpperCase();
  const zeroDecimal = new Set([
    "BIF",
    "CLP",
    "DJF",
    "GNF",
    "JPY",
    "KMF",
    "KRW",
    "MGA",
    "PYG",
    "RWF",
    "UGX",
    "VND",
    "VUV",
    "XAF",
    "XOF",
    "XPF",
  ]);
  const threeDecimal = new Set(["BHD", "JOD", "KWD", "OMR", "TND"]);
  const divisor = zeroDecimal.has(normalizedCurrency)
    ? 1
    : threeDecimal.has(normalizedCurrency)
      ? 1_000
      : 100;
  return valueMinor / divisor;
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

function analyticsStorageNamespace(
  endpoint: string,
  publishableKey: string,
): string {
  const input = `${endpoint}\0${publishableKey}`;
  let hash = 2_166_136_261;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return (hash >>> 0).toString(36);
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

function persistentIdentifier(
  storage: Storage | undefined,
  key: string,
  randomUUID: () => string,
): string {
  try {
    const existing = storage?.getItem(key);
    if (isUuid(existing)) return existing as string;
    const generated = randomUUID();
    storage?.setItem(key, generated);
    return generated;
  } catch {
    return randomUUID();
  }
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

function currentSourceUrl(
  documentRef: Document,
  windowRef: Window & typeof globalThis,
): string | undefined {
  const documentHref = documentRef.location?.href;
  if (isHttpUrl(documentHref)) return withoutUrlFragment(documentHref);
  const windowLocation = (windowRef as unknown as { location?: Location })
    .location;
  const origin = windowLocation?.origin;
  const path = documentRef.location?.pathname;
  if (!isHttpUrl(origin) || typeof path !== "string") return undefined;
  try {
    return new URL(
      `${path}${documentRef.location?.search ?? ""}`,
      origin,
    ).toString();
  } catch {
    return undefined;
  }
}

function isHttpUrl(value: string | undefined): value is string {
  if (!value) return false;
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      Boolean(url.hostname)
    );
  } catch {
    return false;
  }
}

function withoutUrlFragment(value: string): string {
  const url = new URL(value);
  url.hash = "";
  return url.toString();
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

function captureTrafficTouchpoint(
  search: string | undefined,
  referrer: string | undefined,
): TrafficTouchpoint {
  const parameters = new URLSearchParams(search ?? "");
  return compact({
    source: boundedText(
      parameters.get("utm_source") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    medium: boundedText(
      parameters.get("utm_medium") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    campaign: boundedText(
      parameters.get("utm_campaign") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    campaign_id: boundedText(
      parameters.get("utm_id") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    term: boundedText(
      parameters.get("utm_term") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    content: boundedText(
      parameters.get("utm_content") ?? undefined,
      MAX_UTM_VALUE_LENGTH,
    ),
    referrer_domain: referrerHost(referrer),
    fbclid: boundedText(
      parameters.get("fbclid") ?? undefined,
      MAX_META_BROWSER_ID_LENGTH,
    ),
    gclid: boundedText(parameters.get("gclid") ?? undefined, 512),
  }) as TrafficTouchpoint;
}

function isNonDirectTouchpoint(value: TrafficTouchpoint): boolean {
  return Boolean(
    value.source ||
    value.medium ||
    value.campaign ||
    value.referrer_domain ||
    value.fbclid ||
    value.gclid,
  );
}

function readTrafficTouchpoint(
  storage: Storage | undefined,
  key: string,
): TrafficTouchpoint | undefined {
  const value = readStoredJson(storage, key);
  if (!value || typeof value !== "object" || Array.isArray(value))
    return undefined;
  const candidate = value as Record<string, unknown>;
  const result = compact({
    source: storedText(candidate.source, MAX_UTM_VALUE_LENGTH),
    medium: storedText(candidate.medium, MAX_UTM_VALUE_LENGTH),
    campaign: storedText(candidate.campaign, MAX_UTM_VALUE_LENGTH),
    campaign_id: storedText(candidate.campaign_id, MAX_UTM_VALUE_LENGTH),
    term: storedText(candidate.term, MAX_UTM_VALUE_LENGTH),
    content: storedText(candidate.content, MAX_UTM_VALUE_LENGTH),
    referrer_domain: storedText(candidate.referrer_domain, 253),
    fbclid: storedText(candidate.fbclid, MAX_META_BROWSER_ID_LENGTH),
    gclid: storedText(candidate.gclid, 512),
  }) as TrafficTouchpoint;
  return result;
}

function storedText(value: unknown, maximumLength: number): string | undefined {
  return typeof value === "string"
    ? boundedText(value, maximumLength)
    : undefined;
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
    // Storage is optional; the in-memory queue remains functional.
  }
}

function validQueuedEvent(value: unknown): value is QueuedEvent {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const event = value as Partial<QueuedEvent>;
  return (
    isUuid(event.event_id) &&
    typeof event.event_name === "string" &&
    isValidEventName(event.event_name) &&
    typeof event.occurred_at === "string" &&
    Number.isFinite(Date.parse(event.occurred_at)) &&
    isValidEventProperties(event.properties)
  );
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

function isPermanentClientError(error: unknown): error is ChaosApiError {
  return (
    error instanceof ChaosApiError &&
    error.status >= 400 &&
    error.status < 500 &&
    error.status !== 429
  );
}

function isSplittableClientError(error: unknown): error is ChaosApiError {
  return (
    error instanceof ChaosApiError &&
    (error.status === 400 || error.status === 422)
  );
}

function mergeCollectionResults(
  first: AnalyticsCollectionResult | null,
  second: AnalyticsCollectionResult | null,
): AnalyticsCollectionResult | null {
  if (!first) return second;
  if (!second) return first;
  return {
    received: first.received + second.received,
    stored: first.stored + second.stored,
    duplicates: first.duplicates + second.duplicates,
  };
}

function isValidGa4EventName(eventName: string): boolean {
  return (
    eventName.length <= MAX_GA4_EVENT_NAME_BYTES &&
    /^[A-Za-z][A-Za-z0-9_]*$/.test(eventName)
  );
}
