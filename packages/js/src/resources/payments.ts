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
   * Returns short-lived provider client handoff material.
   *
   * For the current `stripe_checkout` provider, type is
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
