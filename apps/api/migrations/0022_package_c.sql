-- Package C. Additive only. Live store stays SQLite/libSQL state.db.
-- extracted_* are what local OCR read. flagged_for_review is 1 until a receipt is linked.
-- account_scope is business or personal, from the Immich tag that triggered intake.
-- payment_kind is cash, account, or unknown.
-- receipt_asset_seen is the poller's seen-set of Immich asset ids. Do not use
-- asset updatedAfter as the only new-since-last-poll signal.

ALTER TABLE pending_receipt ADD COLUMN extracted_supplier TEXT;
ALTER TABLE pending_receipt ADD COLUMN flagged_for_review INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pending_receipt ADD COLUMN account_scope TEXT;
ALTER TABLE pending_receipt ADD COLUMN payment_kind TEXT;
ALTER TABLE pending_receipt ADD COLUMN source TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pending_receipt_photo_unique ON pending_receipt(photo_id);

CREATE TABLE IF NOT EXISTS receipt_asset_seen (
    asset_id TEXT PRIMARY KEY,
    tag_id TEXT NOT NULL,
    seen_at INTEGER NOT NULL
);
