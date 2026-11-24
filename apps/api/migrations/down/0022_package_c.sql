-- Reverse of 0022_package_c.sql. Drops the seen-set and the Package C columns.
-- pending_receipt itself stays (it belongs to 0020). Also removes ledger version 22.
-- Run this before down/0021_claim_flags.sql and down/0020_package_b.sql.

DROP INDEX IF EXISTS idx_pending_receipt_photo_unique;
DROP TABLE IF EXISTS receipt_asset_seen;

ALTER TABLE pending_receipt DROP COLUMN source;
ALTER TABLE pending_receipt DROP COLUMN payment_kind;
ALTER TABLE pending_receipt DROP COLUMN account_scope;
ALTER TABLE pending_receipt DROP COLUMN flagged_for_review;
ALTER TABLE pending_receipt DROP COLUMN extracted_supplier;

DELETE FROM _sqlx_migrations WHERE version = 22;
