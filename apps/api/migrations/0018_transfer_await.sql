-- 0018: remember which account a transfer is waiting on until that feed/CSV arrives.
ALTER TABLE transactions ADD COLUMN transfer_await_account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_transactions_await ON transactions(transfer_await_account_id);
