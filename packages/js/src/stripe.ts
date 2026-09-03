import type { PaymentClientAction } from "./types.js";

export interface EmbeddedCheckoutMount {
  /** Removes the checkout from the DOM; it can be mounted again. */
  unmount(): void;
  /** Removes and destroys the checkout; create a new instance to show it again. */
  destroy(): void;
}

/**
 * A Stripe Embedded Checkout analytics event. Shape is owned by Stripe.js and
 * left opaque here so this package stays dependency-free; narrow it at the call
 * site if needed.
 */
export type EmbeddedCheckoutAnalyticsEvent = {
  eventType: string;
  [key: string]: unknown;
};

export interface MountEmbeddedCheckoutOptions {
  /**
   * Called when checkout completes without a redirect. Only fires when the
   * Checkout Session was created with
   * `redirect_on_completion: "never" | "if_required"`; otherwise Stripe
   * redirects to the session's `return_url` instead.
   */
  onComplete?: () => void;
  /** Stripe Embedded Checkout analytics events during the session. */
  onAnalyticsEvent?: (event: EmbeddedCheckoutAnalyticsEvent) => void;
  /**
   * Provides the Checkout Session client secret lazily. Use it to resume the
   * same session after a reload or a remount instead of creating a new one.
   * When given, it is used in place of `action.client_token`.
   */
  fetchClientSecret?: () => Promise<string>;
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

interface StripeEmbeddedCheckoutPageOptions {
  clientSecret?: string;
  fetchClientSecret?: () => Promise<string>;
  onComplete?: () => void;
  onAnalyticsEvent?: (event: EmbeddedCheckoutAnalyticsEvent) => void;
}

interface StripeInstance {
  createEmbeddedCheckoutPage(
    options: StripeEmbeddedCheckoutPageOptions,
  ): Promise<StripeEmbeddedCheckoutHandle>;
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
        stripeJs = null;
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
  options: MountEmbeddedCheckoutOptions = {},
): Promise<EmbeddedCheckoutMount> {
  if (action.type !== "mount_embedded_checkout") {
    throw new TypeError("unsupported payment client action");
  }

  const Stripe = await loadStripeJs();
  if (!Stripe) {
    throw new Error("Stripe.js is unavailable in this environment");
  }

  const pageOptions: StripeEmbeddedCheckoutPageOptions = options.fetchClientSecret
    ? { fetchClientSecret: options.fetchClientSecret }
    : { clientSecret: action.client_token };
  if (options.onComplete) pageOptions.onComplete = options.onComplete;
  if (options.onAnalyticsEvent) pageOptions.onAnalyticsEvent = options.onAnalyticsEvent;

  const stripe = Stripe(action.public_key);
  const checkout = await stripe.createEmbeddedCheckoutPage(pageOptions);
  checkout.mount(container);

  return {
    unmount: () => checkout.unmount(),
    destroy: () => checkout.destroy(),
  };
}
