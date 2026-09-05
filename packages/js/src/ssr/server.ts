import { ChaosApiError } from "../errors.js";
import {
  createStorefrontClient,
  type ChaosStorefrontClient,
  type ClientOptions,
} from "../client.js";
import type { AddToCartAnalyticsInput, InitiateCheckoutAnalyticsInput } from "../events/types.js";
import type {
  Cart,
  CartLine,
  CartLineMutation,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  SubmitReviewRequest,
  OrderLookup,
} from "../types.js";

/**
 * Per-call context a `ServerEventsPort` needs for matching/attribution — the
 * same shape as `ChaosServerEvents`'s `MetaCapiContext`, declared locally so
 * this file (part of the main package entry) has no static import of
 * `events/capi.ts`/`events/server.ts`, the modules that hold the store's
 * Meta access token. TypeScript's structural typing accepts a
 * `ChaosServerEvents` instance here without either side importing the other.
 */
export interface CommerceEventContext {
  eventSourceUrl?: string;
  fbc?: string;
  fbp?: string;
  clientIpAddress?: string;
  clientUserAgent?: string;
  shopperToken?: string;
}

/**
 * Structural contract for server-side commerce event delivery. Construct
 * `ChaosServerEvents` from `@omnip-org/chaos-js/meta-capi` with the store's
 * own Meta access token and pass it as `events` below — or supply any object
 * matching this shape. See `CommerceEventContext` for why this file never
 * imports the concrete class.
 */
export interface ServerEventsPort {
  recordAddToCart(
    input: AddToCartAnalyticsInput,
    context?: CommerceEventContext,
    eventId?: string,
  ): Promise<string>;
  recordInitiateCheckout(
    input: InitiateCheckoutAnalyticsInput,
    context?: CommerceEventContext,
    eventId?: string,
  ): Promise<string>;
  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
    context?: CommerceEventContext,
  ): Promise<void>;
}

export interface StorefrontCookieOptions {
  httpOnly?: boolean;
  path?: string;
  sameSite?: boolean | "strict" | "lax" | "none";
  secure?: boolean;
  maxAge?: number;
}

/** Small adapter contract for Astro cookies, Hono cookies, or a custom Worker wrapper. */
export interface StorefrontCookieJar {
  get(name: string): { value: string } | undefined;
  set(name: string, value: string, options?: StorefrontCookieOptions): void;
}

export interface StorefrontSessionOptions {
  cartCookieName?: string;
  shopperTokenCookieName?: string;
  cookieOptions?: StorefrontCookieOptions;
}

export interface ServerClientOptions
  extends Omit<
    ClientOptions,
    "storage" | "request" | "autoAcquireShopperToken" | "retryInvalidShopperToken"
  > {
  cookies?: StorefrontCookieJar;
  request?: Pick<Request, "headers">;
  session?: StorefrontSessionOptions;
  /**
   * Server-side commerce event delivery (Meta CAPI today — construct
   * `ChaosServerEvents` from `@omnip-org/chaos-js/meta-capi` with the
   * store's own Meta access token). Omit to send Pixel/GA4 only, no
   * server-side event delivery.
   */
  events?: ServerEventsPort;
}

export interface StorefrontSession {
  cart: Cart;
}

export interface AddCartLineInput {
  variantId: string;
  quantity?: number;
}

export interface UpdateCartLineInput {
  variantId: string;
  quantity?: number;
  intent?: "remove";
}

export interface EmbeddedCheckoutRequestInput {
  returnUrl: string;
  email?: string;
}

export const DEFAULT_CART_COOKIE_NAME = "chaos_cart_id";
export const DEFAULT_SHOPPER_TOKEN_COOKIE_NAME = "chaos_shopper_token";

const DEFAULT_SESSION_COOKIE_OPTIONS: StorefrontCookieOptions = {
  httpOnly: true,
  path: "/",
  sameSite: "lax",
  secure: true,
  maxAge: 60 * 60 * 24 * 30,
};

/**
 * `createServerStorefrontClient`'s `events` option, keyed by the client it
 * belongs to instead of a field on `ChaosStorefrontClient` itself — keeps
 * the shared client class free of anything provider-specific.
 */
