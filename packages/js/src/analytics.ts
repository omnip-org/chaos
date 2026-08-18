/**
 * Consent-aware first-party behavior collection. Ported from
 * @chaos-commerce/storefront-analytics with identical runtime behavior;
 * see that package's original README for the full behavioral contract
 * (six deterministic Node test cases still apply, see analytics.test.ts).
 * It does not load advertising scripts, read cookies, or infer consent.
 */

const EVENT_SCHEMA_VERSION = 1;
const MAX_BATCH_SIZE = 20;
const MAX_QUEUE_SIZE = 100;
const MAX_ENGAGEMENT_INTERVAL_MS = 60_000;

export interface AnalyticsConsentInput {
  analyticsStorage: boolean;
  advertisingStorage: boolean;
  policyVersion: string;
}

export interface PageViewedInput {
  path?: string;
  title?: string;
  referrerDomain?: string;
  campaignSource?: string;
  campaignMedium?: string;
  campaignName?: string;
}

export interface AnalyticsOptions {
  publishableKey: string;
  endpoint?: string;
  fetch?: typeof fetch;
  document?: Document;
  window?: Window & typeof globalThis;
  storage?: Storage;
  sessionStorage?: Storage;
  randomUUID?: () => string;
  now?: () => number;
  setInterval?: typeof setInterval;
  clearInterval?: typeof clearInterval;
  flushIntervalMs?: number;
}

interface QueuedEvent {
  event_id: string;
  event_name: string;
  schema_version: 1;
  occurred_at: string;
  anonymous_id: string;
  session_id: string;
  consent: {
    analytics_storage: boolean;
    advertising_storage: boolean;
    policy_version: string;
  };
  properties: Record<string, unknown>;
}

export class ChaosStorefrontAnalytics {
  private readonly endpoint: string;
  private readonly publishableKey: string;
  private readonly fetchImpl: typeof fetch;
  private readonly documentRef: Document;
  private readonly windowRef: Window & typeof globalThis;
  private readonly storage?: Storage;
  private readonly sessionStorageRef?: Storage;
  private readonly randomUUID: () => string;
  private readonly now: () => number;
  private readonly setIntervalImpl: typeof setInterval;
  private readonly clearIntervalImpl: typeof clearInterval;
  private readonly flushIntervalMs: number;

  private readonly anonymousId: string;
  private readonly sessionId: string;
  private consent: { analyticsStorage: boolean; advertisingStorage: boolean; policyVersion: string };
  private queue: QueuedEvent[] = [];
  private inFlight: Promise<unknown> | null = null;
  private running = false;
  private timer: ReturnType<typeof setInterval> | null = null;
  private currentPageViewEventId: string | null = null;
  private activeStartedAt: number | null = null;
  private accumulatedActiveMs = 0;
  private readonly onActivityChange = () => this.updateActivityState();
  private readonly onPageHide = () => {
    this.flushEngagement();
    void this.flush({ keepalive: true }).catch(() => {});
  };

