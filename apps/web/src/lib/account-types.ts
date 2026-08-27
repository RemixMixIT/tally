/** Manual/offline account types. Values are stored on `accounts.account_type`.
 *  Existing `current` / `savings` / `credit_card` values stay valid. */
export const MANUAL_ACCOUNT_TYPES = [
  { value: "current", label: "Current" },
  { value: "savings", label: "Savings" },
  { value: "credit_card", label: "Credit Card" },
  { value: "business_current", label: "Business Current" },
  { value: "business_savings", label: "Business Savings" },
  { value: "business_credit_card", label: "Business Credit Card" },
  { value: "sipp", label: "SIPP" },
  { value: "isa", label: "ISA" },
  { value: "workplace_pension", label: "Workplace Pension" },
  { value: "frozen_pension", label: "Frozen Pension" },
  { value: "trading_account", label: "Trading Account" },
  { value: "investment", label: "Investment" },
  { value: "premium_bonds", label: "Premium Bonds" },
  { value: "mortgage", label: "Mortgage" },
  { value: "tax_liability", label: "Tax Liability" },
  { value: "property", label: "Property" },
  { value: "receivables", label: "Receivables" },
  { value: "multi_currency", label: "Multi-currency" },
] as const;

export type ManualAccountType = (typeof MANUAL_ACCOUNT_TYPES)[number]["value"];

/** Map TrueLayer / legacy labels onto the canonical dropdown values. */
const PROVIDER_TYPE_MAP: Record<string, ManualAccountType> = {
  TRANSACTION: "current",
  CURRENT: "current",
  CURRENT_ACCOUNT: "current",
  SAVINGS: "savings",
  CARD: "credit_card",
  CREDIT_CARD: "credit_card",
  BUSINESS_TRANSACTION: "business_current",
  BUSINESS_CURRENT: "business_current",
  BUSINESS_SAVINGS: "business_savings",
  BUSINESS_CARD: "business_credit_card",
  BUSINESS_CREDIT_CARD: "business_credit_card",
};

export function canonicalAccountType(
  raw: string | null | undefined
): ManualAccountType | null {
  if (!raw) return null;
  const exact = MANUAL_ACCOUNT_TYPES.find((t) => t.value === raw);
  if (exact) return exact.value;
  const key = raw.trim().toUpperCase().replace(/\s+/g, "_");
  return PROVIDER_TYPE_MAP[key] ?? null;
}

export function accountTypeLabel(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const canon = canonicalAccountType(raw);
  if (canon) {
    return MANUAL_ACCOUNT_TYPES.find((t) => t.value === canon)?.label ?? canon;
  }
  return raw
    .replace(/_/g, " ")
    .toLowerCase()
    .replace(/^./, (c) => c.toUpperCase());
}