const serverEventPorts = new WeakMap<ChaosStorefrontClient, ServerEventsPort>();

/**
 * Creates the request-scoped server client with cookie-backed shopper
 * identity. Returns one `ChaosServerClient` object — `chaos.cart`,
 * `chaos.checkout`, `chaos.orders`, `chaos.reviews` already carry this
 * request's `cookies`, so route handlers call methods directly instead of
 * threading a client and a cookie jar through free functions. `chaos.catalog`/
 * `chaos.payments`/`chaos.shopperSession` pass straight through to the
 * low-level Storefront API client (no cookies needed for those reads);
 * `chaos.client` is an escape hatch to that low-level client for anything
 * else (`getShopperToken`, `randomUUID`, `edgeRequestContext`, `cart.get`...).
 */
export function createServerStorefrontClient(
  options: ServerClientOptions,
): ChaosServerClient {
  const { cookies, request, session, events, ...clientOptions } = options;
  if (!cookies) {
    throw new TypeError(
      "createServerStorefrontClient requires cookies for cart/checkout/order session state",
    );
  }
  const resolvedSession = resolveSessionOptions(session);
  const client = createStorefrontClient({
    ...clientOptions,
    storage: createShopperTokenStorage(
      cookies,
      resolvedSession.shopperTokenCookieName,
      resolvedSession.cookieOptions,
    ),
    ...(request ? { request } : {}),
    autoAcquireShopperToken: false,
    retryInvalidShopperToken: false,
  });
  if (events) serverEventPorts.set(client, events);
  return new ChaosServerClient(client, cookies, resolvedSession);
}

/** Bridges the SDK's shopper-token storage to an HttpOnly cookie. */
export function createShopperTokenStorage(
  cookies: StorefrontCookieJar,
  cookieName = DEFAULT_SHOPPER_TOKEN_COOKIE_NAME,
  cookieOptions: StorefrontCookieOptions = DEFAULT_SESSION_COOKIE_OPTIONS,
): Pick<Storage, "getItem" | "setItem" | "removeItem"> {
  return {
    getItem: () => cookies.get(cookieName)?.value ?? null,
    setItem: (_key, value) =>
      cookies.set(cookieName, value, cookieOptions),
    removeItem: () =>
      cookies.set(cookieName, "", {
        ...cookieOptions,
        maxAge: 0,
      }),
  };
}

/** Reads an existing cart or explicitly creates a new cart session. */
export async function getOrCreateCartSession(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  options: StorefrontSessionOptions = {},
): Promise<StorefrontSession> {
  const resolved = resolveSessionOptions(options);
  const existingCartId = cookies.get(resolved.cartCookieName)?.value;
  const { data: cart } = await client.cart.getOrCreate(existingCartId);
  persistCartSession(cookies, { cart }, resolved);
  return { cart };
}

/** Reads an existing active cart without minting a session for a visitor with no cart. */
export async function peekCartSession(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  options: StorefrontSessionOptions = {},
): Promise<StorefrontSession | null> {
  const resolved = resolveSessionOptions(options);
  const existingCartId = cookies.get(resolved.cartCookieName)?.value;
  if (!existingCartId) return null;
  const response = await client.cart.getActive(existingCartId);
  if (response) {
    const session = { cart: response.data };
    persistCartSession(cookies, session, resolved);
    return session;
  }
  clearCookie(cookies, resolved.cartCookieName, resolved.cookieOptions);
  return null;
}

export function persistCartSession(
  cookies: StorefrontCookieJar,
  session: StorefrontSession,
  options: StorefrontSessionOptions = {},
): void {
  const resolved = resolveSessionOptions(options);
  cookies.set(
    resolved.cartCookieName,
    session.cart.id,
    resolved.cookieOptions,
  );
}

