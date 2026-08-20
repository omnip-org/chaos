import { useState } from "react";
import { createChaosClient } from "../../lib/chaos";
import { getOrCreateCart } from "../../lib/cart";

interface Props {
  productVariantId: string;
}

export default function AddToCartButton({ productVariantId }: Props) {
  const [state, setState] = useState<"idle" | "adding" | "added" | "error">("idle");

  async function handleClick() {
    setState("adding");
    try {
      const chaos = createChaosClient();
      const cart = await getOrCreateCart();
      await chaos.cart.addLine(cart.id, productVariantId);
      setState("added");
    } catch {
      setState("error");
    }
  }

  return (
    <button
      type="button"
      onClick={handleClick}
      disabled={state === "adding"}
      className="mt-6 w-full rounded-md bg-gray-900 px-4 py-2 text-sm font-medium text-white hover:bg-gray-700 disabled:opacity-50"
    >
      {state === "adding" && "Adding..."}
      {state === "added" && "Added to cart"}
      {state === "error" && "Something went wrong — try again"}
      {state === "idle" && "Add to cart"}
    </button>
  );
}
