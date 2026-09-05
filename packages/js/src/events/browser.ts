import { toPurchaseAnalyticsInput } from "../domain.js";
import { fnv1a32 } from "../internal/hash.js";
import type { CartLineMutation, EmbeddedCheckoutCreation, OrderLookup } from "../types.js";
import {
  addToCartEventData,
  initiateCheckoutEventData,
  purchaseEventData,
} from "./meta-payload.js";
import type {
  AddToCartAnalyticsInput,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "./types.js";

/**
 * First-party behavior collection. Events project directly to the
 * configured browser providers (Meta Pixel, GA4) — there is no chaos-owned
 * ledger or delivery queue; a store that also wants server-side Meta
 * Conversions API delivery uses `ChaosServerEvents` from the separate
 * `@omnip-org/chaos-js/meta-capi` subpath, from its own server-side code
 * (see `../ssr/server.ts`'s `events` option).
 *
 * There are exactly six events (page_view, view_content, search,
 * add_to_cart, initiate_checkout, purchase); every one of them is emitted by
 * this SDK, never by store-supplied names or properties. The three commerce
 * events share their Meta `custom_data` shape with the CAPI sender via
 * `./meta-payload.js`; GA4's field names differ enough per event that they
 * stay inlined below instead of adding a second shared mapper for one caller
 * each.
 */

const MAX_META_BROWSER_ID_LENGTH = 2_048;
const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const PROVIDER_EVENT_MAX_AGE_MS = 90 * 24 * 60 * 60 * 1000;

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
  providers?: {
    metaPixel?: { pixelId: string };
    ga4?: { measurementId: string };
  };
  /** Starts lifecycle and SPA page tracking. Defaults to true. */
  autoStart?: boolean;
  /**
   * Best-effort delivery-failure hook: called when a browser provider call
   * (Pixel/GA4) throws, so a store can log or alert instead of failing
   * silently. Never awaited and never allowed to throw back into the caller
   * — mirrors `MetaCapiConfig.onError` for the server-side CAPI sender.
   */
  onError?: (
    error: unknown,
    event: { eventName: string; eventId?: string | undefined },
  ) => void;
}

export class ChaosStorefrontAnalytics {
  private readonly publishableKey: string;
  private readonly documentRef: Document;
  private readonly windowRef: Window & typeof globalThis;
  private readonly storage?: Storage;
  private readonly sessionStorageRef?: Storage;
  private readonly randomUUID: () => string;
  private readonly now: () => number;
  private readonly destinations: AnalyticsDestinations;

  private readonly metaFbcStorageKey: string;
  private readonly providerEventStoragePrefix: string;
  private running = false;
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
    if (!this.randomUUID || !this.documentRef || !this.windowRef) {
      throw new TypeError("randomUUID, document, and window are required");
    }
    this.destinations = new AnalyticsDestinations(
      this.windowRef,
      this.documentRef,
      options.providers,
      options.onError,
    );

    const storageNamespace = analyticsStorageNamespace(this.publishableKey);
    this.metaFbcStorageKey = `chaos.analytics.${storageNamespace}.meta.fbc.v2`;
    this.providerEventStoragePrefix = `chaos.analytics.${storageNamespace}.provider_event.v1.`;
    this.pruneExpiredProviderEvents();
    this.maintainFbcCookie();
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

  pageView(input: PageViewInput = {}): string {
    this.maintainFbcCookie();
    const path = input.path ?? this.documentRef.location?.pathname ?? "/";
    const title = input.title ?? nonEmpty(this.documentRef.title);
    const eventId = this.randomUUID();
    this.destinations.pixel("PageView", eventId, { page_path: path });
    this.destinations.ga4(
      "page_view",
      compact({ event_id: eventId, page_path: path, page_title: title }),
    );
    return eventId;
  }