/** Adds a line to the active shopper cart and returns the canonical mutation result. */
export async function addCartLine(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  input: AddCartLineInput,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const variantId = requireText(input.variantId, "variantId");
  const quantity = requirePositiveQuantity(input.quantity ?? 1);
  const session = await getOrCreateCartSession(client, cookies, options);
  const previousQuantity =
    session.cart.lines.find((line) => line.product_variant_id === variantId)
      ?.quantity ?? 0;
  const { data: cart } = await client.cart.addLine(
    session.cart.id,
    variantId,
    quantity,
  );
  persistCartSession(cookies, { cart }, options);
  const line = cart.lines.find((candidate) => candidate.product_variant_id === variantId);
  if (!line) throw new ChaosApiError(502, "cart_line_missing", "cart line missing after mutation");
  const eventId = await dispatchAddToCart(client, cookies, {
    line,
    quantity,
    currency: cart.currency,
    cartId: cart.id,
  });
  return {
    cart,
    product_variant_id: variantId,
    previous_quantity: previousQuantity,
    new_quantity: line.quantity,
    removed: false,
    ...(eventId ? { event_id: eventId } : {}),
  };
}

/** Updates or removes a line while keeping quantity validation in the shared commerce layer. */
export async function updateCartLine(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  input: UpdateCartLineInput,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const variantId = requireText(input.variantId, "variantId");
  const session = await getOrCreateCartSession(client, cookies, options);
  const line = session.cart.lines.find(
    (candidate) => candidate.product_variant_id === variantId,
  );
  if (!line) {
    throw new ChaosApiError(404, "cart_line_not_found", "cart line not found");
  }

  if (input.intent === "remove") {
    const { data: cart } = await client.cart.removeLine(session.cart.id, variantId);
    persistCartSession(cookies, { cart }, options);
    return {
      cart,
      product_variant_id: variantId,
      previous_quantity: line.quantity,
      new_quantity: 0,
      removed: true,
    };
  }

  const quantity = requirePositiveQuantity(input.quantity);
  if (quantity === line.quantity) {
    return {
      cart: session.cart,
      product_variant_id: variantId,
      previous_quantity: line.quantity,
      new_quantity: line.quantity,
      removed: false,
    };
  }

  const { data: cart } = await client.cart.setLine(
    session.cart.id,
    variantId,
    { quantity },
    session.cart.version,
  );
  persistCartSession(cookies, { cart }, options);
  const newLine = cart.lines.find((candidate) => candidate.product_variant_id === variantId);
  const increase = quantity - line.quantity;
  const eventId =
    increase > 0 && newLine
      ? await dispatchAddToCart(client, cookies, {
          line: newLine,
          quantity: increase,
          currency: cart.currency,
          cartId: cart.id,
        })
      : undefined;
  return {
    cart,
    product_variant_id: variantId,
    previous_quantity: line.quantity,
    new_quantity: quantity,
    removed: false,
    ...(eventId ? { event_id: eventId } : {}),
  };
}

/** Parses and executes the standard JSON add-to-cart request contract. */
export async function addCartLineFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const body = await readJsonRecord(request, "add_cart_line");
  const variantId = body.variant_id;
  if (typeof variantId !== "string") {
    throw invalidRequest("variant_id is required");
  }
  const quantity = optionalPositiveQuantity(body.quantity);
  return addCartLine(
    client,
    cookies,
    { variantId, ...(quantity === undefined ? {} : { quantity }) },
    options,
  );
}

/** Parses and executes the standard JSON cart-line update/remove request contract. */
export async function updateCartLineFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  variantId: string,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const body = await readJsonRecord(request, "update_cart_line");
  const intent = body.intent;
  const quantity = optionalPositiveQuantity(body.quantity);
  return updateCartLine(
    client,
    cookies,
    {
      variantId,
      ...(intent === "remove" ? { intent: "remove" as const } : {}),
      ...(intent !== "remove" && quantity !== undefined ? { quantity } : {}),
    },
    options,
  );
}

