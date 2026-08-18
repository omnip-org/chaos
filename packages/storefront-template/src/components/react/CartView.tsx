import { useEffect, useState } from "react";
import type { Cart } from "@omnip-org/chaos-js";
import { ChaosApiError } from "@omnip-org/chaos-js";
import { createChaosClient } from "../../lib/chaos";
import { getOrCreateCart } from "../../lib/cart";

function formatAmount(amountMinor: number, currency: string): string {
  return `${(amountMinor / 100).toFixed(2)} ${currency}`;
}

interface BillingForm {
  email: string;
  fullName: string;
  addressLine1: string;
  locality: string;
  postalCode: string;
  countryCode: string;
}

const EMPTY_FORM: BillingForm = {
  email: "",
  fullName: "",
  addressLine1: "",
  locality: "",
  postalCode: "",
  countryCode: "",
};

export default function CartView() {
  const [cart, setCart] = useState<Cart | null>(null);
  const [loading, setLoading] = useState(true);
  const [checkingOut, setCheckingOut] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<BillingForm>(EMPTY_FORM);

  function updateField<K extends keyof BillingForm>(field: K, value: BillingForm[K]) {
    setForm((current) => ({ ...current, [field]: value }));
  }

  const formComplete = Object.values(form).every((value) => value.trim().length > 0);

  useEffect(() => {
    getOrCreateCart()
      .then(setCart)
      .catch(() => setError("Could not load your cart."))
      .finally(() => setLoading(false));
  }, []);

  async function updateQuantity(productVariantId: string, quantity: number) {
    if (!cart) return;
    const chaos = createChaosClient();
    const { data } = quantity > 0
      ? await chaos.cart.setLine(cart.id, productVariantId, { quantity })
      : await chaos.cart.removeLine(cart.id, productVariantId);
    setCart(data);
  }

  async function handleCheckout() {
    if (!cart || cart.lines.length === 0 || !formComplete) return;
    setCheckingOut(true);
    setError(null);
    try {
      const chaos = createChaosClient();
      const { data: checkout } = await chaos.checkout.create(cart.id, {
        contact: { email: form.email },
        billing_address: {
          full_name: form.fullName,
          address_line1: form.addressLine1,
          locality: form.locality,
          postal_code: form.postalCode,
          country_code: form.countryCode.toUpperCase(),
        },
      });
      chaos.analytics?.checkoutStarted({ cartId: cart.id, checkoutId: checkout.id });
      const { data: order } = await chaos.checkout.createOrder(checkout.id);
      const { data: attempt } = await chaos.payments.createAttempt(order.id, {
        provider: "stripe_checkout",
        success_url: new URL("/checkout/success?session_id={CHECKOUT_SESSION_ID}", location.origin).toString(),
        cancel_url: new URL("/checkout/cancel", location.origin).toString(),
      });
      const { data: action } = await chaos.payments.getClientAction(attempt.id);
      if (action.type === "redirect_to_checkout") {
        window.location.href = action.client_token;
        return;
      }
      setError("This template only supports the stripe_checkout provider.");
    } catch (caught) {
      setError(caught instanceof ChaosApiError ? caught.message : "Checkout failed. Please try again.");
    } finally {
      setCheckingOut(false);
    }
  }

  if (loading) return <p className="text-gray-500">Loading cart...</p>;
  if (!cart || cart.lines.length === 0) {
    return <p className="text-gray-500">Your cart is empty.</p>;
  }

  return (
    <div className="space-y-6">
      <ul className="divide-y divide-gray-200">
        {cart.lines.map((line) => (
          <li key={line.product_variant_id} className="flex items-center justify-between py-4">
            <div>
              <p className="font-medium">{line.product_title}</p>
              <p className="text-sm text-gray-500">{line.variant_title}</p>
            </div>
            <div className="flex items-center gap-3">
              <input
                type="number"
                min={0}
                value={line.quantity}
                onChange={(event) => updateQuantity(line.product_variant_id, Number(event.target.value))}
                className="w-16 rounded-md border border-gray-300 px-2 py-1 text-sm"
              />
              <span className="w-24 text-right text-sm">
                {formatAmount(line.subtotal_amount_minor, cart.currency)}
              </span>
            </div>
          </li>
        ))}
      </ul>

      <div className="flex items-center justify-between border-t border-gray-200 pt-4 text-lg font-semibold">
        <span>Subtotal</span>
        <span>{formatAmount(cart.subtotal_amount_minor, cart.currency)}</span>
      </div>

      <div className="space-y-4">
        <h2 className="text-sm font-semibold text-gray-900">Billing details</h2>
        <div>
          <label htmlFor="email" className="block text-sm font-medium text-gray-700">
            Email
          </label>
          <input
            id="email"
            type="email"
            required
            value={form.email}
            onChange={(event) => updateField("email", event.target.value)}
            className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            placeholder="you@example.com"
          />
        </div>
        <div>
          <label htmlFor="fullName" className="block text-sm font-medium text-gray-700">
            Full name
          </label>
          <input
            id="fullName"
            type="text"
            required
            value={form.fullName}
            onChange={(event) => updateField("fullName", event.target.value)}
            className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
          />
        </div>
        <div>
          <label htmlFor="addressLine1" className="block text-sm font-medium text-gray-700">
            Address
          </label>
          <input
            id="addressLine1"
            type="text"
            required
            value={form.addressLine1}
            onChange={(event) => updateField("addressLine1", event.target.value)}
            className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
          />
        </div>
        <div className="grid grid-cols-3 gap-4">
          <div>
            <label htmlFor="locality" className="block text-sm font-medium text-gray-700">
              City
            </label>
            <input
              id="locality"
              type="text"
              required
              value={form.locality}
              onChange={(event) => updateField("locality", event.target.value)}
              className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
          <div>
            <label htmlFor="postalCode" className="block text-sm font-medium text-gray-700">
              Postal code
            </label>
            <input
              id="postalCode"
              type="text"
              required
              value={form.postalCode}
              onChange={(event) => updateField("postalCode", event.target.value)}
              className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
          <div>
            <label htmlFor="countryCode" className="block text-sm font-medium text-gray-700">
              Country
            </label>
            <input
              id="countryCode"
              type="text"
              required
              maxLength={2}
              placeholder="US"
              value={form.countryCode}
              onChange={(event) => updateField("countryCode", event.target.value.toUpperCase())}
              className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm uppercase"
            />
          </div>
        </div>
      </div>

      {error && <p className="text-sm text-red-600">{error}</p>}

      <button
        type="button"
        onClick={handleCheckout}
        disabled={checkingOut || !formComplete}
        className="w-full rounded-md bg-gray-900 px-4 py-3 text-sm font-medium text-white hover:bg-gray-700 disabled:opacity-50"
      >
        {checkingOut ? "Redirecting to Stripe..." : "Checkout with Stripe"}
      </button>
    </div>
  );
}
