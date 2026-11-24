-- B7. Claim flags on a pending receipt.
-- transactions.claim_flags already exists (0020) and stays a JSON array.
-- Allowed values written by the app: "RemixMix" and "Client-X". Both may be set.

ALTER TABLE pending_receipt ADD COLUMN claim_flags TEXT;
