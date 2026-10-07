import type {
  EmbeddedCheckoutAnalyticsEvent,
  EmbeddedCheckoutMount,
  PaymentClientAction,
} from "../types.js";

export interface StripeEmbeddedCheckoutOptions {
  onComplete: () => void;
  onAnalyticsEvent?: (event: EmbeddedCheckoutAnalyticsEvent) => void;
  fetchClientSecret?: () => Promise<string>;
}

/**
 * The minimal slice of Stripe.js this module uses. Stripe.js itself is always
 * loaded from https://js.stripe.com at runtime (Stripe does not allow
 * self-hosting or bundling it), so this package carries no `@stripe/stripe-js`
 * dependency.
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
const STRIPE_JS_LOAD_TIMEOUT_MS = 20_000;

/** Reads `window.Stripe` without a global `Window` augmentation that could clash
 * with a consumer that also has `@stripe/stripe-js` types loaded. */
function readStripeGlobal(): StripeConstructor | undefined {
  return (globalThis as { Stripe?: StripeConstructor }).Stripe;
}

let stripeJs: Promise<StripeConstructor> | null = null;

/**
 * Loads Stripe.js from Stripe's CDN once and resolves the `Stripe` global.
 */
function loadStripeJs(): Promise<StripeConstructor> {
  if (stripeJs) return stripeJs;

  stripeJs = new Promise<StripeConstructor>((resolve, reject) => {
    const preloaded = readStripeGlobal();
    if (preloaded) {
      resolve(preloaded);
      return;
    }

    const existing = document.querySelector<HTMLScriptElement>(
      'script[src^="https://js.stripe.com/"]',
    );
    const script = existing ?? document.createElement("script");

    let settled = false;
    const cleanup = () => {
      clearTimeout(timeoutId);
      script.removeEventListener("load", handleLoad);
      script.removeEventListener("error", handleError);
    };
    const fail = (message: string) => {
      if (settled) return;
      settled = true;
      cleanup();
      stripeJs = null;
      reject(new Error(message));
    };
    const finish = () => {
      if (settled) return;
      const loaded = readStripeGlobal();
      if (loaded) {
        settled = true;
        cleanup();
        resolve(loaded);
      } else {
        fail("Stripe.js loaded but window.Stripe is unavailable");
      }
    };
    const handleLoad = () => finish();
    const handleError = () => fail("Failed to load Stripe.js");
    const timeoutId = setTimeout(
      () => fail("Timed out waiting for Stripe.js to load"),
      STRIPE_JS_LOAD_TIMEOUT_MS,
    );

    script.addEventListener("load", handleLoad);
    script.addEventListener("error", handleError);

    if (existing) {
      // Close the race where an eager script finishes after the first global
      // read but before the listeners above are attached. Stripe installs its
      // global synchronously before dispatching `load`, so a microtask recheck
      // catches that state without polling or a second network request.
      queueMicrotask(() => {
        if (readStripeGlobal()) finish();
      });
      return;
    }

    const parent = document.head ?? document.body;
    if (!parent) {
      fail("Cannot load Stripe.js before <head> or <body> exists");
      return;
    }
    script.src = STRIPE_JS_URL;
    script.async = true;
    parent.appendChild(script);
  });

  return stripeJs;
}

/** Owns Stripe's provider-specific embedded checkout lifecycle for storefronts. */
export async function mountEmbeddedCheckout(
  action: PaymentClientAction,
  container: HTMLElement,
  options: StripeEmbeddedCheckoutOptions,
): Promise<EmbeddedCheckoutMount> {
  if (action.type !== "stripe_checkout_embedded") {
    throw new TypeError("unsupported payment client action");
  }

  const Stripe = await loadStripeJs();
  const pageOptions: StripeEmbeddedCheckoutPageOptions = options.fetchClientSecret
    ? { fetchClientSecret: options.fetchClientSecret }
    : { clientSecret: action.client_token };
  pageOptions.onComplete = options.onComplete;
  if (options.onAnalyticsEvent) pageOptions.onAnalyticsEvent = options.onAnalyticsEvent;

  const stripe = Stripe(action.public_key);
  const checkout = await stripe.createEmbeddedCheckoutPage(pageOptions);
  checkout.mount(container);

  return {
    unmount: () => checkout.unmount(),
    destroy: () => checkout.destroy(),
  };
}
