import http from "k6/http";
import { check } from "k6";
import { Counter } from "k6/metrics";

const checkoutSuccesses = new Counter("checkout_successes");
const capacityVus = Number(__ENV.CAPACITY_VUS || "50");
const warmupDuration = __ENV.CAPACITY_WARMUP || "2m";
const measurementDuration = __ENV.CAPACITY_DURATION || "10m";

export const options = {
  scenarios: {
    warmup: {
      executor: "constant-vus",
      vus: capacityVus,
      duration: warmupDuration,
      tags: { phase: "warmup" },
    },
    measurement: {
      executor: "constant-vus",
      vus: capacityVus,
      startTime: warmupDuration,
      duration: measurementDuration,
      tags: { phase: "measurement" },
    },
  },
  thresholds: {
    "http_req_duration{phase:measurement}": ["p(95)<250", "p(99)<750"],
    "http_req_failed{phase:measurement}": ["rate==0"],
    "checks{phase:measurement}": ["rate==1"],
    "checkout_successes{phase:measurement}": ["rate>=50"],
  },
};

export default function () {
  const unique = `${__VU}-${__ITER}-${Date.now()}`;
  const headers = {
    Authorization: `Bearer ${__ENV.PUBLISHABLE_KEY}`,
    "Content-Type": "application/json",
  };
  const session = http.post(`${__ENV.BASE_URL}/store/v1/shopper-sessions`, "{}", {
    headers,
    tags: { name: "POST /store/v1/shopper-sessions" },
  });
  const shopperToken = session.json("data.shopper_token");
  const shopperHeaders = {
    ...headers,
    "x-chaos-shopper-token": shopperToken,
  };
  const cart = http.post(`${__ENV.BASE_URL}/store/v1/carts`, "{}", {
    headers: { ...shopperHeaders, "Idempotency-Key": `cart-${unique}` },
    tags: { name: "POST /store/v1/carts" },
  });
  const cartId = cart.json("data.id");
  const line = http.put(
    `${__ENV.BASE_URL}/store/v1/carts/${cartId}/lines/${__ENV.PRODUCT_VARIANT_ID}`,
    JSON.stringify({ quantity: 1 }),
    {
      headers: { ...shopperHeaders, "Idempotency-Key": `line-${unique}` },
      tags: { name: "PUT /store/v1/carts/:cart_id/lines/:variant_id" },
    },
  );
  const address = {
    full_name: "Capacity Test",
    address_line1: "1 Load Test Way",
    locality: "Singapore",
    postal_code: "018956",
    country_code: "SG",
  };
  const checkout = http.post(
    `${__ENV.BASE_URL}/store/v1/carts/${cartId}/checkout`,
    JSON.stringify({
      contact: { email: `capacity-${unique}@example.invalid` },
      billing_address: address,
      shipping_address: address,
      shipping_service_id: __ENV.SHIPPING_SERVICE_ID,
    }),
    {
      headers: {
        ...shopperHeaders,
        "Idempotency-Key": `checkout-${unique}`,
      },
      tags: { name: "POST /store/v1/carts/:cart_id/checkout" },
    },
  );
  const succeeded =
    session.status === 201 &&
    cart.status === 201 &&
    line.status === 200 &&
    checkout.status === 201;
  checkoutSuccesses.add(succeeded ? 1 : 0);
  check(checkout, { "checkout flow succeeds": () => succeeded });
}
