-- Reverse of 0021_claim_flags.sql. Removes the pending-receipt flag column only.
-- Transaction claim flags from 0020 stay. Also removes ledger version 21.
-- Run this before down/0020_package_b.sql. That script drops pending_receipt.

ALTER TABLE pending_receipt DROP COLUMN claim_flags;

DELETE FROM _sqlx_migrations WHERE version = 21;
