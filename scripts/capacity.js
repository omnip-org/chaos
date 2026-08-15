import http from "k6/http";
import { check } from "k6";
import { Counter } from "k6/metrics";

const checkoutSuccesses = new Counter("checkout_successes");

export const options = {
  thresholds: {
    http_req_duration: ["p(95)<250", "p(99)<750"],
    http_req_failed: ["rate==0"],
    checks: ["rate==1"],
    checkout_successes: ["count>30000"],
  },
};

export default function () {
  const unique = `${__VU}-${__ITER}-${Date.now()}`;
  const headers = {
    Authorization: `Bearer ${__ENV.PUBLISHABLE_KEY}`,
    "Content-Type": "application/json",
  };
  const cart = http.post(`${__ENV.BASE_URL}/store/v1/carts`, "{}", {
    headers: { ...headers, "Idempotency-Key": `cart-${unique}` },
  });
  const cartId = cart.json("data.id");
  const line = http.put(
    `${__ENV.BASE_URL}/store/v1/carts/${cartId}/lines/${__ENV.PRODUCT_VARIANT_ID}`,
    JSON.stringify({ quantity: 1 }),
    { headers: { ...headers, "Idempotency-Key": `line-${unique}` } },
  );
  const checkout = http.post(
    `${__ENV.BASE_URL}/store/v1/carts/${cartId}/checkout`,
    "{}",
    { headers: { ...headers, "Idempotency-Key": `checkout-${unique}` } },
  );
  const succeeded = cart.status === 201 && line.status === 200 && checkout.status === 201;
  checkoutSuccesses.add(succeeded ? 1 : 0);
  check(checkout, { "checkout flow succeeds": () => succeeded });
}
