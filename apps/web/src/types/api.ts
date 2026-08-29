// Match the Rust DTOs in apps/api/src/models.rs and route responses.

export interface Reminder {
  id: number;
  title: string;
  notes: string | null;
  freq: string;
  every_n: number;
  anchor_day: number | null;
  due_at: number;
  notify_before: number;
  notify_enabled: number;
  completed_at: number | null;
  notified_at: number | null;
  archived: number;
  created_at: number;
  updated_at: number;
}

export interface Account {
  id: number;
  consent_id: number | null;
  truelayer_id: string;
  kind: "account" | "card";
  display_name: string;
  iban: string | null;
  sort_code: string | null;
  account_number: string | null;
  card_last4: string | null;
  currency: string;
  enabled: number;
  // 0006
  current_balance_cents: number | null;
  available_balance_cents: number | null;
  overdraft_cents: number | null;
  credit_limit_cents: number | null;
  last_statement_balance_cents: number | null;
  last_statement_date: string | null;
  payment_due_cents: number | null;
  payment_due_date: string | null;
  account_type: string | null;
  card_network: string | null;
  name_on_card: string | null;
  custom_display_name: string | null;
  balance_updated_at: number | null;
  consent_nickname: string | null;
  pending_net_cents: number | null;
}

export interface Transaction {
  id: number;
  account_id: number;
  provider_txn_id: string;
  timestamp: number;
  description: string;
  amount_cents: number;
  currency: string;
  is_credit: number;
  is_pending: number;
  merchant_name: string | null;
  counterparty_iban: string | null;
  counterparty_name: string | null;
  category_id: number | null;
  notes: string | null;
  created_at: number;
  updated_at: number;
}

export interface Category {
  id: number;
  name: string;
  parent_id: number | null;
  icon: string | null;
  colour: string | null;
}

export interface Tag {
  id: number;
  name: string;
}

export interface Rule {
  id: number;
  name: string;
  enabled: number;
  priority: number;
  match_description_regex: string | null;
  match_merchant_regex: string | null;
  match_min_amount_cents: number | null;
  match_max_amount_cents: number | null;
  match_account_id: number | null;
  match_is_credit: number | null;
  set_category_id: number | null;
  add_tag_ids: string | null;
  set_notes: string | null;
  times_applied: number;
  last_applied_at: number | null;
}

export interface Budget {
  id: number;
  name: string;
  category_id: number | null;
  amount_cents: number;
  period: "weekly" | "monthly" | "yearly";
  currency: string;
  rollover: number;
  enabled: number;
}

export interface BudgetStatus {
  budget: Budget;
  spent_cents: number;
  percent: number;
  remaining_cents: number;
  over_budget: boolean;
}

export interface Bill {
  id: number;
  name: string;
  expected_amount_min_cents: number;
  expected_amount_max_cents: number;
  currency: string;
  repeat_freq: string;
  next_expected_date: number | null;
  last_paid_date: number | null;
  match_description_regex: string | null;
  enabled: number;
}

export interface Consent {
  id: number;
  nickname: string;
  enabled: number;
  last_sync_at: number | null;
  last_sync_status: string | null;
  last_sync_error: string | null;
  consent_expires_at: number | null;
}

export interface MeResponse {
  authenticated: boolean;
  awaiting_2fa?: boolean;
  totp_enrolled?: boolean;
  username?: string;
  setup_required: boolean;
}

export interface TransactionsListResponse {
  transactions: Transaction[];
  total: number;
  limit: number;
  offset: number;
}

export interface FinancialItem {
  id: number;
  name: string;
  class: string;
  item_type: string;
  owner_entity: string;
  provider: string | null;
  reference: string | null;
  parent_item_id: number | null;
  currency: string;
  is_asset: number;
  is_liability: number;
  is_rollup: number;
  is_active: number;
  notes: string | null;
  created_at: number;
  updated_at: number;
}

export interface ValuationSnapshot {
  id: number;
  financial_item_id: number;
  valuation_date: string;
  native_value: number;
  currency: string;
  gbp_value: number;
  fx_rate: number | null;
  fx_rate_date: string | null;
  source_type: string;
  source_reference: string | null;
  confidence: string;
  contributions: number | null;
  withdrawals: number | null;
  notes: string | null;
  created_at: number;
  updated_at: number;
}

export interface ItemHolding {
  id: number;
  financial_item_id: number;
  holding_name: string;
  ticker_or_identifier: string | null;
  units: number | null;
  price: number | null;
  price_currency: string | null;
  cost_basis: number | null;
  target_allocation: number | null;
  target_return: number | null;
  valuation_date: string;
  native_value: number;
  currency: string;
  gbp_value: number;
  source_reference: string | null;
  notes: string | null;
}

export interface PropertyDetails {
  financial_item_id: number;
  region: string;
  postcode_area: string | null;
  valuation_method: string;
  valuation_url: string | null;
  valuation_cadence: string;
  comparable_sales_notes: string | null;
  valuation_confidence: string;
  last_reviewed: string | null;
}

export interface MortgageDetails {
  financial_item_id: number;
  property_item_id: number | null;
  repayment_type: string;
  outstanding_balance: number;
  interest_rate: number | null;
  rate_type: string | null;
  fixed_period_expiry: string | null;
  payment_amount: number | null;
  payment_frequency: string | null;
  term_end_date: string | null;
  overpayment_total: number | null;
  projected_balance: number | null;
  notes: string | null;
}

export interface LinkedMortgage {
  item: FinancialItem;
  mortgage: MortgageDetails;
  counted_balance_gbp: number;
}

export interface ReceivableDetails {
  financial_item_id: number;
  debtor: string;
  invoice_reference: string;
  due_date: string;
  expected_amount: number;
  currency: string;
  gbp_value: number;
  status: string;
}

export interface TaxDetails {
  financial_item_id: number;
  tax_kind: string;
}

export interface ForecastScenario {
  id: number;
  name: string;
  scenario_type: string;
  financial_item_id: number | null;
  forecast_start: string;
  forecast_end: string;
  starting_gbp_value: number;
  target_return: number | null;
  property_growth_rate: number | null;
  mortgage_rate: number | null;
  repayment_assumption: string | null;
  fx_assumption: number | null;
  contributions: number | null;
  withdrawals: number | null;
  ending_projected_value: number | null;
  assumptions: string;
  notes: string | null;
}

export interface CountedLine {
  financial_item_id: number | null;
  name: string;
  class: string;
  role: string;
  gbp_value: number;
  valuation_date: string | null;
  source: string;
}

export interface ExcludedLine {
  financial_item_id: number | null;
  name: string;
  reason: string;
}

export interface UnreconciledLine {
  financial_item_id: number;
  name: string;
  parent_gbp: number;
  holdings_gbp: number;
  difference_gbp: number;
  message: string;
}

export interface ClassSubtotal {
  class: string;
  assets_gbp: number;
  liabilities_gbp: number;
}

export interface NetWorthReport {
  as_of: string | null;
  assets_gbp: number;
  liabilities_gbp: number;
  net_worth_gbp: number;
  by_class: ClassSubtotal[];
  counted: CountedLine[];
  excluded: ExcludedLine[];
  unreconciled: UnreconciledLine[];
}

export interface FinancialItemRow {
  item: FinancialItem;
  latest_valuation: ValuationSnapshot | null;
  counted_gbp: number | null;
  counted_source: string | null;
  unreconciled: UnreconciledLine | null;
}
