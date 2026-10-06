import {
  ChaosStorefrontAnalytics,
  type AnalyticsOptions,
} from "../events/browser.js";
import type { ViewContentAnalyticsInput } from "../events/types.js";
import type {
  CartLineMutation,
  ConfirmedPurchaseOrderInput,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutStart,
  OwnOrder,
} from "../types.js";
import { scopedStorageKey } from "./hash.js";

const CHECKOUT_ORDER_STORAGE_PREFIX = "chaos.storefront.checkout_order";
const CHECKOUT_ORDER_MAX_AGE_MS = 24 * 60 * 60 * 1000;

interface StorefrontEventCoordinatorOptions {
  baseUrl: string;
  publishableKey: string;
  events?: Omit<AnalyticsOptions, "publishableKey">;
  now: () => number;
}

/** Keeps browser analytics and checkout-return state outside the API client. */
export class StorefrontEventCoordinator {
  private readonly analytics: ChaosStorefrontAnalytics | null;
  private readonly checkoutStorage: Pick<
    Storage,
    "getItem" | "setItem" | "removeItem"
  > | null;
  private readonly checkoutOrderStorageKey: string;
  private warnedUnreachable = false;

  constructor(private readonly options: StorefrontEventCoordinatorOptions) {
    this.analytics = options.events
      ? new ChaosStorefrontAnalytics({
          publishableKey: `${options.baseUrl}\0${options.publishableKey}`,
          ...options.events,
        })
      : null;
    this.checkoutStorage = resolveCheckoutStorage(options.events?.sessionStorage);
    this.checkoutOrderStorageKey = scopedStorageKey(
      CHECKOUT_ORDER_STORAGE_PREFIX,
      options.baseUrl,
      options.publishableKey,
    );
  }

  setShopperId(shopperId: string): void {
    this.analytics?.setShopperId(shopperId);
  }

  clearShopperId(): void {
    this.analytics?.clearShopperId();
  }

  recordCartMutation(mutation: CartLineMutation): void {
    this.bestEffort("recordCartMutation", () =>
      this.analytics?.recordCartMutation(mutation),
    );
  }

  recordCheckoutCreation(
    creation: EmbeddedCheckoutStart | EmbeddedCheckoutCreation,
  ): void {
    this.bestEffort("recordCheckoutCreation", () =>
      this.analytics?.recordCheckoutCreation(creation),
    );
  }

  rememberCheckoutOrder(orderId: string): void {
    try {
      this.checkoutStorage?.setItem(
        this.checkoutOrderStorageKey,
        JSON.stringify({ orderId, startedAt: this.options.now() }),
      );
    } catch {
      // The payment handoff must work even when session storage is blocked.
    }
  }

  async recordCheckoutPurchase(
    order: OwnOrder,
    recordConfirmedPurchase: (order: ConfirmedPurchaseOrderInput) => void,
  ): Promise<void> {
    if (
      order.status !== "confirmed" ||
      !["paid", "partially_refunded", "refunded"].includes(
        order.payment_status,
      )
    ) {
      return;
    }
    try {
      const stored = this.checkoutStorage?.getItem(this.checkoutOrderStorageKey);
      if (!stored) return;
      const marker: unknown = JSON.parse(stored);
      if (!marker || typeof marker !== "object") return;
      const { orderId, startedAt } = marker as Record<string, unknown>;
      const age =
        typeof startedAt === "number" ? this.options.now() - startedAt : NaN;
      if (
        orderId !== order.id ||
        !Number.isFinite(age) ||
        age < 0 ||
        age > CHECKOUT_ORDER_MAX_AGE_MS
      ) {
        return;
      }
      await this.analytics?.setMetaOrderIdentity(order);
      recordConfirmedPurchase(order);
    } catch {
      // Analytics and browser storage are best-effort after payment.
    }
  }

  recordConfirmedPurchase(order: ConfirmedPurchaseOrderInput): void {
    this.bestEffort("recordConfirmedPurchase", () =>
      this.analytics?.recordConfirmedPurchase(order),
    );
  }

  recordSearch(input: { query: string }): void {
    this.bestEffort("recordSearch", () => this.analytics?.search(input));
  }

  recordViewContent(input: ViewContentAnalyticsInput): void {
    this.bestEffort("recordViewContent", () =>
      this.analytics?.viewContent(input),
    );
  }

  private bestEffort(method: string, operation: () => void): void {
    this.warnIfUnreachable(method);
    try {
      operation();
    } catch {
      // Storefront operations have already succeeded; analytics cannot fail them.
    }
  }

  private warnIfUnreachable(method: string): void {
    if (
      this.analytics ||
      typeof document !== "undefined" ||
      this.warnedUnreachable
    ) {
      return;
    }
    this.warnedUnreachable = true;
    console.warn(
      `[chaos-js] ChaosStorefrontClient.${method}() ran with no \`document\` present, so it can ` +
        "never reach Meta Pixel/GA4 from here (this is normal during SSR). Call the matching " +
        "record* method again from a browser-hydrated component instead of relying on this call.",
    );
  }
}

function resolveCheckoutStorage(
  explicit: Storage | undefined,
): Pick<Storage, "getItem" | "setItem" | "removeItem"> | null {
  if (explicit) return explicit;
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}