/** Creates checkout from the cookie-backed Cart and rotates to a new active Cart. */
export async function createEmbeddedCheckoutFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  options: StorefrontSessionOptions = {},
): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
  const body = await readJsonRecord(request, "create_checkout");
  const returnUrl = body.returnUrl;
  if (typeof returnUrl !== "string" || !returnUrl) {
    throw invalidRequest("returnUrl is required");
  }
  const email = typeof body.email === "string" && body.email ? body.email : undefined;
  const resolved = resolveSessionOptions(options);
  const existingCartId = cookies.get(resolved.cartCookieName)?.value;
  if (existingCartId && client.getShopperToken()) {
    try {
      return await createCheckoutWithCart(
        client,
        cookies,
        existingCartId,
        returnUrl,
        email,
        resolved,
        request.url,
      );
    } catch (error) {
      if (!isRecoverableCartCheckoutError(error)) throw error;
    }
  }

  const session = await getOrCreateCartSession(client, cookies, resolved);
  return createCheckoutWithCart(
    client,
    cookies,
    session.cart.id,
    returnUrl,
    email,
    resolved,
    request.url,
  );
}

async function createCheckoutWithCart(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  cartId: string,
  returnUrl: string,
  email: string | undefined,
  options: StorefrontSessionOptions,
  eventSourceUrl?: string,
): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
  const response = await client.payments.createEmbeddedCheckoutWithCart(
    cartId,
    {
      returnUrl,
      ...(email ? { email } : {}),
    },
  );
  persistCartSession(cookies, response.data, options);
  const eventId =
    response.data.source_cart.status === "active"
      ? await dispatchInitiateCheckout(client, cookies, response.data, eventSourceUrl)
      : undefined;
  return {
    data: {
      ...response.data,
      ...(eventId ? { event_id: eventId } : {}),
    },
  };
}

function isRecoverableCartCheckoutError(error: unknown): boolean {
  return (
    error instanceof ChaosApiError &&
    ([401, 403, 404].includes(error.status) ||
      error.code === "cart_not_active" ||
      error.code === "checkout_cart_already_started")
  );
}

/** Parses and validates a guest order-number + email pair before calling the shared order resource. */
export async function lookupOrderFromRequest(
  client: ChaosStorefrontClient,
  request: Request,
): Promise<OrderLookup> {
  const body = await readJsonRecord(request, "lookup_order");
  const orderNumber = body.order_number;
  const email = body.email;
  if (
    typeof orderNumber !== "string" ||
    !/^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$/.test(orderNumber.trim())
  ) {
    throw invalidRequest("order_number is invalid");
  }
  if (typeof email !== "string" || email.trim().length === 0) {
    throw invalidRequest("email is required");
  }
  const { data } = await client.orders.lookupOrder({
    orderNumber: orderNumber.trim(),
    email: email.trim(),
  });
  return data;
}

/**
 * Sends a confirmed, paid order to the configured server event port
 * (`events` passed to `createServerStorefrontClient`) once. No-op without
 * that config or without a confirmed+paid order — mirrors
 * `ChaosStorefrontAnalytics.recordConfirmedPurchase`, which projects the
 * same order to the browser Pixel and GA4 using the same deterministic,
 * order-derived event ID.
 */
export async function recordConfirmedPurchaseEvent(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  order: Pick<
    OrderLookup,
    "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
  >,
  eventSourceUrl?: string,
): Promise<void> {
  const port = serverEventPorts.get(client);
  if (!port) return;
  await port.recordConfirmedPurchase(order, eventContextFrom(client, cookies, eventSourceUrl));
}

async function dispatchAddToCart(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  input: { line: CartLine; quantity: number; currency: string; cartId: string },
): Promise<string | undefined> {
  const port = serverEventPorts.get(client);
  if (!port) return undefined;
  return port.recordAddToCart(
    {
      cartId: input.cartId,
      productId: input.line.product_id,
      productVariantId: input.line.product_variant_id,
      quantity: input.quantity,
      priceMinor: input.line.unit_price_amount_minor,
      valueMinor: input.line.unit_price_amount_minor * input.quantity,
      currency: input.currency,
    },
    eventContextFrom(client, cookies),
  );
}

