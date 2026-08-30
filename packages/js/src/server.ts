import { ChaosApiError } from "./errors.js";
import {
  createStorefrontClient,
  type ChaosStorefrontClient,
  type ClientOptions,
} from "./client.js";
import type {
  AnalyticsCollectionRequest,
  AnalyticsCollectionResult,
  BrowserAnalyticsEvent,
  Cart,
  CartLineMutation,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutOptions,
  OrderStatus,
  PendingPaymentOrder,
  SubmitReviewRequest,
  TrackedOrder,
} from "./types.js";

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
    | "storage"
    | "request"
    | "analytics"
    | "autoAcquireShopperToken"
    | "retryInvalidShopperToken"
  > {
  cookies?: StorefrontCookieJar;
  request?: Pick<Request, "headers">;
  session?: StorefrontSessionOptions;
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

/** Creates the request-scoped server client with cookie-backed shopper identity. */
export function createServerStorefrontClient(
  options: ServerClientOptions,
): ChaosStorefrontClient {
  const { cookies, request, session, ...clientOptions } = options;
  const resolvedSession = resolveSessionOptions(session);
  return createStorefrontClient({
    ...clientOptions,
    storage: cookies
      ? createShopperTokenStorage(
          cookies,
          resolvedSession.shopperTokenCookieName,
          resolvedSession.cookieOptions,
        )
      : null,
    ...(request ? { request } : {}),
    analytics: false,
    autoAcquireShopperToken: false,
    retryInvalidShopperToken: false,
  });
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
  return {
    cart,
    product_variant_id: variantId,
    previous_quantity: previousQuantity,
    new_quantity: line.quantity,
    removed: false,
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
  );
  persistCartSession(cookies, { cart }, options);
  return {
    cart,
    product_variant_id: variantId,
    previous_quantity: line.quantity,
    new_quantity: quantity,
    removed: false,
  };
}

/** Parses and executes the standard no-JavaScript add-to-cart form contract. */
export async function addCartLineFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const form = await readForm(request, "add_cart_line");
  const variantId = form.get("variant_id");
  if (typeof variantId !== "string") {
    throw invalidRequest("variant_id is required");
  }
  const quantity = parseQuantity(form.get("quantity"));
  return addCartLine(
    client,
    cookies,
    { variantId, ...(quantity === undefined ? {} : { quantity }) },
    options,
  );
}

/** Parses and executes the standard cart-line update/remove form contract. */
export async function updateCartLineFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  variantId: string,
  options: StorefrontSessionOptions = {},
): Promise<CartLineMutation> {
  const form = await readForm(request, "update_cart_line");
  const intent = form.get("intent");
  const quantity = parseQuantity(form.get("quantity"));
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
    const activeCart = await client.cart.getActive(existingCartId);
    if (activeCart) {
      persistCartSession(cookies, { cart: activeCart.data }, resolved);
      return createCheckoutWithCart(
        client,
        cookies,
        activeCart.data,
        returnUrl,
        email,
        resolved,
      );
    }

    // The response may have been lost after Chaos locked the source Cart but
    // before the new Cart cookie was written. Resume that Order instead of
    // attempting checkout against a newly created empty Cart.
    if (client.getShopperToken()) {
      const pending = await client.payments.listPendingPaymentOrders();
      const pendingOrder = pending.data.find(
        (order) => order.source_cart_id === existingCartId,
      );
      if (pendingOrder) {
        return resumeCheckoutWithNewCart(
          client,
          cookies,
          pendingOrder.order_id,
          returnUrl,
          resolved,
        );
      }
    }
  }

  const session = await getOrCreateCartSession(client, cookies, resolved);
  return createCheckoutWithCart(
    client,
    cookies,
    session.cart,
    returnUrl,
    email,
    resolved,
  );
}

/** Lists Orders that can still be resumed for the current shopper. */
export async function listPendingPaymentOrdersFromRequest(
  client: ChaosStorefrontClient,
): Promise<PendingPaymentOrder[]> {
  const response = await client.payments.listPendingPaymentOrders();
  return response.data;
}

