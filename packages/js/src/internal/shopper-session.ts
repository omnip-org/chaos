import { ChaosApiError } from "../errors.js";
import { scopedStorageKey } from "./hash.js";
import {
  firstTouchUtmTags,
  lastTouchUtmTags,
  recordPageUtm,
} from "./utm.js";
import type { CheckoutUtm, DataEnvelope, ShopperSession } from "../types.js";

const SHOPPER_TOKEN_STORAGE_PREFIX = "chaos.storefront.shopper_token";
const SHOPPER_ID_STORAGE_PREFIX = "chaos.storefront.shopper_id";

export type StorefrontStorage = Pick<
  Storage,
  "getItem" | "setItem" | "removeItem"
>;

interface ShopperSessionStoreOptions {
  baseUrl: string;
  publishableKey: string;
  storage?: StorefrontStorage | null;
  autoAcquire: boolean;
  setAnalyticsShopperId: (shopperId: string) => void;
  clearAnalyticsShopperId: () => void;
}

/** Owns the anonymous shopper credential, identity and attribution storage. */
export class ShopperSessionStore {
  private readonly storage: StorefrontStorage | null;
  private readonly tokenStorageKey: string;
  private readonly shopperIdStorageKey: string;
  private shopperToken: string | null;
  private pendingSession: Promise<string> | null = null;
  private mintedHere = false;

  constructor(private readonly options: ShopperSessionStoreOptions) {
    this.storage = resolveStorage(options.storage);
    this.tokenStorageKey = scopedStorageKey(
      SHOPPER_TOKEN_STORAGE_PREFIX,
      options.baseUrl,
      options.publishableKey,
    );
    this.shopperIdStorageKey = scopedStorageKey(
      SHOPPER_ID_STORAGE_PREFIX,
      options.baseUrl,
      options.publishableKey,
    );
    this.shopperToken = this.read(this.tokenStorageKey);
    if (this.shopperToken) {
      const shopperId = this.read(this.shopperIdStorageKey);
      if (shopperId) options.setAnalyticsShopperId(shopperId);
    }
    recordPageUtm(this.storage);
  }

  get token(): string | null {
    return this.shopperToken;
  }

  get wasMintedHere(): boolean {
    return this.mintedHere;
  }

  get attributionStorage(): Pick<Storage, "getItem" | "setItem"> | null {
    return this.storage;
  }

  firstTouchUtm(): CheckoutUtm | undefined {
    return firstTouchUtmTags(this.storage);
  }

  lastTouchUtm(): CheckoutUtm | undefined {
    return lastTouchUtmTags(this.storage);
  }

  setToken(token: string | null): void {
    const tokenChanged = token !== this.shopperToken;
    this.shopperToken = token;
    if (token) this.write(this.tokenStorageKey, token);
    else this.remove(this.tokenStorageKey);

    if (!token || tokenChanged) {
      this.remove(this.shopperIdStorageKey);
      this.options.clearAnalyticsShopperId();
    }
  }

  install(session: ShopperSession): void {
    this.setToken(session.shopper_token);
    this.write(this.shopperIdStorageKey, session.shopper_id);
    this.options.setAnalyticsShopperId(session.shopper_id);
    this.mintedHere = true;
  }

  async acquire(
    issue: () => Promise<DataEnvelope<ShopperSession>>,
  ): Promise<string> {
    if (this.shopperToken) return this.shopperToken;
    if (!this.pendingSession) {
      this.pendingSession = issue()
        .then((response) => response.data.shopper_token)
        .finally(() => {
          this.pendingSession = null;
        });
    }
    return this.pendingSession;
  }

  async require(
    issue: () => Promise<DataEnvelope<ShopperSession>>,
  ): Promise<string> {
    if (this.shopperToken) return this.shopperToken;
    if (!this.options.autoAcquire) {
      throw new ChaosApiError(
        401,
        "shopper_token_required",
        "a shopper token is required for this request",
      );
    }
    return this.acquire(issue);
  }

  /** Returns this journey's UTM once per tab for a returning shopper. */
  takeLastSeenUtm(): CheckoutUtm | undefined {
    if (!this.shopperToken || this.mintedHere) return undefined;
    const utm = this.lastTouchUtm();
    if (!utm) return undefined;

    const sessionStorage = resolveSessionStorage();
    const throttleKey = `${this.tokenStorageKey}.last_seen_synced`;
    try {
      if (sessionStorage?.getItem(throttleKey)) return undefined;
      sessionStorage?.setItem(throttleKey, "1");
    } catch {
      // Refresh remains safe when browser storage cannot throttle it.
    }
    return utm;
  }

  private read(key: string): string | null {
    try {
      return this.storage?.getItem(key) ?? null;
    } catch {
      return null;
    }
  }

  private write(key: string, value: string): void {
    try {
      this.storage?.setItem(key, value);
    } catch {
      // Persistence is optional; in-memory authentication remains usable.
    }
  }

  private remove(key: string): void {
    try {
      this.storage?.removeItem(key);
    } catch {
      // Persistence is optional; in-memory authentication remains usable.
    }
  }
}

function resolveStorage(
  explicit: StorefrontStorage | null | undefined,
): StorefrontStorage | null {
  if (explicit !== undefined) return explicit;
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function resolveSessionStorage(): Pick<Storage, "getItem" | "setItem"> | null {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}