async function dispatchInitiateCheckout(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  creation: EmbeddedCheckoutCreation,
  eventSourceUrl: string | undefined,
): Promise<string | undefined> {
  const port = serverEventPorts.get(client);
  if (!port) return undefined;
  return port.recordInitiateCheckout(
    {
      cartId: creation.source_cart.id,
      orderNumber: creation.checkout.order_number,
      valueMinor: creation.source_cart.subtotal_amount_minor,
      currency: creation.source_cart.currency,
      items: creation.source_cart.lines.map((line) => ({
        productId: line.product_id,
        productVariantId: line.product_variant_id,
        quantity: line.quantity,
        priceMinor: line.unit_price_amount_minor,
      })),
    },
    eventContextFrom(client, cookies, eventSourceUrl),
  );
}

function eventContextFrom(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  eventSourceUrl?: string,
): CommerceEventContext {
  const edge = client.edgeRequestContext();
  const fbc = cookies.get("_fbc")?.value;
  const fbp = cookies.get("_fbp")?.value;
  const shopperToken = client.getShopperToken();
  return {
    ...(eventSourceUrl ? { eventSourceUrl } : {}),
    ...(fbc ? { fbc } : {}),
    ...(fbp ? { fbp } : {}),
    ...edge,
    ...(shopperToken ? { shopperToken } : {}),
  };
}

/** Parses and submits the standard JSON product review request contract. */
export async function createProductReviewFromRequest(
  client: ChaosStorefrontClient,
  request: Request,
  productId: string,
): Promise<void> {
  const body = await readJsonRecord(request, "create_product_review");
  const rating = body.rating;
  const content = body.content;
  const authorName = body.author_name;
  if (
    typeof rating !== "number" ||
    !Number.isSafeInteger(rating) ||
    rating < 1 ||
    rating > 5 ||
    typeof content !== "string" ||
    !content.trim() ||
    typeof authorName !== "string" ||
    !authorName.trim()
  ) {
    throw invalidRequest("review payload is invalid");
  }

  const title = body.title;
  const authorEmail = body.author_email;
  const payload: SubmitReviewRequest = {
    rating,
    content: content.trim(),
    author_name: authorName.trim(),
    ...(typeof title === "string" && title.trim()
      ? { title: title.trim() }
      : {}),
    ...(typeof authorEmail === "string" && authorEmail.trim()
      ? { author_email: authorEmail.trim() }
      : {}),
  };
  await client.reviews.submit(requireText(productId, "productId"), payload);
}

export function cartItemCount(cart: Cart): number {
  return cart.lines.reduce((total, line) => total + line.quantity, 0);
}

function resolveSessionOptions(options: StorefrontSessionOptions | undefined) {
  return {
    cartCookieName: options?.cartCookieName ?? DEFAULT_CART_COOKIE_NAME,
    shopperTokenCookieName:
      options?.shopperTokenCookieName ?? DEFAULT_SHOPPER_TOKEN_COOKIE_NAME,
    cookieOptions: {
      ...DEFAULT_SESSION_COOKIE_OPTIONS,
      ...(options?.cookieOptions ?? {}),
    },
  };
}

function clearCookie(
  cookies: StorefrontCookieJar,
  name: string,
  options: StorefrontCookieOptions,
): void {
  cookies.set(name, "", { ...options, maxAge: 0 });
}

async function readJsonRecord(
  request: Request,
  code: string,
): Promise<Record<string, unknown>> {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    throw invalidRequest(`${code} payload is invalid`);
  }
  if (!isRecord(body)) throw invalidRequest(`${code} payload is invalid`);
  return body;
}

function optionalPositiveQuantity(value: unknown): number | undefined {
  return value === undefined ? undefined : requirePositiveQuantity(value);
}

function requirePositiveQuantity(quantity: unknown): number {
  if (typeof quantity !== "number" || !Number.isSafeInteger(quantity) || quantity < 1) {
    throw invalidRequest("quantity must be a positive safe integer");
  }
  return quantity;
}

function requireText(value: string, field: string): string {
  if (!value.trim()) throw invalidRequest(`${field} is required`);
  return value.trim();
}

