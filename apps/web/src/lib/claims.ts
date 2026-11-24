export const CLAIM_FLAG_REMIXMIX = "RemixMix";
export const CLAIM_FLAG_CLIENT = "Client-X";

export const CLAIM_FLAGS = [CLAIM_FLAG_REMIXMIX, CLAIM_FLAG_CLIENT] as const;

export type ClaimFlag = (typeof CLAIM_FLAGS)[number];

/** Keep only the two known flags, in RemixMix then Client-X order. */
export function normalizeClaimFlags(flags: readonly string[]): ClaimFlag[] {
  const selected = new Set(flags);
  return CLAIM_FLAGS.filter((flag) => selected.has(flag));
}

export function isDualClaim(flags: readonly string[]): boolean {
  const selected = new Set(flags);
  return selected.has(CLAIM_FLAG_REMIXMIX) && selected.has(CLAIM_FLAG_CLIENT);
}
