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

  constructor(options: StorefrontEventCoordinatorOptions) {
    const documentRef = options.events?.document ?? document;
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
    this.bestEffort(() =>
      this.analytics?.recordCartMutation(mutation),
    );
  }

  recordCheckoutCreation(
    creation: EmbeddedCheckoutStart | EmbeddedCheckoutCreation,
  ): void {
    this.bestEffort(() =>
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
    this.bestEffort(() =>
      this.analytics?.recordConfirmedPurchase(order),
    );
  }

  recordSearch(input: { query: string }): void {
    this.bestEffort(() => this.analytics?.search(input));
  }

  recordProductView(product: Product): void {
    this.bestEffort(() =>
      this.analytics?.viewContent(product.id),
    );
  }

  private bestEffort(operation: () => void): void {
    try {
      operation();
    } catch {
      // Storefront operations have already succeeded; analytics cannot fail them.
    }
  }
}
