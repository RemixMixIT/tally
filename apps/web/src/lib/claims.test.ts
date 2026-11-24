import { describe, expect, it } from "vitest";
import { isDualClaim, normalizeClaimFlags } from "./claims";

describe("claim flags", () => {
  it("marks RemixMix, Client-X, and both", () => {
    expect(normalizeClaimFlags(["RemixMix"])).toEqual(["RemixMix"]);
    expect(normalizeClaimFlags(["Client-X"])).toEqual(["Client-X"]);
    expect(normalizeClaimFlags(["Client-X", "RemixMix", "RemixMix"])).toEqual([
      "RemixMix",
      "Client-X",
    ]);
    expect(isDualClaim(["RemixMix", "Client-X"])).toBe(true);
    expect(isDualClaim(["RemixMix"])).toBe(false);
    expect(isDualClaim(["Client-X"])).toBe(false);
  });

  it("drops anything that is not one of the two flags", () => {
    expect(normalizeClaimFlags(["remixmix", "Other"])).toEqual([]);
  });
});
