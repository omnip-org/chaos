import type { ChaosStorefrontClient } from "../client.js";
import type {
  CreateEmbeddedCheckoutRequest,
  CreatePaymentAttemptRequest,
  DataEnvelope,
  EmbeddedCheckoutSession,
  PaymentAttempt,
  PaymentClientAction,
} from "../types.js";

export class PaymentsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  createEmbeddedCheckout(
    cartId: string,
    body: CreateEmbeddedCheckoutRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    return this.client.request<DataEnvelope<EmbeddedCheckoutSession>>(
      `/carts/${encodeURIComponent(cartId)}/embedded-checkout`,
      {
        method: "POST",
        body,
        requiresShopperToken: true,
        idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
      },
    );
  }

  /** Creates a Stripe Embedded Checkout payment attempt. */
  createAttempt(
    orderId: string,
    body: CreatePaymentAttemptRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<PaymentAttempt>> {
    return this.client.request<DataEnvelope<PaymentAttempt>>(`/orders/${encodeURIComponent(orderId)}/payment-attempts`, {
      method: "POST",
      body,
      requiresShopperToken: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }

  getAttempt(paymentAttemptId: string): Promise<DataEnvelope<PaymentAttempt>> {
    return this.client.request(`/payment-attempts/${encodeURIComponent(paymentAttemptId)}`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }

  /**
   * Returns short-lived Stripe client handoff material.
   *
   * For Embedded Checkout, type is
   * `mount_embedded_checkout` and client_token is an Embedded Checkout Session
   * client secret. Never log, cache, or place it in a URL.
   */
  getClientAction(paymentAttemptId: string): Promise<DataEnvelope<PaymentClientAction>> {
    return this.client.request(`/payment-attempts/${encodeURIComponent(paymentAttemptId)}/client-action`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }
}
