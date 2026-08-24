import type { ChaosStorefrontClient } from "../client.js";
import type { CursorPageParams, DataEnvelope, PageEnvelope, Review, SubmitReviewRequest } from "../types.js";

export type ListProductReviewsParams = CursorPageParams;

export class ReviewsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  listForProduct(productId: string, params: ListProductReviewsParams = {}): Promise<PageEnvelope<Review>> {
    return this.client.request(`/products/${encodeURIComponent(productId)}/reviews`, {
      method: "GET",
      query: params,
    });
  }

  submit(productId: string, request: SubmitReviewRequest): Promise<DataEnvelope<{ id: string }>> {
    return this.client.request(`/products/${encodeURIComponent(productId)}/reviews`, {
      method: "POST",
      body: request,
    });
  }
}
