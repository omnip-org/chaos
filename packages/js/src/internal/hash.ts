/**
 * Shared FNV-1a 32-bit hash core used for deterministic, non-cryptographic
 * storage keys across the SDK.
 */
export function fnv1a32(input: string, seed = 2_166_136_261): number {
  let hash = seed >>> 0;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}
