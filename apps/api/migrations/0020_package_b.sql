-- Package B (B2). Additive only. Live store is SQLite/libSQL state.db.
-- Amounts are integer pence, matching the rest of Tally.
-- claim_flags is a JSON array of strings (RemixMix, client, and so on).
-- A dual claim uses both pack id columns, one id per pack. A single claim uses claim_pack_id only.
-- director_classification is keyed by (account_id, tide_transaction_id). tide_transaction_id
-- is the TrueLayer transaction_id already stored on transactions.provider_txn_id.

ALTER TABLE transactions ADD COLUMN receipt_photo_id TEXT;
ALTER TABLE transactions ADD COLUMN claim_flags TEXT;
ALTER TABLE transactions ADD COLUMN claim_pack_id TEXT;
ALTER TABLE transactions ADD COLUMN claim_pack_id_2 TEXT;

CREATE TABLE pending_receipt (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    photo_id TEXT NOT NULL,
    extracted_amount INTEGER,
    extracted_date INTEGER,
    status TEXT NOT NULL DEFAULT 'unmatched' CHECK (status IN ('unmatched', 'matched', 'cash')),
    linked_transaction_id INTEGER REFERENCES transactions(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_pending_receipt_status ON pending_receipt(status);
CREATE INDEX idx_pending_receipt_photo ON pending_receipt(photo_id);

CREATE TABLE director_classification (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    tide_transaction_id TEXT NOT NULL,
    classification_type TEXT NOT NULL CHECK (classification_type IN ('dividend', 'pension', 'salary', 'expense')),
    amount INTEGER,
    evidence_link TEXT,
    notes TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (account_id, tide_transaction_id)
);

CREATE INDEX idx_director_classification_account ON director_classification(account_id);
