import { useEffect, useState } from "react";
import type { Order } from "@omnip-org/chaos-js";
import { createChaosClient } from "../../lib/chaos";

const TRACKING_SESSION_KEY = "chaos.order_tracking.access_token";

export default function OrderTracking() {
  const [order, setOrder] = useState<Order | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const chaos = createChaosClient();
        const fragment = location.hash.slice(1);
        let accessToken = sessionStorage.getItem(TRACKING_SESSION_KEY);
        if (fragment.startsWith("otk_")) {
          history.replaceState(history.state, "", `${location.pathname}${location.search}`);
          const { data } = await chaos.orders.exchangeTrackingKey(fragment);
          accessToken = data.access_token;
          sessionStorage.setItem(TRACKING_SESSION_KEY, accessToken);
          if (!cancelled) setOrder(data.order);
          return;
        }
        if (!accessToken) throw new Error("missing tracking credential");
        const { data } = await chaos.orders.getTrackedOrder(accessToken);
        if (!cancelled) setOrder(data);
      } catch {
        if (!cancelled) setError("This tracking link is invalid or has expired.");
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  if (error) return <p className="text-sm text-red-600">{error}</p>;
  if (!order) return <p className="text-gray-600" role="status">Loading your order…</p>;
  return (
    <div className="space-y-5">
      <div>
        <p className="text-sm text-gray-500">Order</p>
        <h1 className="text-2xl font-semibold">{order.order_number}</h1>
      </div>
      <dl className="grid gap-4 rounded-lg border p-5 sm:grid-cols-3">
        <div><dt className="text-sm text-gray-500">Order</dt><dd className="font-medium capitalize">{order.status}</dd></div>
        <div><dt className="text-sm text-gray-500">Fulfillment</dt><dd className="font-medium capitalize">{order.fulfillment_status.replaceAll("_", " ")}</dd></div>
        <div><dt className="text-sm text-gray-500">Delivery</dt><dd className="font-medium capitalize">{order.delivery_status.replaceAll("_", " ")}</dd></div>
      </dl>
      <div className="rounded-lg border p-5">
        <h2 className="font-medium">Items</h2>
        <ul className="mt-3 divide-y">
          {order.lines.map((line) => <li className="flex justify-between py-3" key={line.product_variant_id}><span>{line.product_title} × {line.quantity}</span><span>{line.total_amount_minor} {order.currency} minor units</span></li>)}
        </ul>
      </div>
    </div>
  );
}
