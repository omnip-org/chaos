import { sha256Hex } from "../internal/sha256.js";
import { compact, isValidMetaBrowserId } from "../internal/meta.js";
import {
  addToCartEventData,
  initiateCheckoutEventData,
  purchaseEventData,
} from "./meta-payload.js";
import type {
  AddToCartAnalyticsInput,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "./types.js";

/**
 * Server-only Meta Conversions API sender. This module sends the store's
 * Meta access token straight to graph.facebook.com, so it is published only
 * through the separate `@omnip-org/chaos-js/meta-capi` subpath — the main
 * entry (`index.ts`) never imports this file, not even transitively through
 * `ssr/server.ts` (see that file's `ServerEventsPort`), so a browser bundle
 * built from the main entry cannot pull this module in regardless of the
 * bundler's tree-shaking. The storefront supplies `accessToken`/`pixelId`
 * from its own deployment's environment variables; chaos-js never stores or
 * proxies this secret.
 *
 * Ports the wire behavior of the Rust `MetaConversionsDestination` adapter
 * (event_name mapping, hashed `external_id`, `fbc`/`fbp` shape validation).
 * `custom_data` itself comes from `./meta-payload.js`, shared with the
 * browser Pixel sender in `./browser.ts` so both projections of one event
 * always agree on its fields.
 */

const GRAPH_API_DEFAULT_VERSION = "v21.0";

export interface MetaCapiConfig {
  accessToken: string;
  pixelId: string;
  testEventCode?: string;
  /** Graph API version segment, e.g. "v21.0". Defaults to "v21.0". */
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
 * @internal
 */
export async function sendAddToCartCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<AddToCartAnalyticsInput>,
): Promise<void> {
  await postMetaEvent(config, {
    event_name: "AddToCart",
    eventId,
    occurredAt,
    context,
    custom_data: addToCartEventData(input),
  });
}

/**
 * Sends an InitiateCheckout event to Meta's Conversions API.
 * See `sendAddToCartCapi`.
 * @internal
 */
export async function sendInitiateCheckoutCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<InitiateCheckoutAnalyticsInput>,
): Promise<void> {
  await postMetaEvent(config, {
    event_name: "InitiateCheckout",
    eventId,
    occurredAt,
    context,
    custom_data: initiateCheckoutEventData(input),
  });
}

/**
 * Sends a Purchase event to Meta's Conversions API.
 * See `sendAddToCartCapi`.
 * @internal
 */
export async function sendPurchaseCapi(
  config: MetaCapiConfig,
  { eventId, occurredAt, context, input }: CommerceCapiParams<PurchaseAnalyticsInput>,
): Promise<void> {
  await postMetaEvent(config, {
    event_name: "Purchase",
    eventId,
    occurredAt,
    context,
    custom_data: purchaseEventData(input),
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
