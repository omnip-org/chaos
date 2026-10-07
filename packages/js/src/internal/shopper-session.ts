import { ChaosApiError } from "../errors.js";
import {
  storefrontStorageKeys,
  type BrowserStorage,
  type StorefrontStorageKeys,
} from "./browser-storage.js";
import {
  firstTouchUtmTags,
  recordFirstTouchUtm,
} from "./utm.js";
import type { CheckoutUtm, DataEnvelope, ShopperSession } from "../types.js";

interface ShopperSessionStoreOptions {
  baseUrl: string;
  publishableKey: string;
  storage?: BrowserStorage | null;
  autoAcquire: boolean;
  setAnalyticsShopperId: (shopperId: string) => void;
  clearAnalyticsShopperId: () => void;
}

/** Owns the anonymous shopper credential, identity and attribution storage. */
export class ShopperSessionStore {
  private readonly storage: BrowserStorage | null;
  private readonly keys: StorefrontStorageKeys;
  private shopperToken: string | null;
  private pendingSession: Promise<string> | null = null;
  private mintedHere = false;

  constructor(private readonly options: ShopperSessionStoreOptions) {
    this.storage = resolveStorage(options.storage);
    this.keys = storefrontStorageKeys(
      options.baseUrl,
      options.publishableKey,
    );
    this.shopperToken = this.read(this.keys.shopperToken);
    if (this.shopperToken) {
      const shopperId = this.read(this.keys.shopperId);
      if (shopperId) options.setAnalyticsShopperId(shopperId);
    }
    recordFirstTouchUtm(this.storage, this.keys.attributionUtmFirst);
  }

  get token(): string | null {
    return this.shopperToken;
  }

  get wasMintedHere(): boolean {
    return this.mintedHere;
  }

  firstTouchUtm(): CheckoutUtm | undefined {
    return firstTouchUtmTags(this.storage, this.keys.attributionUtmFirst);
  }

  setToken(token: string | null): void {
    const tokenChanged = token !== this.shopperToken;
    this.shopperToken = token;
    if (token) this.write(this.keys.shopperToken, token);
    else this.remove(this.keys.shopperToken);

    if (!token || tokenChanged) {
      this.mintedHere = false;
      this.remove(this.keys.shopperId);
      this.options.clearAnalyticsShopperId();
    }
  }

  install(session: ShopperSession): void {
    this.setToken(session.shopper_token);
    this.write(this.keys.shopperId, session.shopper_id);
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
  explicit: BrowserStorage | null | undefined,
): BrowserStorage | null {
  if (explicit !== undefined) return explicit;
  try {
    return window.localStorage ?? null;
  } catch {
    return null;
  }
}
