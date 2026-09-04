/**
 * Shared FNV-1a 32-bit hash core used for deterministic, non-cryptographic
 * storage keys and idempotency keys across the SDK. `avalanche` mixes the
 * hash after every character, which callers combine with distinct seeds to
 * derive several low-collision hashes from the same input (see
 * `stableUuid` in resources/payments.ts).
 */
export function fnv1a32(
  input: string,
  seed = 2_166_136_261,
  avalanche = false,
): number {
  let hash = seed >>> 0;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
    if (avalanche) hash ^= hash >>> 13;
  }
  return hash >>> 0;
}
