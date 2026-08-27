-- Allow offline/manual accounts that have no TrueLayer consent.
-- Existing connected-account rows are copied unchanged; only consent_id becomes nullable
-- so a manual account can exist without an Open Banking link. SQLite cannot DROP NOT NULL
-- in place, so the table is rebuilt.

CREATE TABLE accounts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    consent_id INTEGER REFERENCES consents(id) ON DELETE CASCADE,
    truelayer_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    display_name TEXT NOT NULL,
    iban TEXT,
    sort_code TEXT,
    account_number TEXT,
    card_last4 TEXT,
    currency TEXT NOT NULL DEFAULT 'GBP',
    firefly_account_id INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    current_balance_cents INTEGER,
    available_balance_cents INTEGER,
    overdraft_cents INTEGER,
    credit_limit_cents INTEGER,
    last_statement_balance_cents INTEGER,
    last_statement_date TEXT,
    payment_due_cents INTEGER,
    payment_due_date TEXT,
    account_type TEXT,
    card_network TEXT,
    name_on_card TEXT,
    custom_display_name TEXT,
    balance_updated_at INTEGER,
    UNIQUE(consent_id, truelayer_id)
);

INSERT INTO accounts_new (
    id, consent_id, truelayer_id, kind, display_name, iban, sort_code,
    account_number, card_last4, currency, firefly_account_id, enabled,
    created_at, updated_at, current_balance_cents, available_balance_cents,
    overdraft_cents, credit_limit_cents, last_statement_balance_cents,
    last_statement_date, payment_due_cents, payment_due_date, account_type,
    card_network, name_on_card, custom_display_name, balance_updated_at
)
SELECT
    id, consent_id, truelayer_id, kind, display_name, iban, sort_code,
    account_number, card_last4, currency, firefly_account_id, enabled,
    created_at, updated_at, current_balance_cents, available_balance_cents,
    overdraft_cents, credit_limit_cents, last_statement_balance_cents,
    last_statement_date, payment_due_cents, payment_due_date, account_type,
    card_network, name_on_card, custom_display_name, balance_updated_at
FROM accounts;

DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;

DELETE FROM sqlite_sequence WHERE name IN ('accounts', 'accounts_new');
INSERT INTO sqlite_sequence (name, seq)
    SELECT 'accounts', IFNULL(MAX(id), 0) FROM accounts;
