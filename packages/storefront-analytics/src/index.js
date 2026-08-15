const EVENT_SCHEMA_VERSION = 1;
const MAX_BATCH_SIZE = 20;
const MAX_QUEUE_SIZE = 100;
const MAX_ENGAGEMENT_INTERVAL_MS = 60_000;

export class ChaosStorefrontAnalytics {
  constructor(options) {
    if (!options?.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.endpoint = options.endpoint ?? "/store/v1/analytics/events";
    this.publishableKey = options.publishableKey;
    this.fetch = options.fetch ?? globalThis.fetch?.bind(globalThis);
    this.document = options.document ?? globalThis.document;
    this.window = options.window ?? globalThis.window;
    this.storage = options.storage ?? this.window?.localStorage;
    this.sessionStorage = options.sessionStorage ?? this.window?.sessionStorage;
    this.randomUUID = options.randomUUID ?? globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    this.now = options.now ?? Date.now;
    this.setInterval = options.setInterval ?? globalThis.setInterval;
    this.clearInterval = options.clearInterval ?? globalThis.clearInterval;
    this.flushIntervalMs = options.flushIntervalMs ?? 15_000;
    if (!this.fetch || !this.randomUUID || !this.document || !this.window) {
      throw new TypeError("fetch, randomUUID, document, and window are required");
    }
    if (this.flushIntervalMs < 1_000 || this.flushIntervalMs > 60_000) {
      throw new RangeError("flushIntervalMs must be between 1000 and 60000");
    }

    this.anonymousId = persistentIdentifier(
      this.storage,
      "chaos.analytics.anonymous_id",
      this.randomUUID,
    );
    this.sessionId = persistentIdentifier(
      this.sessionStorage,
      "chaos.analytics.session_id",
      this.randomUUID,
    );
    this.consent = {
      analyticsStorage: false,
      advertisingStorage: false,
      policyVersion: "unset",
    };
    this.queue = [];
    this.inFlight = null;
    this.running = false;
    this.timer = null;
    this.currentPageViewEventId = null;
    this.activeStartedAt = null;
    this.accumulatedActiveMs = 0;
    this.onActivityChange = () => this.updateActivityState();
    this.onPageHide = () => {
      this.flushEngagement();
      void this.flush({ keepalive: true }).catch(() => {});
    };
  }

  setConsent({ analyticsStorage, advertisingStorage, policyVersion }) {
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

  start() {
    if (this.running) return;
    this.running = true;
    this.document.addEventListener("visibilitychange", this.onActivityChange);
    this.window.addEventListener("focus", this.onActivityChange);
    this.window.addEventListener("blur", this.onActivityChange);
    this.window.addEventListener("pagehide", this.onPageHide);
    this.updateActivityState();
    this.timer = this.setInterval(() => {
      this.flushEngagement();
      void this.flush().catch(() => {});
    }, this.flushIntervalMs);
  }

  async stop() {
    if (this.running) {
      this.snapshotActiveTime();
      this.running = false;
      this.document.removeEventListener("visibilitychange", this.onActivityChange);
      this.window.removeEventListener("focus", this.onActivityChange);
      this.window.removeEventListener("blur", this.onActivityChange);
      this.window.removeEventListener("pagehide", this.onPageHide);
      if (this.timer !== null) this.clearInterval(this.timer);
      this.timer = null;
      this.activeStartedAt = null;
    }
    this.flushEngagement();
    await this.flush({ keepalive: true });
  }

  pageViewed({ path, title, referrerDomain } = {}) {
    this.flushEngagement();
    const resolvedPath = path ?? this.document.location?.pathname ?? "/";
    const resolvedTitle = title ?? nonEmpty(this.document.title);
    const resolvedReferrer = referrerDomain ?? referrerHost(this.document.referrer);
    const eventId = this.enqueue("page_viewed", compact({
      path: resolvedPath,
      title: resolvedTitle,
      referrer_domain: resolvedReferrer,
    }));
    this.accumulatedActiveMs = 0;
    this.currentPageViewEventId = eventId;
    this.activeStartedAt = this.isActive() && eventId ? this.now() : null;
    return eventId;
  }

  productViewed({ productId, productVariantId }) {
    return this.enqueue("product_viewed", compact({
      product_id: productId,
      product_variant_id: productVariantId,
    }));
  }

  searchPerformed({ query, resultCount }) {
    return this.enqueue("search_performed", compact({
      query,
      result_count: resultCount,
    }));
  }

  cartLineAdded({ cartId, productVariantId, quantity }) {
    return this.enqueue("cart_line_added", {
      cart_id: cartId,
      product_variant_id: productVariantId,
      quantity,
    });
  }

  checkoutStarted({ cartId, checkoutId }) {
    return this.enqueue("checkout_started", compact({
      cart_id: cartId,
      checkout_id: checkoutId,
    }));
  }

  flushEngagement() {
    this.snapshotActiveTime();
    if (!this.currentPageViewEventId || !this.consent.analyticsStorage) {
      this.accumulatedActiveMs = 0;
      return 0;
    }
    let emitted = 0;
    while (this.accumulatedActiveMs >= 1) {
      const activeMilliseconds = Math.min(
        Math.floor(this.accumulatedActiveMs),
        MAX_ENGAGEMENT_INTERVAL_MS,
      );
      this.enqueue("engagement_heartbeat", {
        page_view_event_id: this.currentPageViewEventId,
        active_milliseconds: activeMilliseconds,
      });
      this.accumulatedActiveMs -= activeMilliseconds;
      emitted += activeMilliseconds;
    }
    return emitted;
  }

  flush(options = {}) {
    if (this.inFlight) return this.inFlight;
    if (this.queue.length === 0) return Promise.resolve(null);
    this.inFlight = this.sendNextBatch(Boolean(options.keepalive)).finally(() => {
      this.inFlight = null;
    });
    return this.inFlight;
  }

  async sendNextBatch(keepalive) {
    const batch = this.queue.splice(0, MAX_BATCH_SIZE);
    try {
      const response = await this.fetch(this.endpoint, {
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

  enqueue(eventName, properties) {
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

  trimQueue() {
    if (this.queue.length > MAX_QUEUE_SIZE) {
      this.queue.splice(0, this.queue.length - MAX_QUEUE_SIZE);
    }
  }

  updateActivityState() {
    const active = this.isActive() && this.consent.analyticsStorage;
    this.snapshotActiveTime();
    this.activeStartedAt = active ? this.now() : null;
  }

  snapshotActiveTime() {
    if (this.activeStartedAt === null) return;
    const now = this.now();
    this.accumulatedActiveMs += Math.max(0, now - this.activeStartedAt);
    this.activeStartedAt = now;
  }

  isActive() {
    return this.document.visibilityState === "visible" && this.document.hasFocus();
  }
}

export function createStorefrontAnalytics(options) {
  return new ChaosStorefrontAnalytics(options);
}

function persistentIdentifier(storage, key, randomUUID) {
  try {
    const existing = storage?.getItem(key);
    if (isUuid(existing)) return existing;
    const generated = randomUUID();
    storage?.setItem(key, generated);
    return generated;
  } catch {
    return randomUUID();
  }
}

function isUuid(value) {
  return typeof value === "string"
    && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function validatePolicyVersion(value) {
  if (typeof value !== "string" || !/^[A-Za-z0-9_.:-]{1,64}$/.test(value)) {
    throw new TypeError("policyVersion must match the server contract");
  }
}

function referrerHost(value) {
  if (!value) return undefined;
  try {
    return new URL(value).host || undefined;
  } catch {
    return undefined;
  }
}

function nonEmpty(value) {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function compact(value) {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined));
}
