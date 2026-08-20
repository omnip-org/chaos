import type { ChaosStorefrontClient } from "../client.js";
import type {
  CreateCustomerAddressRequest,
  CursorPageParams,
  Customer,
  CustomerAddress,
  CustomerMutationResult,
  DataEnvelope,
  Order,
  PageEnvelope,
  UpdateCustomerRequest,
} from "../types.js";

export class CustomerResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  /** Associates the current possession-bound shopper with the authenticated Customer. */
  async associate(idempotencyKey?: string): Promise<DataEnvelope<Customer>> {
    const response = await this.client.request<DataEnvelope<Customer>>("/customer/associate", {
      method: "POST",
      requiresShopperToken: true,
      requiresCustomerSession: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
    const identity = this.client.analytics?.identityLinkInput();
    if (identity) {
      void this.client
        .request("/analytics/identity-links", {
          method: "POST",
          body: identity,
          requiresCustomerSession: true,
          idempotencyKey: this.client.randomUUID(),
        })
        .catch(() => {});
    }
    return response;
  }

  get(): Promise<DataEnvelope<Customer>> {
    return this.client.request("/customer", { method: "GET", requiresCustomerSession: true });
  }

  update(body: UpdateCustomerRequest, idempotencyKey?: string): Promise<DataEnvelope<Customer>> {
    return this.client.request("/customer", {
      method: "PUT",
      body,
      requiresCustomerSession: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }

  createAddress(
    body: CreateCustomerAddressRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<CustomerAddress>> {
    return this.client.request("/customer/addresses", {
      method: "POST",
      body,
      requiresCustomerSession: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }

  deleteAddress(addressId: string, idempotencyKey?: string): Promise<DataEnvelope<CustomerMutationResult>> {
    return this.client.request(`/customer/addresses/${encodeURIComponent(addressId)}`, {
      method: "DELETE",
      requiresCustomerSession: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }

  listOrders(params: CursorPageParams = {}): Promise<PageEnvelope<Order>> {
    return this.client.request("/customer/orders", {
      method: "GET",
      query: params,
      requiresCustomerSession: true,
    });
  }
}