/** Resumes an Order's persisted payment action and rotates to a new active Cart. */
export async function resumeEmbeddedCheckoutFromRequest(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  request: Request,
  options: StorefrontSessionOptions = {},
): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
  const body = await readJsonRecord(request, "resume_checkout");
  const orderId = body.orderId;
  if (typeof orderId !== "string" || !orderId) {
    throw invalidRequest("orderId is required");
  }
  const returnUrl = body.returnUrl;
  if (returnUrl !== undefined && (typeof returnUrl !== "string" || !returnUrl)) {
    throw invalidRequest("returnUrl is invalid");
  }
  return resumeCheckoutWithNewCart(client, cookies, orderId, returnUrl, options);
}

async function createCheckoutWithCart(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  cart: Cart,
  returnUrl: string,
  email: string | undefined,
  options: StorefrontSessionOptions,
): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
  const response = await client.payments.createEmbeddedCheckoutWithCart(
    cart.id,
    {
      returnUrl,
      ...(email ? { email } : {}),
    },
  );
  persistCartSession(cookies, response.data, options);
  return response;
}

async function resumeCheckoutWithNewCart(
  client: ChaosStorefrontClient,
  cookies: StorefrontCookieJar,
  orderId: string,
  returnUrl: string | undefined,
  options: StorefrontSessionOptions,
): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
  const checkout = await client.payments.resumeEmbeddedCheckout(
    orderId,
    returnUrl === undefined ? undefined : { returnUrl },
  );
  const nextCart = await client.cart.getOrCreate();
  const response = {
    data: {
      checkout: checkout.data,
      cart: nextCart.data,
    },
  } satisfies DataEnvelope<EmbeddedCheckoutCreation>;
  persistCartSession(cookies, response.data, options);
  return response;
}

export async function getOrderStatus(
  client: ChaosStorefrontClient,
  orderId: string,
): Promise<OrderStatus> {
  requireText(orderId, "orderId");
  return client.orders.getStatus(orderId);
}

/** Parses and validates a guest order capability before calling the shared order resource. */
export async function getTrackedOrderFromRequest(
  client: ChaosStorefrontClient,
  request: Request,
): Promise<TrackedOrder> {
  const body = await readJsonRecord(request, "get_tracked_order");
  const token = body.tracking_token;
  if (typeof token !== "string" || !/^ot_[^\s]{1,509}$/.test(token)) {
    throw invalidRequest("tracking_token is invalid");
  }
  const { data } = await client.orders.getTrackedOrder(token);
  return data;
}

/** Parses and submits the standard no-JavaScript product review form. */
export async function createProductReviewFromRequest(
  client: ChaosStorefrontClient,
  request: Request,
  productId: string,
): Promise<void> {
  const form = await readForm(request, "create_product_review");
  const rating = Number(form.get("rating"));
  const content = form.get("content");
  const authorName = form.get("author_name");
  if (
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

  const title = form.get("title");
  const authorEmail = form.get("author_email");
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

/** Parses and forwards the analytics envelope without exposing its wire validation to a storefront. */
export async function collectAnalyticsFromRequest(
  client: ChaosStorefrontClient,
  request: Request,
): Promise<DataEnvelope<AnalyticsCollectionResult>> {
  const body = await readJsonRecord(request, "collect_analytics");
  if (!Array.isArray(body.events) || !body.events.every(isBrowserAnalyticsEvent)) {
    throw invalidRequest("events is required");
  }
  const payload: AnalyticsCollectionRequest = { events: body.events };
  return client.collectAnalytics(payload);
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

async function readForm(request: Request, code: string): Promise<FormData> {
  try {
    return await request.formData();
  } catch {
    throw invalidRequest(`${code} payload is invalid`);
  }
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

function parseQuantity(value: FormDataEntryValue | null): number | undefined {
  if (typeof value !== "string" || !value.trim()) return undefined;
  const quantity = Number(value);
  if (!Number.isSafeInteger(quantity)) {
    throw invalidRequest("quantity must be a positive safe integer");
  }
  return quantity;
}

function requirePositiveQuantity(quantity: number | undefined): number {
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

function isBrowserAnalyticsEvent(value: unknown): value is BrowserAnalyticsEvent {
  if (!isRecord(value)) return false;
  return (
    typeof value.event_id === "string" &&
    typeof value.event_name === "string" &&
    typeof value.occurred_at === "string" &&
    isRecord(value.properties)
  );
}
