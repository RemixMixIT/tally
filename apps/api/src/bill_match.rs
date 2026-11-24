//! Match incoming transactions to regular payments (bills).
//!
//! A match needs the same Tally account, a description/merchant regex hit, a date within
//! ±5 days of `next_expected_date`, and the right direction (debit vs inflow credit).
//! Amount inside the expected window (or unknown amount, max = 0) auto-links. Amount
//! outside the window is an exception: the user must choose this-payment-only vs update.

use crate::models::Bill;
use chrono::{Datelike, Duration, Months, NaiveDate, TimeZone, Utc};
use regex::Regex;

pub const DATE_WINDOW_SECS: i64 = 5 * 86_400;

#[derive(Debug, Clone)]
pub struct CompiledBill {
    pub bill_id: i64,
    pub account_id: i64,
    pub is_inflow: bool,
    pub next_expected_date: i64,
    pub min_cents: i64,
    pub max_cents: i64,
    pub regex: Regex,
}

impl CompiledBill {
    pub fn from_bill(b: &Bill) -> Option<Self> {
        if b.enabled == 0 {
            return None;
        }
        let account_id = b.account_id?;
        let next_expected_date = b.next_expected_date?;
        let pattern = b
            .match_description_regex
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let regex = Regex::new(&format!("(?i){pattern}")).ok()?;
        Some(Self {
            bill_id: b.id,
            account_id,
            is_inflow: b.is_inflow != 0,
            next_expected_date,
            min_cents: b.expected_amount_min_cents,
            max_cents: b.expected_amount_max_cents,
            regex,
        })
    }
}

