//! Canonical account types for manual accounts and user overrides on linked accounts.
//! TrueLayer's own labels (`TRANSACTION`, `BUSINESS_SAVINGS`, …) stay in `account_type`
//! until the user picks one of these; after that, sync must not overwrite.

pub const CANONICAL_ACCOUNT_TYPES: &[&str] = &[
    "current",
    "savings",
    "credit_card",
    "business_current",
    "business_savings",
    "business_credit_card",
    "sipp",
    "isa",
    "workplace_pension",
    "frozen_pension",
    "trading_account",
    "investment",
    "premium_bonds",
    "mortgage",
    "tax_liability",
    "property",
    "receivables",
    "multi_currency",
];

pub fn is_canonical(t: &str) -> bool {
    CANONICAL_ACCOUNT_TYPES.contains(&t)
}

pub fn kind_for_type(t: &str) -> &'static str {
    match t {
        "credit_card" | "business_credit_card" => "card",
        _ => "account",
    }
}

/// SQL `IN (…)` list of canonical types — values are compile-time constants, not user input.
pub fn canonical_sql_in_list() -> String {
    CANONICAL_ACCOUNT_TYPES
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",")
}