  /**
   * Records a successful cart addition. `eventId` lets a caller that already
   * sent this event through server-side Meta CAPI (see
   * `@omnip-org/chaos-js/meta-capi`) share the same event ID for Meta's
   * Pixel+CAPI deduplication, instead of minting a second one.
   *
   * Warning: if a server-side CAPI call for this same action already ran
   * (directly, or through `addCartLine`/`updateCartLine` in `ssr/server.ts`),
   * always pass its `eventId` here. Calling this without it while CAPI is
   * also configured sends Meta two independent events for one action, which
   * it cannot deduplicate — `recordCartMutation` does this pairing for you
   * when you go through the `ssr/server.ts` + `StorefrontBrowserClient` pair.
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

    const resolvedId = canonicalEventId(eventId, this.randomUUID());
    try {
      return this.recordOnce("add_to_cart", resolvedId, () => {
        const eventData = addToCartEventData(input);
        this.destinations.pixel("AddToCart", resolvedId, eventData);
        this.destinations.ga4("add_to_cart", {
          event_id: resolvedId,
          value: eventData.value,
          currency: eventData.currency,
          items: [
            { item_id: input.productVariantId, quantity: input.quantity, price: eventData.contents[0]!.item_price },
          ],
        });
      });
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
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

    const resolvedId = canonicalEventId(eventId, this.randomUUID());
    try {
      return this.recordOnce("initiate_checkout", resolvedId, () => {
        const eventData = initiateCheckoutEventData(input);
        this.destinations.pixel("InitiateCheckout", resolvedId, eventData);
        this.destinations.ga4("begin_checkout", {
          event_id: resolvedId,
          transaction_id: input.orderNumber,
          value: eventData.value,
          currency: eventData.currency,
          items: eventData.contents.map((content) => ({
            item_id: content.id,
            quantity: content.quantity,
            price: content.item_price,
          })),
        });
      });
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
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

  viewContent({
    productId,
    productVariantId,
  }: {
    productId: string;
    productVariantId?: string;
  }): string {
    const contentId = productVariantId ?? productId;
    const eventId = this.randomUUID();
    this.destinations.pixel("ViewContent", eventId, {
      content_ids: [contentId],
      content_type: "product",
    });
    this.destinations.ga4("view_item", {
      event_id: eventId,
      items: [{ item_id: contentId }],
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
    const currency = input.currency.toUpperCase();
    if (!/^[A-Z]{3}$/.test(currency))
      throw new TypeError("currency must be an ISO 4217 code");
    if (!isUuid(input.orderId))
      throw new TypeError("orderId must be a valid UUID");
    const orderId = input.orderId.toLowerCase();

    try {
      return this.recordOnce("purchase", orderId, () => {
        const eventData = purchaseEventData(input);
        this.destinations.pixel("Purchase", orderId, eventData);
        this.destinations.ga4("purchase", {
          event_id: orderId,
          transaction_id: orderId,
          value: eventData.value,
          currency: eventData.currency,
          items: eventData.contents.map((content) => ({
            item_id: content.id,
            quantity: content.quantity,
            price: content.item_price,
          })),
        });
      });
    } catch {
      // Provider/storage problems are best-effort; bad input above already threw.
      return null;
    }
  }

  /** Projects a confirmed, paid order without making the caller rebuild event fields. */
  recordConfirmedPurchase(order: Pick<OrderLookup, "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines">): string | null {
    const input = toPurchaseAnalyticsInput(order);
    return input ? this.recordPurchase(input) : null;
  }

  /**
   * Drops `provider_event` dedup keys older than `PROVIDER_EVENT_MAX_AGE_MS`
   * so a long-lived visitor's `storage` doesn't accumulate one entry per
   * commerce action forever. Realistic reloads of a confirmation page never
   * approach this age, so this never reopens a real dedup window.
   */
  private pruneExpiredProviderEvents(): void {
    if (!this.storage) return;
    try {
      const cutoff = this.now() - PROVIDER_EVENT_MAX_AGE_MS;
      const staleKeys: string[] = [];
      for (let index = 0; index < this.storage.length; index += 1) {
        const key = this.storage.key(index);
        if (!key || !key.startsWith(this.providerEventStoragePrefix)) continue;
        const recordedAt = Date.parse(this.storage.getItem(key) ?? "");
        if (!Number.isNaN(recordedAt) && recordedAt < cutoff) staleKeys.push(key);
      }
      for (const key of staleKeys) this.storage.removeItem(key);
    } catch {
      // Storage enumeration is optional; growth is bounded on a best-effort basis.
    }
  }

