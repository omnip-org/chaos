import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import type {
  DataEnvelope,
  OrderLookup,
  OwnOrder,
  WaitForCheckoutOrderOptions,
} from "../types.js";

const DEFAULT_POLL_INTERVAL_MS = 1_000;
const DEFAULT_WAIT_TIMEOUT_MS = 30_000;

export interface OrderLookupParams {
  orderNumber: string;
  email: string;
}

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  lookupOrder(params: OrderLookupParams): Promise<DataEnvelope<OrderLookup>> {
    return this.client.request("/orders/search", {
      query: { order_number: params.orderNumber, email: params.email },
    });
  }

  /** Reads this shopper's existing Order and projects a paid checkout in the browser. */
  async getCheckoutOrder(orderId: string): Promise<DataEnvelope<OwnOrder>> {
    return this.readCheckoutOrder(orderId);
  }

  private async readCheckoutOrder(
    orderId: string,
    signal?: AbortSignal,
  ): Promise<DataEnvelope<OwnOrder>> {
    if (!this.client.getShopperToken()) {
      throw new ChaosApiError(
        401,
        "shopper_token_required",
        "the checkout shopper token is missing",
      );
    }
    const result = await this.client.request<DataEnvelope<OwnOrder>>(
      `/orders/${encodeURIComponent(orderId)}/details`,
      {
        requiresShopperToken: true,
        retryShopperToken: false,
        ...(signal ? { signal } : {}),
      },
    );
    await this.client.recordCheckoutPurchase(result.data);
    return result;
  }

  /**
   * Waits for the checkout webhook to move an Order out of its pending state.
   * A confirmed paid result projects Purchase before it is returned. Failed,
   * expired and cancelled Orders are returned immediately for confirmation UI.
   */
  async waitForCheckoutOrder(
    orderId: string,
    options: WaitForCheckoutOrderOptions = {},
  ): Promise<DataEnvelope<OwnOrder>> {
    const intervalMs = finiteDuration(
      options.intervalMs,
      DEFAULT_POLL_INTERVAL_MS,
      "intervalMs",
      true,
    );
    const timeoutMs = finiteDuration(
      options.timeoutMs,
      DEFAULT_WAIT_TIMEOUT_MS,
      "timeoutMs",
      false,
    );
    const timeoutController = new AbortController();
    const timeout = setTimeout(
      () =>
        timeoutController.abort(
          new DOMException(
            `Timed out waiting for checkout Order ${orderId}`,
            "TimeoutError",
          ),
        ),
      timeoutMs,
    );
    const signal = options.signal
      ? AbortSignal.any([options.signal, timeoutController.signal])
      : timeoutController.signal;

    try {
      for (;;) {
        signal.throwIfAborted();
        const result = await this.readCheckoutOrder(orderId, signal);
        if (isTerminalCheckoutOrder(result.data)) return result;
        await delay(intervalMs, signal);
      }
    } finally {
      clearTimeout(timeout);
    }
  }
}

function isTerminalCheckoutOrder(order: OwnOrder): boolean {
  if (order.status === "cancelled") return true;
  if (["failed", "expired"].includes(order.payment_status)) return true;
  return (
    order.status === "confirmed" &&
    ["paid", "partially_refunded", "refunded"].includes(order.payment_status)
  );
}

function finiteDuration(
  value: number | undefined,
  fallback: number,
  name: string,
  allowZero: boolean,
): number {
  const duration = value ?? fallback;
  if (
    !Number.isFinite(duration) ||
    duration < 0 ||
    (!allowZero && duration === 0)
  ) {
    throw new TypeError(
      `${name} must be a finite ${allowZero ? "non-negative" : "positive"} number`,
    );
  }
  return duration;
}

function delay(
  milliseconds: number,
  signal: AbortSignal | undefined,
): Promise<void> {
  if (signal?.aborted) {
    return Promise.reject(
      signal.reason ?? new DOMException("Polling was aborted", "AbortError"),
    );
  }
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      signal?.removeEventListener("abort", abort);
      resolve();
    }, milliseconds);
    const abort = () => {
      clearTimeout(timeout);
      reject(
        signal?.reason ?? new DOMException("Polling was aborted", "AbortError"),
      );
    };
    signal?.addEventListener("abort", abort, { once: true });
  });
}
