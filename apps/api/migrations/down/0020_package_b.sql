-- Reverse of 0020_package_b.sql. Drops the Package B columns and tables.
-- Also removes ledger version 20 so the up migration can be applied again.
-- Classification rows, pending receipts, receipt links and claim pack ids are removed.

DROP TABLE IF EXISTS director_classification;
DROP TABLE IF EXISTS pending_receipt;

ALTER TABLE transactions DROP COLUMN claim_pack_id_2;
ALTER TABLE transactions DROP COLUMN claim_pack_id;
ALTER TABLE transactions DROP COLUMN claim_flags;
ALTER TABLE transactions DROP COLUMN receipt_photo_id;

DELETE FROM _sqlx_migrations WHERE version = 20;
