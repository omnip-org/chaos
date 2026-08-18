import type { Cart } from "@omnip-org/chaos-js";
import { createChaosClient } from "./chaos";

export const CART_ID_STORAGE_KEY = "storefront.cart_id";

/**
 * Returns the current active Cart, creating one and persisting its id in
 * localStorage on first use. If a stored cart id no longer resolves (e.g.
 * it expired), a fresh Cart is created transparently.
 */
export async function getOrCreateCart(): Promise<Cart> {
  const chaos = createChaosClient();
  const cartId = localStorage.getItem(CART_ID_STORAGE_KEY);
  if (cartId) {
    try {
      const { data } = await chaos.cart.get(cartId);
      if (data.status === "active") return data;
    } catch {
      // Fall through and create a new Cart below.
    }
  }
  const { data } = await chaos.cart.create();
  localStorage.setItem(CART_ID_STORAGE_KEY, data.id);
  return data;
}

export function clearStoredCart(): void {
  localStorage.removeItem(CART_ID_STORAGE_KEY);
}
