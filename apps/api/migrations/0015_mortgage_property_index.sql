-- 0015: index the existing mortgage → property relationship.
--
-- `mortgage_details.property_item_id` was introduced in 0014 (nullable FK).
-- This migration only adds a lookup index. Existing mortgage rows, including
-- those with a NULL property reference, are left unchanged.

CREATE INDEX IF NOT EXISTS idx_mortgage_details_property
    ON mortgage_details(property_item_id);
