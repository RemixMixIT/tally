-- 0017: mark a transfer as intentionally unpaired (no matching leg, or history still to import).
ALTER TABLE transactions ADD COLUMN transfer_unmatchable INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_transactions_unmatchable ON transactions(transfer_unmatchable);
