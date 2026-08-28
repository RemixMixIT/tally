-- 0014: manual financial assets/liabilities with dated valuations, holding-level
-- SIPP records, property/mortgage/tax/receivable details, and forecast scenarios.
--
-- Additive only. Existing consents, accounts (linked and manual), transactions,
-- CSV imports, brokers, holdings, budgets, bills, and reminders are not rewritten
-- and keep their current semantics. Manual financial_items require no TrueLayer
-- consent and store no credentials.
--
-- Rollback is unsupported: tally's migrator is version-presence only and no prior
-- migration ships a down script. Recover by restoring a pre-0014 SQLite backup.
--
-- Money columns are major units (pounds, dollars, …) as REAL, matching holdings
-- rather than accounts.current_balance_cents. GBP and native currency are stored
-- separately. Forecasts never contribute to actual net worth.

CREATE TABLE financial_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    class TEXT NOT NULL,
    item_type TEXT NOT NULL,
    owner_entity TEXT NOT NULL,
    provider TEXT,
    reference TEXT,
    parent_item_id INTEGER REFERENCES financial_items(id) ON DELETE RESTRICT,
    currency TEXT NOT NULL DEFAULT 'GBP',
    is_asset INTEGER NOT NULL DEFAULT 1,
    is_liability INTEGER NOT NULL DEFAULT 0,
    is_rollup INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    notes TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (class IN (
        'cash', 'investment', 'pension', 'trading',
        'property', 'mortgage', 'tax_liability', 'receivable'
    )),
    CHECK (is_asset IN (0, 1)),
    CHECK (is_liability IN (0, 1)),
    CHECK (is_rollup IN (0, 1)),
    CHECK (is_active IN (0, 1)),
    CHECK (is_asset + is_liability = 1)
);
CREATE INDEX idx_financial_items_class ON financial_items(class);
CREATE INDEX idx_financial_items_parent ON financial_items(parent_item_id);
CREATE INDEX idx_financial_items_active ON financial_items(is_active);
CREATE INDEX idx_financial_items_owner ON financial_items(owner_entity);

CREATE TABLE valuation_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    financial_item_id INTEGER NOT NULL REFERENCES financial_items(id) ON DELETE RESTRICT,
    valuation_date TEXT NOT NULL,
    native_value REAL NOT NULL,
    currency TEXT NOT NULL,
    gbp_value REAL NOT NULL,
    fx_rate REAL,
    fx_rate_date TEXT,
    source_type TEXT NOT NULL,
    source_reference TEXT,
    confidence TEXT NOT NULL,
    contributions REAL,
    withdrawals REAL,
    notes TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (source_type IN (
        'bank_feed', 'statement', 'portal', 'valuation_site',
        'comparable_sale', 'manual_estimate', 'other'
    )),
    CHECK (confidence IN ('confirmed', 'estimated', 'provisional'))
);
CREATE INDEX idx_valuations_item_date ON valuation_snapshots(financial_item_id, valuation_date DESC, id DESC);

-- Holding-level detail for SIPP (and other investment/pension items). Distinct from
-- the existing `holdings` table, which is the broker/Yahoo portfolio on Investments.
CREATE TABLE investment_holdings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    financial_item_id INTEGER NOT NULL REFERENCES financial_items(id) ON DELETE RESTRICT,
    holding_name TEXT NOT NULL,
    ticker_or_identifier TEXT,
    units REAL,
    price REAL,
    price_currency TEXT,
    cost_basis REAL,
    target_allocation REAL,
    target_return REAL,
    valuation_date TEXT NOT NULL,
    native_value REAL NOT NULL,
    currency TEXT NOT NULL,
    gbp_value REAL NOT NULL,
    source_reference TEXT,
    notes TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_investment_holdings_item ON investment_holdings(financial_item_id);

CREATE TABLE property_details (
    financial_item_id INTEGER PRIMARY KEY REFERENCES financial_items(id) ON DELETE RESTRICT,
    region TEXT NOT NULL,
    postcode_area TEXT,
    valuation_method TEXT NOT NULL,
    valuation_url TEXT,
    valuation_cadence TEXT NOT NULL,
    comparable_sales_notes TEXT,
    valuation_confidence TEXT NOT NULL,
    last_reviewed TEXT,
    CHECK (valuation_method IN (
        'portal_estimate', 'comparable_sales', 'surveyor', 'manual_estimate', 'other'
    )),
    CHECK (valuation_cadence IN (
        'monthly', 'quarterly', 'six_monthly', 'yearly', 'ad_hoc'
    )),
    CHECK (valuation_confidence IN ('confirmed', 'estimated', 'provisional'))
);

CREATE TABLE mortgage_details (
    financial_item_id INTEGER PRIMARY KEY REFERENCES financial_items(id) ON DELETE RESTRICT,
    property_item_id INTEGER REFERENCES financial_items(id) ON DELETE RESTRICT,
    repayment_type TEXT NOT NULL,
    outstanding_balance REAL NOT NULL,
    interest_rate REAL,
    rate_type TEXT,
    fixed_period_expiry TEXT,
    payment_amount REAL,
    payment_frequency TEXT,
    term_end_date TEXT,
    overpayment_total REAL,
    projected_balance REAL,
    notes TEXT,
    CHECK (repayment_type IN ('interest_only', 'capital_and_interest'))
);

CREATE TABLE tax_liability_details (
    financial_item_id INTEGER PRIMARY KEY REFERENCES financial_items(id) ON DELETE RESTRICT,
    tax_kind TEXT NOT NULL,
    CHECK (tax_kind IN ('vat', 'corporation_tax', 'personal_tax', 'other'))
);

CREATE TABLE receivable_details (
    financial_item_id INTEGER PRIMARY KEY REFERENCES financial_items(id) ON DELETE RESTRICT,
    debtor TEXT NOT NULL,
    invoice_reference TEXT NOT NULL,
    due_date TEXT NOT NULL,
    expected_amount REAL NOT NULL,
    currency TEXT NOT NULL,
    gbp_value REAL NOT NULL,
    status TEXT NOT NULL,
    CHECK (status IN ('open', 'part_paid', 'overdue', 'settled', 'written_off'))
);

CREATE TABLE forecast_scenarios (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    scenario_type TEXT NOT NULL,
    financial_item_id INTEGER REFERENCES financial_items(id) ON DELETE RESTRICT,
    forecast_start TEXT NOT NULL,
    forecast_end TEXT NOT NULL,
    starting_gbp_value REAL NOT NULL,
    target_return REAL,
    property_growth_rate REAL,
    mortgage_rate REAL,
    repayment_assumption TEXT,
    fx_assumption REAL,
    contributions REAL,
    withdrawals REAL,
    ending_projected_value REAL,
    assumptions TEXT NOT NULL DEFAULT '',
    notes TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (scenario_type IN ('conservative', 'base', 'optimistic'))
);
CREATE INDEX idx_forecasts_item ON forecast_scenarios(financial_item_id);
CREATE INDEX idx_forecasts_type ON forecast_scenarios(scenario_type);
