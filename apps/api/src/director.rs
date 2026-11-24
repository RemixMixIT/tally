//! Director-payment classification for the late accounts period.
//!
//! The period is 16 Jan 2025 through 31 Jan 2026, inclusive, in Europe/London.
//! Both ends fall in GMT, so the bounds are UTC midnights. Tide statement
//! accounts are the TrueLayer accounts whose name starts with "Tide".

use chrono::{DateTime, Datelike, TimeZone, Utc};

pub const CLASSIFICATION_TYPES: &[&str] = &["dividend", "pension", "salary", "expense"];

pub struct DirectorTarget {
    pub name: &'static str,
    pub amount_gbp: &'static str,
}

/// Agreed director expense targets. Reference amounts only; not computed from transactions.
pub const DIRECTOR_TARGETS: &[DirectorTarget] = &[
    DirectorTarget {
        name: "Office",
        amount_gbp: "1416.22",
    },
    DirectorTarget {
        name: "Travel",
        amount_gbp: "4293.00",
    },
    DirectorTarget {
        name: "Software and IT",
        amount_gbp: "1139.35",
    },
];

/// SQL predicate on `accounts a`. Keep in step with [`is_tide_statement_account`].
pub const TIDE_ACCOUNT_SQL: &str = "(\
lower(a.display_name) = 'tide' OR lower(a.display_name) LIKE 'tide %' \
OR lower(ifnull(a.custom_display_name, '')) = 'tide' \
OR lower(ifnull(a.custom_display_name, '')) LIKE 'tide %')";

pub fn is_tide_statement_account(display_name: &str, custom_display_name: Option<&str>) -> bool {
    fn tide(name: &str) -> bool {
        let n = name.trim().to_ascii_lowercase();
        n == "tide" || n.starts_with("tide ")
    }
    tide(display_name) || custom_display_name.is_some_and(tide)
}

/// Inclusive start and exclusive end, unix seconds.
pub fn accounts_period() -> (i64, i64) {
    let start = Utc
        .with_ymd_and_hms(2025, 1, 16, 0, 0, 0)
        .unwrap()
        .timestamp();
    let end = Utc
        .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
        .unwrap()
        .timestamp();
    (start, end)
}

pub fn in_accounts_period(timestamp: i64) -> bool {
    let (start, end) = accounts_period();
    timestamp >= start && timestamp < end
}

pub fn period_label() -> String {
    let (start, end) = accounts_period();
    format!("{} – {}", fmt_date(start), fmt_date(end - 1))
}

fn fmt_date(ts: i64) -> String {
    let d = DateTime::from_timestamp(ts, 0)
        .unwrap_or_else(Utc::now)
        .date_naive();
    let month = match d.month() {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    format!("{} {} {}", d.day(), month, d.year())
}

pub fn classification_type_ok(value: &str) -> bool {
    CLASSIFICATION_TYPES.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_is_the_late_accounts_window() {
        let (start, end) = accounts_period();
        assert_eq!(fmt_date(start), "16 Jan 2025");
        assert_eq!(fmt_date(end - 1), "31 Jan 2026");
        assert!(in_accounts_period(start));
        assert!(in_accounts_period(end - 1));
        assert!(!in_accounts_period(end));
        assert!(!in_accounts_period(start - 1));
        assert_eq!(period_label(), "16 Jan 2025 – 31 Jan 2026");
    }

    #[test]
    fn tide_names_match_the_live_accounts() {
        assert!(is_tide_statement_account("Tide Current Account", None));
        assert!(is_tide_statement_account("Tide Instant Saver", Some("")));
        assert!(is_tide_statement_account(
            "Classic Plus Account",
            Some("Tide Current")
        ));
        assert!(!is_tide_statement_account("Barclays Bank Account", None));
        assert!(!is_tide_statement_account("Smile", None));
        assert!(!is_tide_statement_account("Capital On Tap", None));
        assert!(!is_tide_statement_account("TSB Main Account", None));
    }

    #[test]
    fn targets_match_the_brief() {
        let pairs: Vec<_> = DIRECTOR_TARGETS
            .iter()
            .map(|t| (t.name, t.amount_gbp))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("Office", "1416.22"),
                ("Travel", "4293.00"),
                ("Software and IT", "1139.35"),
            ]
        );
    }
}