  /**
   * Runs `project()` exactly once per `eventName`+`eventId` pair, tracked in
   * `storage`. Provider failures inside `project()` are swallowed — analytics
   * is best-effort and must never surface here as a thrown error.
   */
  private recordOnce(
    eventName: string,
    eventId: string,
    project: () => void,
  ): string | null {
    const storageKey = `${this.providerEventStoragePrefix}${eventName}.${eventId}`;
    if (this.storage?.getItem(storageKey)) return null;
    try {
      project();
    } catch {
      // Browser provider failures are best-effort; keep the event ID stable
      // for a future retry.
    }
    this.storage?.setItem(storageKey, new Date(this.now()).toISOString());
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

/** Resolves the event ID a commerce projection uses: an explicit, validated ID, or a freshly minted one — always lowercased for Meta's dedup match. */
function canonicalEventId(explicit: string | undefined, fallback: string): string {
  const resolved = explicit ?? fallback;
  if (!isUuid(resolved)) {
    throw new TypeError("commerce event_id must be a valid UUID");
  }
  return resolved.toLowerCase();
}

type DestinationOptions = AnalyticsOptions["providers"];
type GtagDataLayerEntry = unknown[] | IArguments;
type FbqFunction = ((...args: unknown[]) => void) & {
  callMethod?: (...args: unknown[]) => void;
  queue?: unknown[][];
  loaded?: boolean;
  version?: string;
};
type AnalyticsWindow = Window &
  typeof globalThis & {
    dataLayer?: GtagDataLayerEntry[];
    gtag?: (...args: unknown[]) => void;
    fbq?: FbqFunction;
    _fbq?: FbqFunction;
  };

/** Talks to `fbq`/`gtag` directly — no field mapping, callers build their own params. */
class AnalyticsDestinations {
  private readonly windowRef: AnalyticsWindow;
  private readonly documentRef: Document;
  private readonly options: DestinationOptions;
  private readonly onError: AnalyticsOptions["onError"];
  private metaStarted = false;
  private ga4Started = false;

  constructor(
    windowRef: Window & typeof globalThis,
    documentRef: Document,
    options: DestinationOptions,
    onError: AnalyticsOptions["onError"],
  ) {
    this.windowRef = windowRef as AnalyticsWindow;
    this.documentRef = documentRef;
    this.options = options;
    this.onError = onError;
    validateDestinationOptions(options);
    if (this.options?.ga4) this.startGa4();
    if (this.options?.metaPixel) this.startMeta();
  }

  pixel(eventName: string, eventId: string, params: Record<string, unknown>): void {
    if (!this.metaStarted) return;
    try {
      this.windowRef.fbq?.("track", eventName, params, { eventID: eventId });
    } catch (error) {
      this.reportError(error, eventName, eventId);
    }
  }

  ga4(eventName: string, params: Record<string, unknown>): void {
    if (!this.ga4Started) return;
    try {
      this.windowRef.gtag?.("event", eventName, params);
    } catch (error) {
      this.reportError(
        error,
        eventName,
        typeof params.event_id === "string" ? params.event_id : undefined,
      );
    }
  }

  private reportError(
    error: unknown,
    eventName: string,
    eventId: string | undefined,
  ): void {
    try {
      this.onError?.(error, { eventName, eventId });
    } catch {
      // onError must never break delivery — see AnalyticsOptions.onError.
    }
  }

  private startMeta(): void {
    if (this.metaStarted || !this.options?.metaPixel) return;
    this.metaStarted = true;
    if (!this.windowRef.fbq) {
      const fbq: FbqFunction = (...args: unknown[]) => {
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

function validateDestinationOptions(options: DestinationOptions): void {
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

function validateMoney(valueMinor: number, currency: string): void {
  if (!Number.isSafeInteger(valueMinor) || valueMinor < 0) {
    throw new RangeError("valueMinor must be a non-negative safe integer");
  }
  if (!/^[A-Za-z]{3}$/.test(currency))
    throw new TypeError("currency must be an ISO 4217 code");
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

function validFbc(value: string | undefined): value is string {
  if (!value || value.length > MAX_META_BROWSER_ID_LENGTH) return false;
  const match = /^fb\.\d+\.(\d{13})\.[^\s]+$/.exec(value);
  return match !== null && Number.isSafeInteger(Number(match[1]));
}
