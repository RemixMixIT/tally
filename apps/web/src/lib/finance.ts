/** Manual assets/liabilities (migration 0014). Distinct from Banks accounts and Investments brokers. */

export const ITEM_CLASSES = [
  { value: "cash", label: "Cash" },
  { value: "investment", label: "Investment" },
  { value: "pension", label: "Pension" },
  { value: "trading", label: "Trading" },
  { value: "property", label: "Property" },
  { value: "mortgage", label: "Mortgage" },
  { value: "tax_liability", label: "Tax liability" },
  { value: "receivable", label: "Receivable" },
] as const;

export type ItemClass = (typeof ITEM_CLASSES)[number]["value"];

export const ITEM_TYPES: Record<ItemClass, { value: string; label: string }[]> = {
  cash: [
    { value: "current", label: "Current" },
    { value: "savings", label: "Savings" },
    { value: "fx_account", label: "FX account" },
    { value: "nsi_premium_bonds", label: "NS&I Premium Bonds" },
    { value: "nsi_savings", label: "NS&I savings" },
    { value: "cash_isa", label: "Cash ISA" },
    { value: "other_cash", label: "Other cash" },
  ],
  investment: [
    { value: "isa", label: "ISA" },
    { value: "gia", label: "GIA" },
    { value: "crowdcube", label: "Crowdcube" },
    { value: "fund", label: "Fund" },
    { value: "other_investment", label: "Other investment" },
  ],
  pension: [
    { value: "sipp", label: "SIPP" },
    { value: "workplace_pension", label: "Workplace pension" },
    { value: "frozen_pension", label: "Frozen pension" },
    { value: "other_pension", label: "Other pension" },
  ],
  trading: [
    { value: "trading_account", label: "Trading account" },
    { value: "gia", label: "GIA" },
    { value: "other_trading", label: "Other trading" },
  ],
  property: [
    { value: "residential", label: "Residential" },
    { value: "buy_to_let", label: "Buy to let" },
    { value: "other_property", label: "Other property" },
  ],
  mortgage: [
    { value: "interest_only", label: "Interest only" },
    { value: "capital_and_interest", label: "Capital and interest" },
  ],
  tax_liability: [
    { value: "vat", label: "VAT" },
    { value: "corporation_tax", label: "Corporation tax" },
    { value: "personal_tax", label: "Personal tax" },
    { value: "other", label: "Other tax" },
  ],
  receivable: [
    { value: "invoice", label: "Invoice" },
    { value: "other_receivable", label: "Other receivable" },
  ],
};

export const SOURCE_TYPES = [
  { value: "bank_feed", label: "Bank feed" },
  { value: "statement", label: "Statement" },
  { value: "portal", label: "Portal" },
  { value: "valuation_site", label: "Valuation site" },
  { value: "comparable_sale", label: "Comparable sale" },
  { value: "manual_estimate", label: "Manual estimate" },
  { value: "other", label: "Other" },
] as const;

export const CONFIDENCE = [
  { value: "confirmed", label: "Confirmed" },
  { value: "estimated", label: "Estimated" },
  { value: "provisional", label: "Provisional" },
] as const;

export const OWNER_ENTITIES = [
  { value: "personal", label: "Personal" },
  { value: "joint", label: "Joint" },
  { value: "limited_company", label: "Limited company" },
] as const;

export const VALUATION_METHODS = [
  { value: "portal_estimate", label: "Portal estimate" },
  { value: "comparable_sales", label: "Comparable sales" },
  { value: "surveyor", label: "Surveyor" },
  { value: "manual_estimate", label: "Manual estimate" },
  { value: "other", label: "Other" },
] as const;

export const VALUATION_CADENCES = [
  { value: "monthly", label: "Monthly" },
  { value: "quarterly", label: "Quarterly" },
  { value: "six_monthly", label: "Six-monthly" },
  { value: "yearly", label: "Yearly" },
  { value: "ad_hoc", label: "Ad hoc" },
] as const;

export const REPAYMENT_TYPES = [
  { value: "interest_only", label: "Interest only" },
  { value: "capital_and_interest", label: "Capital and interest" },
] as const;

export const RATE_TYPES = [
  { value: "fixed", label: "Fixed" },
  { value: "tracker", label: "Tracker" },
  { value: "variable", label: "Variable" },
  { value: "other", label: "Other" },
] as const;

export const PAYMENT_FREQUENCIES = [
  { value: "monthly", label: "Monthly" },
  { value: "quarterly", label: "Quarterly" },
  { value: "yearly", label: "Yearly" },
  { value: "ad_hoc", label: "Ad hoc" },
] as const;

export const RECEIVABLE_STATUSES = [
  { value: "open", label: "Open" },
  { value: "part_paid", label: "Part paid" },
  { value: "overdue", label: "Overdue" },
  { value: "settled", label: "Settled" },
  { value: "written_off", label: "Written off" },
] as const;

export const SCENARIO_TYPES = [
  { value: "conservative", label: "Conservative" },
  { value: "base", label: "Base" },
  { value: "optimistic", label: "Optimistic" },
] as const;

export function classLabel(value: string): string {
  if (value === "credit_card") return "Credit card";
  return ITEM_CLASSES.find((c) => c.value === value)?.label ?? value;
}

export function confidenceLabel(value: string): string {
  return CONFIDENCE.find((c) => c.value === value)?.label ?? value;
}

export function isLiabilityClass(cls: string): boolean {
  return cls === "mortgage" || cls === "tax_liability";
}

export function holdingsAllowed(cls: string): boolean {
  return cls === "investment" || cls === "pension";
}

export function isoToday(): string {
  return new Date().toISOString().slice(0, 10);
}

export function parseOptionalNumber(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : NaN;
}

export type SideFilter = "all" | "asset" | "liability";
export type ActiveFilter = "active" | "all" | "inactive";

export function matchesItemFilters(
  item: {
    class: string;
    owner_entity: string;
    is_asset: number;
    is_liability: number;
    is_active: number;
  },
  filters: {
    classFilter: string;
    ownerFilter: string;
    sideFilter: SideFilter;
    activeFilter: ActiveFilter;
  }
): boolean {
  if (filters.classFilter && item.class !== filters.classFilter) return false;
  if (filters.ownerFilter && item.owner_entity !== filters.ownerFilter) return false;
  if (filters.sideFilter === "asset" && item.is_asset !== 1) return false;
  if (filters.sideFilter === "liability" && item.is_liability !== 1) return false;
  if (filters.activeFilter === "active" && item.is_active !== 1) return false;
  if (filters.activeFilter === "inactive" && item.is_active === 1) return false;
  return true;
}
