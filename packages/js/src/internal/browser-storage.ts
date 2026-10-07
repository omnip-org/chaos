export type BrowserStorage = Pick<
  Storage,
  "getItem" | "setItem" | "removeItem"
>;

export interface StorefrontStorageKeys {
  shopperToken: string;
  shopperId: string;
  attributionUtmFirst: string;
}

/** All Chaos-owned browser keys share one versioned, store-scoped namespace. */
export function storefrontStorageKeys(
  baseUrl: string,
  publishableKey: string,
): StorefrontStorageKeys {
  const scope = fnv1a32(`${baseUrl}\0${publishableKey}`).toString(36);
  const root = `chaos.storefront.v1.${scope}`;
  return {
    shopperToken: `${root}.shopper.token`,
    shopperId: `${root}.shopper.id`,
    attributionUtmFirst: `${root}.attribution.utm.first`,
  };
}

function fnv1a32(input: string): number {
  let hash = 2_166_136_261;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}
