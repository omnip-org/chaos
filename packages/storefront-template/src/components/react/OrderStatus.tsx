import { useEffect, useState } from "react";
import type { Order } from "@omnip-org/chaos-js";
import { createChaosClient } from "../../lib/chaos";
import { CART_ID_STORAGE_KEY } from "../../lib/cart";

export default function OrderStatus({ orderId }: { orderId: string }) {
  const [order, setOrder] = useState<Order | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let attempts = 0;
    const chaos = createChaosClient();

    async function refresh() {
      try {
        const { data } = await chaos.orders.get(orderId);
        if (cancelled) return;
        setOrder(data);
        setError(null);
        if (data.status === "confirmed") {
          localStorage.removeItem(CART_ID_STORAGE_KEY);
          return;
        }
        if (data.status === "pending" && attempts++ < 60) {
          timeout = setTimeout(refresh, 1_000);
        }
      } catch {
        if (cancelled) return;
        setError("We could not load this order. Use the order ID below when contacting support.");
      }
    }

    void refresh();
    return () => {
      cancelled = true;
      if (timeout) clearTimeout(timeout);
    };
  }, [orderId]);

  if (error) return <p className="mt-4 text-sm text-red-600">{error}</p>;
  if (!order || order.status === "pending") {
    return (
      <p className="mt-4 text-gray-600" role="status">
        Payment received by Stripe. Waiting for the verified webhook to confirm your order…
      </p>
    );
  }
  if (order.status === "cancelled") {
    return <p className="mt-4 text-red-600">This order was cancelled.</p>;
  }
  return (
    <div className="mt-4 space-y-2 text-gray-600">
      <p>Your payment is confirmed and the order is ready for fulfillment.</p>
      <p className="text-sm">
        Order: <span className="font-medium">{order.order_number}</span>
      </p>
      <p className="text-sm">
        Fulfillment: <span className="font-medium">{order.fulfillment_status}</span>
      </p>
    </div>
  );
}