  constructor(options: AnalyticsOptions) {
    if (!options?.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.endpoint = options.endpoint ?? "/store/v1/analytics/events";
    this.publishableKey = options.publishableKey;
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    this.documentRef = options.document ?? globalThis.document;
    this.windowRef = options.window ?? (globalThis as unknown as Window & typeof globalThis);
    this.storage = options.storage ?? this.windowRef?.localStorage;
    this.sessionStorageRef = options.sessionStorage ?? this.windowRef?.sessionStorage;
    this.randomUUID = options.randomUUID ?? globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    this.now = options.now ?? Date.now;
    this.setIntervalImpl = options.setInterval ?? globalThis.setInterval;
    this.clearIntervalImpl = options.clearInterval ?? globalThis.clearInterval;
    this.flushIntervalMs = options.flushIntervalMs ?? 15_000;
    if (!this.fetchImpl || !this.randomUUID || !this.documentRef || !this.windowRef) {
      throw new TypeError("fetch, randomUUID, document, and window are required");
    }
    if (this.flushIntervalMs < 1_000 || this.flushIntervalMs > 60_000) {
      throw new RangeError("flushIntervalMs must be between 1000 and 60000");
    }

    this.anonymousId = persistentIdentifier(this.storage, "chaos.analytics.anonymous_id", this.randomUUID);
    this.sessionId = persistentIdentifier(this.sessionStorageRef, "chaos.analytics.session_id", this.randomUUID);
    this.consent = { analyticsStorage: false, advertisingStorage: false, policyVersion: "unset" };
  }

  setConsent({ analyticsStorage, advertisingStorage, policyVersion }: AnalyticsConsentInput): void {
    validatePolicyVersion(policyVersion);
    if (!analyticsStorage) {
      this.queue = [];
      this.accumulatedActiveMs = 0;
      this.currentPageViewEventId = null;
    }
    this.consent = {
      analyticsStorage: Boolean(analyticsStorage),
      advertisingStorage: Boolean(advertisingStorage),
      policyVersion,
    };
    this.activeStartedAt = this.isActive() && analyticsStorage ? this.now() : null;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.documentRef.addEventListener("visibilitychange", this.onActivityChange);
    this.windowRef.addEventListener("focus", this.onActivityChange);
    this.windowRef.addEventListener("blur", this.onActivityChange);
    this.windowRef.addEventListener("pagehide", this.onPageHide);
    this.updateActivityState();
    this.timer = this.setIntervalImpl(() => {
      this.flushEngagement();
      void this.flush().catch(() => {});
    }, this.flushIntervalMs);
  }

  async stop(): Promise<void> {
    if (this.running) {
      this.snapshotActiveTime();
      this.running = false;
      this.documentRef.removeEventListener("visibilitychange", this.onActivityChange);
      this.windowRef.removeEventListener("focus", this.onActivityChange);
      this.windowRef.removeEventListener("blur", this.onActivityChange);
      this.windowRef.removeEventListener("pagehide", this.onPageHide);
      if (this.timer !== null) this.clearIntervalImpl(this.timer);
      this.timer = null;
      this.activeStartedAt = null;
    }
    this.flushEngagement();
    await this.flush({ keepalive: true });
  }

  pageViewed(input: PageViewedInput = {}): string | null {
    const { path, title, referrerDomain, campaignSource, campaignMedium, campaignName } = input;
    this.flushEngagement();
    const resolvedPath = path ?? this.documentRef.location?.pathname ?? "/";
    const resolvedTitle = title ?? nonEmpty(this.documentRef.title);
    const resolvedReferrer = referrerDomain ?? referrerHost(this.documentRef.referrer);
    const campaign = campaignParameters(
      this.documentRef.location?.search,
      campaignSource,
      campaignMedium,
      campaignName,
    );
    const eventId = this.enqueue(
      "page_viewed",
      compact({
        path: resolvedPath,
        title: resolvedTitle,
        referrer_domain: resolvedReferrer,
        ...campaign,
      }),
    );
    this.accumulatedActiveMs = 0;
    this.currentPageViewEventId = eventId;
    this.activeStartedAt = this.isActive() && eventId ? this.now() : null;
    return eventId;
  }

  productViewed({ productId, productVariantId }: { productId: string; productVariantId?: string }): string | null {
    return this.enqueue(
      "product_viewed",
      compact({ product_id: productId, product_variant_id: productVariantId }),
    );
  }

  searchPerformed({ query, resultCount }: { query: string; resultCount?: number }): string | null {
    return this.enqueue("search_performed", compact({ query, result_count: resultCount }));
  }

  cartLineAdded({
    cartId,
    productVariantId,
    quantity,
  }: {
    cartId: string;
    productVariantId: string;
    quantity: number;
  }): string | null {
    return this.enqueue("cart_line_added", {
      cart_id: cartId,
      product_variant_id: productVariantId,
      quantity,
    });
  }

  checkoutStarted({ cartId, checkoutId }: { cartId: string; checkoutId?: string }): string | null {
    return this.enqueue("checkout_started", compact({ cart_id: cartId, checkout_id: checkoutId }));
  }

  flushEngagement(): number {
    this.snapshotActiveTime();
    if (!this.currentPageViewEventId || !this.consent.analyticsStorage) {
      this.accumulatedActiveMs = 0;
      return 0;
    }
    let emitted = 0;
    while (this.accumulatedActiveMs >= 1) {
      const activeMilliseconds = Math.min(Math.floor(this.accumulatedActiveMs), MAX_ENGAGEMENT_INTERVAL_MS);
      this.enqueue("engagement_heartbeat", {
        page_view_event_id: this.currentPageViewEventId,
        active_milliseconds: activeMilliseconds,
      });
      this.accumulatedActiveMs -= activeMilliseconds;
      emitted += activeMilliseconds;
    }
    return emitted;
  }

  flush(options: { keepalive?: boolean } = {}): Promise<unknown> {
    if (this.inFlight) return this.inFlight;
    if (this.queue.length === 0) return Promise.resolve(null);
    this.inFlight = this.sendNextBatch(Boolean(options.keepalive)).finally(() => {
      this.inFlight = null;
    });
    return this.inFlight;
  }

  private async sendNextBatch(keepalive: boolean): Promise<unknown> {
    const batch = this.queue.splice(0, MAX_BATCH_SIZE);
    try {
      const response = await this.fetchImpl(this.endpoint, {
        method: "POST",
        headers: {
          authorization: `Bearer ${this.publishableKey}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({ events: batch }),
        keepalive,
      });
      if (!response.ok) {
        throw new Error(`analytics collection failed with HTTP ${response.status}`);
      }
      return await response.json();
    } catch (error) {
      this.queue.unshift(...batch);
      this.trimQueue();
      throw error;
    }
  }

  private enqueue(eventName: string, properties: Record<string, unknown>): string | null {
    if (!this.consent.analyticsStorage) return null;
    const eventId = this.randomUUID();
    this.queue.push({
      event_id: eventId,
      event_name: eventName,
      schema_version: EVENT_SCHEMA_VERSION,
      occurred_at: new Date(this.now()).toISOString(),
      anonymous_id: this.anonymousId,
      session_id: this.sessionId,
      consent: {
        analytics_storage: this.consent.analyticsStorage,
        advertising_storage: this.consent.advertisingStorage,
        policy_version: this.consent.policyVersion,
      },
      properties,
    });
    this.trimQueue();
    if (this.queue.length >= MAX_BATCH_SIZE) {
      void this.flush().catch(() => {});
    }
    return eventId;
  }

  private trimQueue(): void {
    if (this.queue.length > MAX_QUEUE_SIZE) {
      this.queue.splice(0, this.queue.length - MAX_QUEUE_SIZE);
    }
  }

  private updateActivityState(): void {
    const active = this.isActive() && this.consent.analyticsStorage;
    this.snapshotActiveTime();
    this.activeStartedAt = active ? this.now() : null;
  }

  private snapshotActiveTime(): void {
    if (this.activeStartedAt === null) return;
    const now = this.now();
    this.accumulatedActiveMs += Math.max(0, now - this.activeStartedAt);
    this.activeStartedAt = now;
  }

  private isActive(): boolean {
    return this.documentRef.visibilityState === "visible" && this.documentRef.hasFocus();
  }
}

export function createStorefrontAnalytics(options: AnalyticsOptions): ChaosStorefrontAnalytics {
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
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  );
}

function validatePolicyVersion(value: string): void {
  if (typeof value !== "string" || !/^[A-Za-z0-9_.:-]{1,64}$/.test(value)) {
    throw new TypeError("policyVersion must match the server contract");
  }
}

function referrerHost(value: string | undefined): string | undefined {
  if (!value) return undefined;
  try {
    return new URL(value).host || undefined;
  } catch {
    return undefined;
  }
}

function nonEmpty(value: string | undefined): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function campaignParameters(
  search: string | undefined,
  source: string | undefined,
  medium: string | undefined,
  name: string | undefined,
): Record<string, unknown> {
  let parameters: URLSearchParams;
  try {
    parameters = new URLSearchParams(search ?? "");
  } catch {
    parameters = new URLSearchParams();
  }
  const campaignSource = boundedText(source ?? parameters.get("utm_source") ?? undefined, 100);
  if (!campaignSource) return {};
  return compact({
    campaign_source: campaignSource,
    campaign_medium: boundedText(medium ?? parameters.get("utm_medium") ?? undefined, 100),
    campaign_name: boundedText(name ?? parameters.get("utm_campaign") ?? undefined, 200),
  });
}

const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;

function boundedText(value: string | undefined, maximumLength: number): string | undefined {
  return typeof value === "string" &&
    value.length >= 1 &&
    value.length <= maximumLength &&
    !CONTROL_CHARACTERS.test(value)
    ? value
    : undefined;
}

function compact<T extends Record<string, unknown>>(value: T): Record<string, unknown> {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined));
}
