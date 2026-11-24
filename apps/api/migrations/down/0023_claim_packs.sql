-- Reverse of 0023_claim_packs.sql. Drops the pack tables and the pending-receipt
-- pack id columns. Transaction pack id columns from 0020 stay.
-- Run this before down/0022_package_c.sql. Also removes ledger version 23.

DROP TABLE IF EXISTS claim_pack_line;
DROP TABLE IF EXISTS claim_pack;

ALTER TABLE pending_receipt DROP COLUMN claim_pack_id_2;
ALTER TABLE pending_receipt DROP COLUMN claim_pack_id;

DELETE FROM _sqlx_migrations WHERE version = 23;
