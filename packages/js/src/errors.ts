import type { ErrorDetail } from "./types.js";

export class ChaosApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: ErrorDetail[];

  constructor(status: number, code: string, message: string, details: ErrorDetail[] = []) {
    super(message);
    this.name = "ChaosApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export async function throwForResponse(response: Response): Promise<never> {
  let code = "unknown_error";
  let message = `Store API request failed with HTTP ${response.status}`;
  let details: ErrorDetail[] = [];
  try {
    const body = (await response.json()) as { error?: { code?: string; message?: string; details?: ErrorDetail[] } };
    if (body.error) {
      code = body.error.code ?? code;
      message = body.error.message ?? message;
      details = body.error.details ?? [];
    }
  } catch {
    // Response had no parseable JSON error body; fall back to the defaults above.
  }
  throw new ChaosApiError(response.status, code, message, details);
}