function invalidRequest(message: string): ChaosApiError {
  return new ChaosApiError(400, "invalid_storefront_request", message);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Cookie/event-aware cart operations — see `ChaosServerClient`. */
class ServerCartResource {
  constructor(
    private readonly client: ChaosStorefrontClient,
    private readonly cookies: StorefrontCookieJar,
    private readonly options: StorefrontSessionOptions,
  ) {}

  getSession(options: StorefrontSessionOptions = this.options): Promise<StorefrontSession> {
    return getOrCreateCartSession(this.client, this.cookies, options);
  }

  peekSession(options: StorefrontSessionOptions = this.options): Promise<StorefrontSession | null> {
    return peekCartSession(this.client, this.cookies, options);
  }

  persistSession(session: StorefrontSession, options: StorefrontSessionOptions = this.options): void {
    persistCartSession(this.cookies, session, options);
  }

  addLine(input: AddCartLineInput, options: StorefrontSessionOptions = this.options): Promise<CartLineMutation> {
    return addCartLine(this.client, this.cookies, input, options);
  }

  addLineFromRequest(
    request: Request,
    options: StorefrontSessionOptions = this.options,
  ): Promise<CartLineMutation> {
    return addCartLineFromRequest(this.client, this.cookies, request, options);
  }

  updateLine(input: UpdateCartLineInput, options: StorefrontSessionOptions = this.options): Promise<CartLineMutation> {
    return updateCartLine(this.client, this.cookies, input, options);
  }

  updateLineFromRequest(
    request: Request,
    variantId: string,
    options: StorefrontSessionOptions = this.options,
  ): Promise<CartLineMutation> {
    return updateCartLineFromRequest(this.client, this.cookies, request, variantId, options);
  }
}

/** Cookie/event-aware checkout operations — see `ChaosServerClient`. */
class ServerCheckoutResource {
  constructor(
    private readonly client: ChaosStorefrontClient,
    private readonly cookies: StorefrontCookieJar,
    private readonly options: StorefrontSessionOptions,
  ) {}

  createFromRequest(
    request: Request,
    options: StorefrontSessionOptions = this.options,
  ): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
    return createEmbeddedCheckoutFromRequest(this.client, this.cookies, request, options);
  }
}

/** Cookie/event-aware order operations — see `ChaosServerClient`. */
class ServerOrdersResource {
  constructor(
    private readonly client: ChaosStorefrontClient,
    private readonly cookies: StorefrontCookieJar,
  ) {}

  lookupFromRequest(request: Request): Promise<OrderLookup> {
    return lookupOrderFromRequest(this.client, request);
  }

  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
    eventSourceUrl?: string,
  ): Promise<void> {
    return recordConfirmedPurchaseEvent(this.client, this.cookies, order, eventSourceUrl);
  }
}

/** Request-parsing review submission — see `ChaosServerClient`. */
class ServerReviewsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  createFromRequest(request: Request, productId: string): Promise<void> {
    return createProductReviewFromRequest(this.client, request, productId);
  }
}

/**
 * The package's primary server-side surface — see `createServerStorefrontClient`.
 */
export class ChaosServerClient {
  /** Escape hatch to the low-level Storefront API client for anything not wrapped below. */
  readonly client: ChaosStorefrontClient;
  readonly catalog: ChaosStorefrontClient["catalog"];
  readonly payments: ChaosStorefrontClient["payments"];
  readonly shopperSession: ChaosStorefrontClient["shopperSession"];
  readonly cart: ServerCartResource;
  readonly checkout: ServerCheckoutResource;
  readonly orders: ServerOrdersResource;
  readonly reviews: ServerReviewsResource;

  constructor(
    client: ChaosStorefrontClient,
    cookies: StorefrontCookieJar,
    sessionOptions: StorefrontSessionOptions = {},
  ) {
    this.client = client;
    this.catalog = client.catalog;
    this.payments = client.payments;
    this.shopperSession = client.shopperSession;
    this.cart = new ServerCartResource(client, cookies, sessionOptions);
    this.checkout = new ServerCheckoutResource(client, cookies, sessionOptions);
    this.orders = new ServerOrdersResource(client, cookies);
    this.reviews = new ServerReviewsResource(client);
  }
}
