import { describe, expect, it } from "vitest";
import {
  ITEM_CLASSES,
  classLabel,
  holdingsAllowed,
  holdingCostGbp,
  isCashHolding,
  isLiabilityClass,
  matchesItemFilters,
  parseOptionalNumber,
  typeLabel,
} from "./finance";
import { formatGbp } from "./format";

const base = {
  class: "pension",
  owner_entity: "personal",
  is_asset: 1,
  is_liability: 0,
  is_active: 1,
};

describe("finance helpers", () => {
  it("labels every class", () => {
    for (const c of ITEM_CLASSES) {
      expect(classLabel(c.value)).toBe(c.label);
    }
    expect(classLabel("credit_card")).toBe("Credit card");
  });

  it("labels item types, falling back to class", () => {
    expect(typeLabel("tax_liability", "vat")).toBe("VAT");
    expect(typeLabel("mortgage", "interest_only")).toBe("Interest only");
    expect(typeLabel("tax_liability", "")).toBe("Tax liability");
    expect(typeLabel("tax_liability", "custom_levy")).toBe("custom levy");
  });

  it("allows holdings only on investment and pension", () => {
    expect(holdingsAllowed("pension")).toBe(true);
    expect(holdingsAllowed("investment")).toBe(true);
    expect(holdingsAllowed("cash")).toBe(false);
    expect(holdingsAllowed("property")).toBe(false);
  });

  it("converts register holding cost into GBP", () => {
    expect(
      holdingCostGbp({
        cost_basis: 5892.52,
        currency: "GBP",
        native_value: 6235.28,
        gbp_value: 6235.28,
      })
    ).toBeCloseTo(5892.52);
    expect(
      holdingCostGbp({
        cost_basis: 6282.06,
        currency: "USD",
        native_value: 5800,
        gbp_value: 4295.42,
      })
    ).toBeCloseTo(6282.06 * (4295.42 / 5800));
    expect(
      holdingCostGbp({
        cost_basis: null,
        currency: "GBP",
        native_value: 100,
        gbp_value: 100,
      })
    ).toBeNull();
    expect(
      holdingCostGbp({
        cost_basis: null,
        currency: "GBP",
        native_value: 12441.58,
        gbp_value: 12441.58,
        ticker_or_identifier: "CASH",
        holding_name: "AJ Bell SIPP cash",
      })
    ).toBeCloseTo(12441.58);
    expect(
      isCashHolding({ ticker_or_identifier: "CASH", holding_name: "AJ Bell SIPP cash" })
    ).toBe(true);
    expect(isCashHolding({ ticker_or_identifier: "LLY", holding_name: "Eli Lilly" })).toBe(
      false
    );
  });

  it("treats mortgage and tax as liabilities", () => {
    expect(isLiabilityClass("mortgage")).toBe(true);
    expect(isLiabilityClass("tax_liability")).toBe(true);
    expect(isLiabilityClass("receivable")).toBe(false);
  });

  it("filters list rows for the assets page", () => {
    const mtg = { ...base, class: "mortgage", is_asset: 0, is_liability: 1 };
    const inactive = { ...base, is_active: 0 };
    expect(
      matchesItemFilters(base, {
        classFilter: "",
        ownerFilter: "",
        sideFilter: "all",
        activeFilter: "active",
      })
    ).toBe(true);
    expect(
      matchesItemFilters(mtg, {
        classFilter: "",
        ownerFilter: "",
        sideFilter: "asset",
        activeFilter: "active",
      })
    ).toBe(false);
    expect(
      matchesItemFilters(mtg, {
        classFilter: "mortgage",
        ownerFilter: "",
        sideFilter: "liability",
        activeFilter: "active",
      })
    ).toBe(true);
    expect(
      matchesItemFilters(inactive, {
        classFilter: "",
        ownerFilter: "",
        sideFilter: "all",
        activeFilter: "active",
      })
    ).toBe(false);
  });

  it("parses optional numbers the way forms do", () => {
    expect(parseOptionalNumber("")).toBeNull();
    expect(parseOptionalNumber("  ")).toBeNull();
    expect(parseOptionalNumber("1.5")).toBe(1.5);
    expect(Number.isNaN(parseOptionalNumber("nope") as number)).toBe(true);
  });
});

describe("formatGbp", () => {
  it("formats major units not cents", () => {
    expect(formatGbp(120000, "GBP")).toMatch(/120,000/);
    expect(formatGbp(85.5, "EUR")).toContain("85.50");
  });
});
