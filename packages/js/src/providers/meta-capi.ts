import { sha256Hex } from "../internal/sha256.js";
import { toMajorUnits } from "../money.js";
import type {
  AddToCartAnalyticsInput,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "../types.js";

/**
 * Server-only Meta Conversions API sender. This module must never be
 * imported from browser code — it sends the store's Meta access token
 * straight to graph.facebook.com, so it is published as the separate
 * `@omnip-org/chaos-js/meta-capi` subpath instead of the main entry, the
 * same way `/stripe` is kept out of the main entry for the opposite reason
 * (browser-only DOM code). The storefront supplies `accessToken`/`pixelId`
 * from its own deployment's environment variables; chaos-js never stores or
 * proxies this secret.
 *
 * Ports the wire behavior of the Rust `MetaConversionsDestination` adapter
 * (event_name mapping, hashed `external_id`, `fbc`/`fbp` shape validation,
 * value/currency/contents derivation) without its dynamic-JSON parsing,
 * since every call site here already has a typed commerce input.
 */

const GRAPH_API_DEFAULT_VERSION = "v21.0";
const MAX_META_BROWSER_ID_LENGTH = 2_048;

export interface MetaCapiConfig {
  accessToken: string;
  pixelId: string;
  testEventCode?: string;
  /** Graph API version segment, e.g. "v21.0". Defaults to a current stable version. */
  apiVersion?: string;
  fetch?: typeof fetch;
}

/** Per-call context Meta needs for matching/attribution; all fields are optional and best-effort. */
export interface MetaCapiContext {
  eventSourceUrl?: string;
  fbc?: string;
  fbp?: string;
  clientIpAddress?: string;
  clientUserAgent?: string;
  /** Hashed into `user_data.external_id` (SHA-256) — pass the raw shopper token, hashing happens here. */
  shopperToken?: string;
}

export type MetaCapiCommerceEvent =
  | {
      eventName: "add_to_cart";
      eventId: string;
      occurredAt?: Date;
      context?: MetaCapiContext;
      input: AddToCartAnalyticsInput;
    }
  | {
      eventName: "initiate_checkout";
      eventId: string;
      occurredAt?: Date;
      context?: MetaCapiContext;
      input: InitiateCheckoutAnalyticsInput;
    }
  | {
      eventName: "purchase";
      eventId: string;
      occurredAt?: Date;
      context?: MetaCapiContext;
      input: PurchaseAnalyticsInput;
    };

const META_EVENT_NAMES: Record<MetaCapiCommerceEvent["eventName"], string> = {
  add_to_cart: "AddToCart",
  initiate_checkout: "InitiateCheckout",
  purchase: "Purchase",
};

/**
 * Sends one event to Meta's Conversions API. Best-effort: a delivery
 * failure is swallowed rather than thrown, matching every other analytics
 * call site in this SDK — Meta CAPI must never turn a successful commerce
 * operation into a failed one.
 */
export async function sendMetaCapiEvent(
  config: MetaCapiConfig,
  event: MetaCapiCommerceEvent,
): Promise<void> {
  try {
    const fetchImpl = config.fetch ?? globalThis.fetch;
    const url = new URL(
      `https://graph.facebook.com/${config.apiVersion ?? GRAPH_API_DEFAULT_VERSION}/${config.pixelId}/events`,
    );
    url.searchParams.set("access_token", config.accessToken);

    const body = {
      data: [
        compact({
          event_name: META_EVENT_NAMES[event.eventName],
          event_time: Math.floor((event.occurredAt ?? new Date()).getTime() / 1000),
          event_id: event.eventId,
          action_source: "website",
          event_source_url: event.context?.eventSourceUrl,
          user_data: await buildUserData(event.context),
          custom_data: buildCustomData(event),
        }),
      ],
      ...(config.testEventCode ? { test_event_code: config.testEventCode } : {}),
    };

    await fetchImpl(url.toString(), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    // Best-effort — see the doc comment above.
  }
}

async function buildUserData(
  context: MetaCapiContext | undefined,
): Promise<Record<string, unknown>> {
  const externalId = context?.shopperToken
    ? [await sha256Hex(context.shopperToken)]
    : undefined;
  return compact({
    external_id: externalId,
    fbc: isValidMetaBrowserId(context?.fbc) ? context?.fbc : undefined,
    fbp: isValidMetaBrowserId(context?.fbp) ? context?.fbp : undefined,
    client_ip_address: context?.clientIpAddress,
    client_user_agent: context?.clientUserAgent,
  });
}

function commerceItems(
  event: MetaCapiCommerceEvent,
): Array<{ id: string; quantity: number; priceMinor?: number }> {
  if (event.eventName === "add_to_cart") {
    return [
      {
        id: event.input.productVariantId || event.input.productId,
        quantity: event.input.quantity,
        priceMinor: event.input.priceMinor,
      },
    ];
  }
  return event.input.items.map((item) => ({
    id: item.productVariantId || item.productId,
    quantity: item.quantity,
    priceMinor: item.priceMinor,
  }));
}

function buildCustomData(event: MetaCapiCommerceEvent): Record<string, unknown> {
  const currency = event.input.currency.toUpperCase();
  const items = commerceItems(event);
  const contents = items.map((item) =>
    compact({
      id: item.id,
      quantity: item.quantity,
      item_price:
        item.priceMinor !== undefined
          ? toMajorUnits(item.priceMinor, currency)
          : undefined,
    }),
  );
  return compact({
    value: toMajorUnits(event.input.valueMinor, currency),
    currency,
    content_ids: contents.map((content) => content.id),
    contents,
    content_type: "product",
    num_items: items.reduce((total, item) => total + item.quantity, 0),
  });
}

function isValidMetaBrowserId(value: string | undefined): value is string {
  if (!value || value.length > MAX_META_BROWSER_ID_LENGTH) return false;
  const match = /^fb\.\d+\.(\d{13})\.[^\s]+$/.exec(value);
  return match !== null && Number.isSafeInteger(Number(match[1]));
}

function compact<T extends Record<string, unknown>>(
  value: T,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  );
}
