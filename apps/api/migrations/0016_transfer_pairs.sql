-- 0016: bidirectional transfer pairing. Both bank legs stay; each points at the other.
ALTER TABLE transactions ADD COLUMN transfer_pair_id INTEGER REFERENCES transactions(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_transactions_transfer_pair ON transactions(transfer_pair_id);
