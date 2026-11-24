-- B8. Portal claim packs. Additive only.
-- One pack per party (RemixMix, then Client-X). A dual claim is a line on both
-- packs and stores both pack ids. The source transaction amount is not changed.
-- transactions.claim_pack_id and claim_pack_id_2 already exist (0020).

CREATE TABLE claim_pack (
    id TEXT PRIMARY KEY,
    party TEXT NOT NULL CHECK (party IN ('RemixMix', 'Client-X')),
    created_at INTEGER NOT NULL
);

CREATE TABLE claim_pack_line (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pack_id TEXT NOT NULL REFERENCES claim_pack(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('transaction', 'pending_receipt')),
    source_id INTEGER NOT NULL,
    amount_cents INTEGER NOT NULL,
    occurred_on TEXT NOT NULL,
    supplier TEXT NOT NULL,
    category TEXT NOT NULL,
    receipt_link TEXT NOT NULL,
    UNIQUE (pack_id, source_kind, source_id)
);

CREATE INDEX idx_claim_pack_line_pack ON claim_pack_line(pack_id);

ALTER TABLE pending_receipt ADD COLUMN claim_pack_id TEXT;
ALTER TABLE pending_receipt ADD COLUMN claim_pack_id_2 TEXT;
