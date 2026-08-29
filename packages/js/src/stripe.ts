import { loadStripe, type StripeEmbeddedCheckout } from "@stripe/stripe-js";
import type { PaymentClientAction } from "./types.js";

export interface EmbeddedCheckoutMount {
  destroy(): void;
}

/** Owns Stripe's provider-specific embedded checkout lifecycle for storefronts. */
export async function mountEmbeddedCheckout(
  action: PaymentClientAction,
  container: HTMLElement,
): Promise<EmbeddedCheckoutMount> {
  if (action.type !== "mount_embedded_checkout") {
    throw new TypeError("unsupported payment client action");
  }
  const stripe = await loadStripe(action.public_key);
  if (!stripe) throw new Error("Stripe failed to load");
  const checkout = await stripe.createEmbeddedCheckoutPage({
    clientSecret: action.client_token,
  });
  checkout.mount(container);
  return createMountHandle(checkout);
}

function createMountHandle(checkout: StripeEmbeddedCheckout): EmbeddedCheckoutMount {
  return {
    destroy: () => checkout.destroy(),
  };
}
