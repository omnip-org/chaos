import type {
  AddToCartAnalyticsInput,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "../analytics-types.js";
import { sha256Hex } from "../internal/sha256.js";
import { toMajorUnits } from "../money.js";

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
 * value/currency/contents derivation). Each event has its own explicit
 * `custom_data` shape below instead of one shared, dynamically-branching
 * builder — every call site here already has a typed commerce input, so
 * there's nothing left to infer.
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
  /**
   * Best-effort delivery-failure hook: called for a network error or a
   * non-2xx Graph API response (e.g. an expired access token), so a store
   * can log or alert instead of delivery failing silently. Never awaited and
   * never allowed to throw back into the caller — delivery stays best-effort
   * either way.
   */
  onError?: (error: unknown, event: { eventName: string; eventId: string }) => void;
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

interface CommerceCapiParams<Input> {
  eventId: string;
  occurredAt?: Date;
  context?: MetaCapiContext;
  input: Input;
}

/**
 * Sends an AddToCart event to Meta's Conversions API. Best-effort: a
 * delivery failure is swallowed rather than thrown, matching every other
 * analytics call site in this SDK — Meta CAPI must never turn a successful
 * commerce operation into a failed one.
 */
export async function sendAddToCartCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<AddToCartAnalyticsInput>,
): Promise<void> {
  const currency = input.currency.toUpperCase();
  const contentId = input.productVariantId || input.productId;
  await postMetaEvent(config, {
    event_name: "AddToCart",
    eventId,
    occurredAt,
    context,
    custom_data: compact({
      value: toMajorUnits(input.valueMinor, currency),
      currency,
      content_ids: [contentId],
      contents: [
        compact({
          id: contentId,
          quantity: input.quantity,
          item_price: toMajorUnits(input.priceMinor, currency),
        }),
      ],
      content_type: "product",
      num_items: input.quantity,
    }),
  });
}

/** Sends an InitiateCheckout event to Meta's Conversions API. See `sendAddToCartCapi`. */
export async function sendInitiateCheckoutCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<InitiateCheckoutAnalyticsInput>,
): Promise<void> {
  const currency = input.currency.toUpperCase();
  const contents = input.items.map((item) =>
    compact({
      id: item.productVariantId || item.productId,
      quantity: item.quantity,
      item_price: toMajorUnits(item.priceMinor, currency),
    }),
  );
  await postMetaEvent(config, {
    event_name: "InitiateCheckout",
    eventId,
    occurredAt,
    context,
    custom_data: compact({
      value: toMajorUnits(input.valueMinor, currency),
      currency,
      content_ids: contents.map((content) => content.id),
      contents,
      content_type: "product",
      num_items: input.items.reduce((total, item) => total + item.quantity, 0),
    }),
  });
}

/** Sends a Purchase event to Meta's Conversions API. See `sendAddToCartCapi`. */
export async function sendPurchaseCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<PurchaseAnalyticsInput>,
): Promise<void> {
  const currency = input.currency.toUpperCase();
  const contents = input.items.map((item) =>
    compact({
      id: item.productVariantId || item.productId,
      quantity: item.quantity,
      item_price: toMajorUnits(item.priceMinor, currency),
    }),
  );
  await postMetaEvent(config, {
    event_name: "Purchase",
    eventId,
    occurredAt,
    context,
    custom_data: compact({
      value: toMajorUnits(input.valueMinor, currency),
      currency,
      content_ids: contents.map((content) => content.id),
      contents,
      content_type: "product",
      num_items: input.items.reduce((total, item) => total + item.quantity, 0),
    }),
  });
}

/** Shared HTTP plumbing — building `custom_data` is each event's own job; sending it isn't. */
async function postMetaEvent(
  config: MetaCapiConfig,
  event: {
    event_name: string;
    eventId: string;
    occurredAt: Date | undefined;
    context: MetaCapiContext | undefined;
    custom_data: Record<string, unknown>;
  },
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
          event_name: event.event_name,
          event_time: Math.floor((event.occurredAt ?? new Date()).getTime() / 1000),
          event_id: event.eventId,
          action_source: "website",
          event_source_url: event.context?.eventSourceUrl,
          user_data: await buildUserData(event.context),
          custom_data: event.custom_data,
        }),
      ],
      ...(config.testEventCode ? { test_event_code: config.testEventCode } : {}),
    };

    const response = await fetchImpl(url.toString(), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      reportCapiError(
        config,
        new Error(`Meta CAPI request failed with status ${response.status}`),
        event,
      );
    }
  } catch (error) {
    reportCapiError(config, error, event);
  }
}

function reportCapiError(
  config: MetaCapiConfig,
  error: unknown,
  event: { event_name: string; eventId: string },
): void {
  try {
    config.onError?.(error, { eventName: event.event_name, eventId: event.eventId });
  } catch {
    // onError must never break delivery — see MetaCapiConfig.onError.
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
