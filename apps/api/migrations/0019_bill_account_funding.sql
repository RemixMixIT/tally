-- Regular payments belong to one Tally account and carry an expected date (already on bills).
-- Backfill Open Banking DD bills from the recurring mandate they were created from.
-- Per-account funding slack (pence) drives "how much to transfer in" for Smile and others.

ALTER TABLE bills ADD COLUMN account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL;
ALTER TABLE bills ADD COLUMN is_inflow INTEGER NOT NULL DEFAULT 0;
ALTER TABLE accounts ADD COLUMN funding_slack_cents INTEGER NOT NULL DEFAULT 0;

UPDATE bills
SET account_id = (
    SELECT r.account_id FROM recurring r WHERE r.id = bills.source_recurring_id
)
WHERE source_recurring_id IS NOT NULL
  AND account_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_bills_account ON bills(account_id);
