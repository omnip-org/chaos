/**
 * SHA-256 hex digest via native Web Crypto. Used for Meta CAPI's hashed
 * `user_data` fields (e.g. `external_id`), which is the only consumer that
 * needs cryptographic hashing in this package — everything else uses the
 * non-cryptographic `fnv1a32` in `internal/hash.ts`.
 */
export async function sha256Hex(input: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(input),
  );
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