pub fn compile(bills: &[Bill]) -> Vec<CompiledBill> {
    bills.iter().filter_map(CompiledBill::from_bill).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountStatus {
    /// `expected_amount_max_cents == 0` — amount unknown (e.g. Capital On Tap). Not an exception.
    Unknown,
    InWindow,
    OutOfWindow,
}

pub fn amount_status(amount_cents: i64, min: i64, max: i64) -> AmountStatus {
    if max <= 0 {
        AmountStatus::Unknown
    } else if amount_cents >= min && amount_cents <= max {
        AmountStatus::InWindow
    } else {
        AmountStatus::OutOfWindow
    }
}

pub fn date_in_window(txn_ts: i64, expected: i64) -> bool {
    (txn_ts - expected).abs() <= DATE_WINDOW_SECS
}

/// ±5% window around a single figure, matching the create/edit UI.
pub fn window_from_amount(cents: i64) -> (i64, i64) {
    let min = (cents as f64 * 0.95).round() as i64;
    let max = (cents as f64 * 1.05).round() as i64;
    (min.max(0), max.max(min.max(0)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchDecision {
    None,
    Auto { bill_id: i64 },
    Exception { bill_id: i64 },
}

fn text_matches(c: &CompiledBill, description: &str, merchant: Option<&str>) -> bool {
    c.regex.is_match(description) || merchant.map(|m| c.regex.is_match(m)).unwrap_or(false)
}

fn closer(current: Option<(i64, i64)>, bill_id: i64, dist: i64) -> Option<(i64, i64)> {
    match current {
        Some((_, d)) if d <= dist => current,
        _ => Some((bill_id, dist)),
    }
}

/// Classify a transaction against compiled regulars. Amount is applied after the
/// account/regex/date/direction filter so unknown-amount bills never become exceptions.
pub fn decide(
    matchers: &[CompiledBill],
    account_id: i64,
    description: &str,
    merchant: Option<&str>,
    amount_cents: i64,
    is_credit: bool,
    txn_ts: i64,
) -> MatchDecision {
    let mut auto: Option<(i64, i64)> = None; // (bill_id, date_distance)
    let mut exception: Option<(i64, i64)> = None;
    for c in matchers {
        if c.account_id != account_id {
            continue;
        }
        if c.is_inflow != is_credit {
            continue;
        }
        if !date_in_window(txn_ts, c.next_expected_date) {
            continue;
        }
        if !text_matches(c, description, merchant) {
            continue;
        }
        let dist = (txn_ts - c.next_expected_date).abs();
        match amount_status(amount_cents, c.min_cents, c.max_cents) {
            AmountStatus::Unknown | AmountStatus::InWindow => {
                auto = closer(auto, c.bill_id, dist);
            }
            AmountStatus::OutOfWindow => {
                exception = closer(exception, c.bill_id, dist);
            }
        }
    }
    if let Some((bill_id, _)) = auto {
        return MatchDecision::Auto { bill_id };
    }
    if let Some((bill_id, _)) = exception {
        return MatchDecision::Exception { bill_id };
    }
    MatchDecision::None
}

/// Typical expected figure for display/funding: midpoint of the window.
pub fn typical_amount_cents(min: i64, max: i64) -> Option<i64> {
    if max <= 0 {
        None
    } else {
        Some((min + max) / 2)
    }
}

/// How to fund a variable-amount regular (expected max = 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableAmount {
    /// Card PDF is in and has not been repaid yet — this is next month's payment.
    Statement(i64),
    /// No unpaid statement; fall back to the last Smile/Tide debit.
    LastPayment(i64),
    Unknown,
}

fn iso_date_start_ts(iso: &str) -> Option<i64> {
    let d = NaiveDate::parse_from_str(iso.trim(), "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

/// Vanquis (and similar): this month's statement balance is next month's DD.
/// Once a payment posts on or after the statement date, that PDF is settled and
/// the next amount is unknown until the following statement.
pub fn resolve_variable_amount(
    statement_balance_cents: Option<i64>,
    statement_date_iso: Option<&str>,
    last_payment_cents: Option<i64>,
    last_payment_ts: Option<i64>,
) -> VariableAmount {
    if let (Some(bal), Some(date)) = (statement_balance_cents, statement_date_iso) {
        if let Some(stmt_ts) = iso_date_start_ts(date) {
            let paid_after = last_payment_ts.map(|ts| ts >= stmt_ts).unwrap_or(false);
            if !paid_after {
                return VariableAmount::Statement(bal);
            }
        }
    }
    match last_payment_cents {
        Some(cents) => VariableAmount::LastPayment(cents),
        None => VariableAmount::Unknown,
    }
}

/// True when a credit-card account name appears on the repayment regular
/// (`Vanquis` ↔ Vanquis bill, `Capital On Tap` ↔ CAPITAL ON TAP DD).
pub fn card_matches_bill(card_name: &str, bill_name: &str, regex: Option<&str>) -> bool {
    let card = card_name.trim().to_ascii_lowercase();
    if card.len() < 4 {
        return false;
    }
    let blob = format!(
        "{} {}",
        bill_name,
        regex.unwrap_or("")
    )
    .to_ascii_lowercase();
    blob.contains(&card)
}

#[derive(Debug, Clone)]
pub struct FundingLine {
    pub bill_id: i64,
    pub name: String,
    pub expected_date: i64,
    pub expected_amount_cents: i64,
    pub is_inflow: bool,
}

#[derive(Debug, Clone)]
pub struct FundingResult {
    pub remaining_net_out_cents: i64,
    pub transfer_in_cents: i64,
    pub lines: Vec<FundingLine>,
}

/// `transfer_in = max(0, remaining_net_out + slack − current_balance)`.
/// Outflows add to remaining_net_out; inflows subtract.
pub fn funding_forecast(
    balance_cents: i64,
    slack_cents: i64,
    lines: Vec<FundingLine>,
) -> FundingResult {
    let remaining_net_out_cents: i64 = lines
        .iter()
        .map(|l| {
            if l.is_inflow {
                -l.expected_amount_cents
            } else {
                l.expected_amount_cents
            }
        })
        .sum();
    let transfer_in_cents = (remaining_net_out_cents + slack_cents - balance_cents).max(0);
    FundingResult {
        remaining_net_out_cents,
        transfer_in_cents,
        lines,
    }
}

/// One `repeat_freq` step forward from `from_ts`.
pub fn add_one_period(from_ts: i64, freq: &str) -> i64 {
    let dt = Utc
        .timestamp_opt(from_ts, 0)
        .single()
        .unwrap_or_else(Utc::now);
    let next = match freq {
        "weekly" => dt + Duration::days(7),
        "fortnightly" => dt + Duration::days(14),
        "yearly" => dt
            .checked_add_months(Months::new(12))
            .unwrap_or(dt + Duration::days(365)),
        _ => dt
            .checked_add_months(Months::new(1))
            .unwrap_or(dt + Duration::days(30)),
    };
    next.timestamp()
}

/// Inclusive last-second of the UTC calendar month containing `now_ts`.
pub fn calendar_month_end(now_ts: i64) -> i64 {
    let dt = Utc
        .timestamp_opt(now_ts, 0)
        .single()
        .unwrap_or_else(Utc::now);
    month_end(dt.year(), dt.month())
}

pub fn month_start(year: i32, month: u32) -> i64 {
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .unwrap()
        .timestamp()
}

pub fn month_end(year: i32, month: u32) -> i64 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    month_start(ny, nm) - 1
}

pub fn next_calendar_month(now_ts: i64) -> (i32, u32) {
    let dt = Utc
        .timestamp_opt(now_ts, 0)
        .single()
        .unwrap_or_else(Utc::now);
    if dt.month() == 12 {
        (dt.year() + 1, 1)
    } else {
        (dt.year(), dt.month() + 1)
    }
}

pub fn month_label(year: i32, month: u32) -> String {
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .unwrap()
        .format("%B %Y")
        .to_string()
}

/// Occurrences of a regular whose expected date falls in `[range_start, range_end]`,
/// walking forward from `next_expected` by `freq`.
pub fn occurrences_in_range(
    next_expected: i64,
    freq: &str,
    range_start: i64,
    range_end: i64,
) -> Vec<i64> {
    let mut t = next_expected;
    let mut guard = 0;
    while t < range_start && guard < 600 {
        t = add_one_period(t, freq);
        guard += 1;
    }
    let mut out = Vec::new();
    while t <= range_end && guard < 800 {
        if t >= range_start {
            out.push(t);
        }
        t = add_one_period(t, freq);
        guard += 1;
    }
    out
}

/// Unpaid dates still due in the current calendar month: remaining from `now` through
/// month-end, plus an overdue `next_expected` that has not rolled yet.
pub fn remaining_this_month(next_expected: i64, freq: &str, now: i64) -> Vec<i64> {
    let end = calendar_month_end(now);
    let mut dates = occurrences_in_range(next_expected, freq, now, end);
    if next_expected < now && next_expected <= end && !dates.contains(&next_expected) {
        dates.insert(0, next_expected);
    }
    dates
}

/// A remaining regular counts if its next expected date is on or before month-end
/// (includes overdue items still sitting on a past expected date).
pub fn in_funding_horizon(next_expected: i64, month_end_ts: i64) -> bool {
    next_expected <= month_end_ts
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp()
    }

    fn bill(
        id: i64,
        account_id: i64,
        expected: i64,
        min: i64,
        max: i64,
        regex: &str,
        is_inflow: bool,
    ) -> CompiledBill {
        CompiledBill {
            bill_id: id,
            account_id,
            is_inflow,
            next_expected_date: expected,
            min_cents: min,
            max_cents: max,
            regex: Regex::new(&format!("(?i){regex}")).unwrap(),
        }
    }

    #[test]
    fn in_window_amount_auto_links() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 3894, 4304, "KCOM", false)];
        let d = decide(&bills, 10, "KCOM GROUP LTD", None, 4099, false, expected);
        assert_eq!(d, MatchDecision::Auto { bill_id: 1 });
    }

    #[test]
    fn out_of_window_amount_is_exception() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 3894, 4304, "KCOM", false)];
        let d = decide(&bills, 10, "KCOM GROUP LTD", None, 9000, false, expected);
        assert_eq!(d, MatchDecision::Exception { bill_id: 1 });
    }

    #[test]
    fn unknown_amount_is_not_an_exception() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 0, 0, "CAPITAL ON TAP", false)];
        let d = decide(
            &bills,
            10,
            "CAPITAL ON TAP",
            None,
            12_345,
            false,
            expected,
        );
        assert_eq!(d, MatchDecision::Auto { bill_id: 1 });
    }

    #[test]
    fn this_only_vs_update_windows() {
        let (min, max) = window_from_amount(4099);
        assert!(4099 >= min && 4099 <= max);
        // A later payment of 9000 is outside the original window.
        assert_eq!(amount_status(9000, min, max), AmountStatus::OutOfWindow);
        // Updating the regular rebuilds the window from this payment.
        let (min2, max2) = window_from_amount(9000);
        assert_eq!(amount_status(9000, min2, max2), AmountStatus::InWindow);
        // Original window is unchanged for a this-payment-only decision.
        assert_eq!(amount_status(4099, min, max), AmountStatus::InWindow);
    }

    #[test]
    fn date_window_allows_early_and_late() {
        let expected = ts(2026, 9, 10);
        let bills = [bill(1, 10, expected, 1000, 1000, "NETFLIX", false)];
        assert_eq!(
            decide(&bills, 10, "NETFLIX", None, 1000, false, expected - 5 * 86_400),
            MatchDecision::Auto { bill_id: 1 }
        );
        assert_eq!(
            decide(&bills, 10, "NETFLIX", None, 1000, false, expected + 5 * 86_400),
            MatchDecision::Auto { bill_id: 1 }
        );
        assert_eq!(
            decide(&bills, 10, "NETFLIX", None, 1000, false, expected - 6 * 86_400),
            MatchDecision::None
        );
    }

    #[test]
    fn wrong_account_does_not_match() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 3894, 4304, "KCOM", false)];
        assert_eq!(
            decide(&bills, 99, "KCOM GROUP LTD", None, 4099, false, expected),
            MatchDecision::None
        );
    }

    #[test]
    fn credit_does_not_match_outflow() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 3894, 4304, "KCOM", false)];
        assert_eq!(
            decide(&bills, 10, "KCOM GROUP LTD", None, 4099, true, expected),
            MatchDecision::None
        );
    }

    #[test]
    fn inflow_matches_credits() {
        let expected = ts(2026, 8, 27);
        let bills = [bill(2, 10, expected, 237_500, 262_500, "John Lancaster", true)];
        assert_eq!(
            decide(
                &bills,
                10,
                "John Lancaster",
                None,
                250_000,
                true,
                expected
            ),
            MatchDecision::Auto { bill_id: 2 }
        );
    }

    #[test]
    fn merchant_name_can_match() {
        let expected = ts(2026, 8, 24);
        let bills = [bill(1, 10, expected, 1000, 1000, "TESCO", false)];
        assert_eq!(
            decide(&bills, 10, "CARD PAYMENT", Some("TESCO, HULL"), 1000, false, expected),
            MatchDecision::Auto { bill_id: 1 }
        );
    }

    #[test]
    fn date_roll_uses_advance_past() {
        let paid = ts(2026, 8, 24);
        let now = ts(2026, 8, 24);
        let next = crate::db::advance_past(paid, "monthly", now);
        let d = Utc.timestamp_opt(next, 0).single().unwrap();
        assert_eq!((d.year(), d.month(), d.day()), (2026, 9, 24));
    }

    #[test]
    fn funding_maths_transfer_in() {
        let lines = vec![
            FundingLine {
                bill_id: 1,
                name: "KCOM".into(),
                expected_date: ts(2026, 9, 24),
                expected_amount_cents: 4099,
                is_inflow: false,
            },
            FundingLine {
                bill_id: 2,
                name: "Council tax".into(),
                expected_date: ts(2026, 9, 15),
                expected_amount_cents: 150_00,
                is_inflow: false,
            },
        ];
        // balance 100, slack 200 → need 4099+15000+200-100 = 19199
        let r = funding_forecast(100, 200, lines);
        assert_eq!(r.remaining_net_out_cents, 19_099);
        assert_eq!(r.transfer_in_cents, 19_199);
    }

    #[test]
    fn funding_inflow_reduces_transfer() {
        let lines = vec![
            FundingLine {
                bill_id: 1,
                name: "Rent".into(),
                expected_date: ts(2026, 9, 10),
                expected_amount_cents: 80_000,
                is_inflow: false,
            },
            FundingLine {
                bill_id: 2,
                name: "Salary".into(),
                expected_date: ts(2026, 9, 27),
                expected_amount_cents: 250_000,
                is_inflow: true,
            },
        ];
        let r = funding_forecast(10_000, 5_000, lines);
        assert_eq!(r.remaining_net_out_cents, 80_000 - 250_000);
        assert_eq!(r.transfer_in_cents, 0);
    }

    #[test]
    fn funding_horizon_includes_overdue_excludes_next_month() {
        let end = calendar_month_end(ts(2026, 9, 4));
        assert!(in_funding_horizon(ts(2026, 8, 20), end));
        assert!(in_funding_horizon(ts(2026, 9, 30), end));
        assert!(!in_funding_horizon(ts(2026, 10, 1), end));
    }

    #[test]
    fn monthly_unrolls_into_next_month() {
        let next = ts(2026, 9, 6);
        let (y, m) = next_calendar_month(ts(2026, 9, 4));
        assert_eq!((y, m), (2026, 10));
        let dates = occurrences_in_range(next, "monthly", month_start(y, m), month_end(y, m));
        assert_eq!(dates.len(), 1);
        let d = Utc.timestamp_opt(dates[0], 0).single().unwrap();
        assert_eq!((d.year(), d.month(), d.day()), (2026, 10, 6));
    }

    #[test]
    fn remaining_this_month_skips_dates_already_in_next_month() {
        let now = ts(2026, 9, 4);
        assert_eq!(
            remaining_this_month(ts(2026, 9, 6), "monthly", now).len(),
            1
        );
        assert!(remaining_this_month(ts(2026, 10, 1), "monthly", now).is_empty());
    }

    #[test]
    fn remaining_this_month_keeps_overdue() {
        let now = ts(2026, 9, 4);
        let dates = remaining_this_month(ts(2026, 9, 2), "monthly", now);
        assert_eq!(dates, vec![ts(2026, 9, 2)]);
    }

    #[test]
    fn weekly_unrolls_multiple_in_a_month() {
        let dates = occurrences_in_range(
            ts(2026, 10, 1),
            "weekly",
            month_start(2026, 10),
            month_end(2026, 10),
        );
        assert!(dates.len() >= 4, "got {}", dates.len());
    }

    #[test]
    fn yearly_outside_target_month_is_empty() {
        let dates = occurrences_in_range(
            ts(2027, 1, 26),
            "yearly",
            month_start(2026, 10),
            month_end(2026, 10),
        );
        assert!(dates.is_empty());
    }

    #[test]
    fn cover_next_month_adds_remaining_this_and_next() {
        // Remaining Sep £87.46 + Oct net £200 + slack £100 − balance £50 → transfer £337.46
        let this_month = vec![FundingLine {
            bill_id: 1,
            name: "Giffgaff".into(),
            expected_date: ts(2026, 9, 23),
            expected_amount_cents: 8746,
            is_inflow: false,
        }];
        let next_month = vec![FundingLine {
            bill_id: 2,
            name: "Mortgage".into(),
            expected_date: ts(2026, 10, 1),
            expected_amount_cents: 20_000,
            is_inflow: false,
        }];
        let mut all = this_month;
        all.extend(next_month);
        let r = funding_forecast(5_000, 10_000, all);
        assert_eq!(r.remaining_net_out_cents, 28_746);
        assert_eq!(r.transfer_in_cents, 33_746);
    }

    #[test]
    fn unpaid_vanquis_statement_is_next_month_payment() {
        let last_pay = ts(2026, 9, 4);
        assert_eq!(
            resolve_variable_amount(
                Some(183_638),
                Some("2026-09-10"),
                Some(127_978),
                Some(last_pay)
            ),
            VariableAmount::Statement(183_638)
        );
    }

    #[test]
    fn payment_after_statement_falls_back_to_last_payment() {
        let last_pay = ts(2026, 9, 14);
        assert_eq!(
            resolve_variable_amount(
                Some(14_004),
                Some("2026-09-07"),
                Some(14_004),
                Some(last_pay)
            ),
            VariableAmount::LastPayment(14_004)
        );
    }

    #[test]
    fn no_statement_uses_last_payment() {
        assert_eq!(
            resolve_variable_amount(None, None, Some(127_978), Some(ts(2026, 9, 4))),
            VariableAmount::LastPayment(127_978)
        );
        assert_eq!(
            resolve_variable_amount(None, None, None, None),
            VariableAmount::Unknown
        );
    }

    #[test]
    fn card_name_matches_repayment_bill() {
        assert!(card_matches_bill("Vanquis", "Vanquis", Some("VANQUIS")));
        assert!(card_matches_bill(
            "Capital On Tap",
            "CAPITAL ON TAP     (DD)",
            Some("CAPITAL ON TAP")
        ));
        assert!(!card_matches_bill("Vanquis", "KCOM", Some("KCOM")));
        assert!(!card_matches_bill("ab", "ab", Some("ab")));
    }
}
