import {
  ChaosStorefrontAnalytics,
  type AnalyticsOptions,
} from "../events/browser.js";
import type {
  CartLineMutation,
  ConfirmedPurchaseOrderInput,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutStart,
  OwnOrder,
  Product,
} from "../types.js";
import { maintainMetaFbcCookie } from "./meta-attribution.js";

interface StorefrontEventCoordinatorOptions {
  events?: AnalyticsOptions;
  now: () => number;
  randomUUID: () => string;
}

/** Keeps browser analytics outside the API client and resource classes. */
export class StorefrontEventCoordinator {
  private readonly analytics: ChaosStorefrontAnalytics | null;
  private warnedUnreachable = false;

  constructor(options: StorefrontEventCoordinatorOptions) {
    const documentRef = options.events?.document ?? resolveDocument();
    if (options.events) {
      this.analytics = new ChaosStorefrontAnalytics({
        now: options.now,
        randomUUID: options.randomUUID,
        ...options.events,
      });
    } else {
      maintainMetaFbcCookie(documentRef, options.now);
      this.analytics = null;
    }
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
      await this.analytics?.setMetaOrderIdentity(order);
      recordConfirmedPurchase(order);
    } catch {
      // Analytics are best-effort after payment.
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

  recordProductView(product: Product): void {
    this.bestEffort("recordProductView", () =>
      this.analytics?.viewContent(product.id),
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
        "never reach Meta Pixel/GA4 from here (this is normal during SSR). Run the customer-facing " +
        "resource operation through a browser ChaosStorefrontClient when it should emit an event.",
    );
  }
}

function resolveDocument(): Document | undefined {
  try {
    return globalThis.document;
  } catch {
    return undefined;
  }
}
