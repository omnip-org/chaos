import type { ChaosStorefrontClient } from "../client.js";
import type { CreatePaymentAttemptRequest, DataEnvelope, PaymentAttempt, PaymentClientAction } from "../types.js";

export class PaymentsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  /**
   * For the stripe_checkout provider, `body.return_url` is required.
   */
  createAttempt(
    orderId: string,
    body: CreatePaymentAttemptRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<PaymentAttempt>> {
    return this.client.request(`/orders/${encodeURIComponent(orderId)}/payment-attempts`, {
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
   * Returns short-lived provider client handoff material.
   *
   * type: "confirm_payment" — client_token is a PaymentIntent client secret.
   * Never log, cache, or place it in a URL.
   *
   * type: "mount_embedded_checkout" — client_token is an Embedded Checkout
   * Session client secret. Never log, cache, or place it in a URL.
   */
  getClientAction(paymentAttemptId: string): Promise<DataEnvelope<PaymentClientAction>> {
    return this.client.request(`/payment-attempts/${encodeURIComponent(paymentAttemptId)}/client-action`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }
}
