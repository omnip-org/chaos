import type { PaymentClientAction } from "./types.js";

export interface EmbeddedCheckoutMount {
  destroy(): void;
}

/**
 * The minimal slice of Stripe.js this module uses. Stripe.js itself is always
 * loaded from https://js.stripe.com at runtime (Stripe does not allow
 * self-hosting or bundling it), so this package carries no `@stripe/stripe-js`
 * dependency and consumers need nothing extra to use `@omnip-org/chaos-js/stripe`.
 */
interface StripeEmbeddedCheckoutHandle {
  mount(location: string | HTMLElement): void;
  unmount(): void;
  destroy(): void;
}

interface StripeInstance {
  createEmbeddedCheckoutPage(options: {
    clientSecret: string;
  }): Promise<StripeEmbeddedCheckoutHandle>;
}

type StripeConstructor = (publishableKey: string) => StripeInstance;

const STRIPE_JS_URL = "https://js.stripe.com/v3/";

/** Reads `window.Stripe` without a global `Window` augmentation that could clash
 * with a consumer that also has `@stripe/stripe-js` types loaded. */
function readStripeGlobal(): StripeConstructor | undefined {
  return (globalThis as { Stripe?: StripeConstructor }).Stripe;
}

let stripeJs: Promise<StripeConstructor | null> | null = null;

/**
 * Loads Stripe.js from Stripe's CDN once and resolves the `Stripe` global.
 * Resolves `null` when there is no DOM (server-side import), so the module stays
 * safe to import in isomorphic code.
 */
function loadStripeJs(): Promise<StripeConstructor | null> {
  if (stripeJs) return stripeJs;

  stripeJs = new Promise<StripeConstructor | null>((resolve, reject) => {
    if (typeof window === "undefined" || typeof document === "undefined") {
      resolve(null);
      return;
    }
    const preloaded = readStripeGlobal();
    if (preloaded) {
      resolve(preloaded);
      return;
    }

    const existing = document.querySelector<HTMLScriptElement>(
      'script[src^="https://js.stripe.com/"]',
    );
    const script = existing ?? document.createElement("script");

    script.addEventListener("load", () => {
      const loaded = readStripeGlobal();
      if (loaded) {
        resolve(loaded);
      } else {
        reject(new Error("Stripe.js loaded but window.Stripe is unavailable"));
      }
    });
    script.addEventListener("error", () => {
      stripeJs = null;
      reject(new Error("Failed to load Stripe.js"));
    });

    if (existing) {
      // A script tag is already present; if it has finished loading the guard
      // above returned, otherwise the listeners handle it.
      return;
    }

    const parent = document.head ?? document.body;
    if (!parent) {
      stripeJs = null;
      reject(new Error("Cannot load Stripe.js before <head> or <body> exists"));
      return;
    }
    script.src = STRIPE_JS_URL;
    parent.appendChild(script);
  });

  return stripeJs;
}

/** Owns Stripe's provider-specific embedded checkout lifecycle for storefronts. */
export async function mountEmbeddedCheckout(
  action: PaymentClientAction,
  container: HTMLElement,
): Promise<EmbeddedCheckoutMount> {
  if (action.type !== "mount_embedded_checkout") {
    throw new TypeError("unsupported payment client action");
  }

  const Stripe = await loadStripeJs();
  if (!Stripe) {
    throw new Error("Stripe.js is unavailable in this environment");
  }

  const stripe = Stripe(action.public_key);
  const checkout = await stripe.createEmbeddedCheckoutPage({
    clientSecret: action.client_token,
  });
  checkout.mount(container);

  return {
    destroy: () => checkout.destroy(),
  };
}
