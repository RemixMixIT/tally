//! Personal rolling-average expenditure: complete-month detection, filters, and grouping.
//!
//! Pure functions over in-memory rows so the API handler and unit tests share one
//! implementation. Calendar months are UTC, matching the rest of Tally's period math.

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// GBP per 1 USD from the 20 Aug 2026 Wise convert (58.87 GBP → 79.92 USD).
pub const WISE_USD_TO_GBP: f64 = 58.87 / 79.92;

const MAX_COMPLETE_MONTHS: usize = 12;

const BUSINESS_ACCOUNT_NAMES: &[&str] = &[
    "Tide Current",
    "Tide Instant Saver",
    "Capital On Tap",
];

const VANQUIS: &str = "Vanquis";
const TSB_MAIN: &str = "TSB Main Account";
const SMILE: &str = "Smile";

const GROUP_HOUSEHOLD: &str = "Household expenditure";
const GROUP_LEISURE: &str = "Leisure";
const GROUP_CAR: &str = "Car";
const GROUP_TRAVEL: &str = "Travel";
const HOUSEHOLD_BILLS: &str = "Household bills";

const LEISURE_LEAVES: &[&str] = &[
    "Entertainment",
    "Dining",
    "Camping",
    "Cash",
    "Gifts",
    "Health & Fitness",
    "Taxi",
    "Gambling",
];
const CAR_LEAVES: &[&str] = &[
    "Fuel",
    "Car Maintenance",
    "Road Tax",
    "Motoring Fines",
    "Car Lease",
    "Car wash",
    "Cars",
    "Parking",
];
const TRAVEL_LEAVES: &[&str] = &[
    "Travel",
    "Flights",
    "Ferry",
    "Accommodation",
    "Car Hire",
    "Coach",
    "Rail",
    "Holiday Money",
];

const INCOME_PARENTS: &[&str] = &["Personal Income", "RemixMix Income", "Property Income"];
const INCOME_LEAVES: &[&str] = &[
    "Salary",
    "Personal Income",
    "Other Income",
    "Child Benefit",
];

const NAMED_HOUSEHOLD_CHART: &[&str] = &[
    "Groceries",
    "Gas & Electric",
    "Childcare",
    "Household",
    "Council Tax",
];

#[derive(Debug, Clone)]
pub struct SpendRow {
    pub timestamp: i64,
    pub amount_cents: i64,
    pub currency: String,
    pub is_credit: i64,
    pub is_pending: i64,
    pub transfer_pair_id: Option<i64>,
    pub display_name: String,
    pub custom_display_name: Option<String>,
    pub category_name: Option<String>,
    pub parent_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    pub year: i32,
    pub month: u32,
}

impl YearMonth {
    pub fn new(year: i32, month: u32) -> Option<Self> {
        if (1..=12).contains(&month) {
            Some(Self { year, month })
        } else {
            None
        }
    }

    pub fn last_day(self) -> NaiveDate {
        let (ny, nm) = if self.month == 12 {
            (self.year + 1, 1)
        } else {
            (self.year, self.month + 1)
        };
        NaiveDate::from_ymd_opt(ny, nm, 1)
            .expect("valid next month")
            .pred_opt()
            .expect("valid last day")
    }

    pub fn iso(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NamedAmount {
    pub name: String,
    pub gbp_per_month: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedFx {
    pub currency: String,
    pub transaction_count: i64,
    pub native_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CountedLine {
    pub name: String,
    pub detail: String,
    pub gbp_per_month: f64,
    pub transaction_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExcludedLine {
    pub name: String,
    pub reason: String,
    pub gbp_value: Option<f64>,
    pub transaction_count: i64,
    /// True when `gbp_value` is an average over the complete-month window.
    pub per_month: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpenditureReport {
    pub months: Vec<String>,
    pub month_count: usize,
    pub is_rolling_12: bool,
    pub caption: String,
    pub total_gbp_per_month: f64,
    pub groups: Vec<NamedAmount>,
    pub categories: Vec<NamedAmount>,
    pub skipped_fx: Vec<SkippedFx>,
    pub counted: Vec<CountedLine>,
    pub excluded: Vec<ExcludedLine>,
}

#[derive(Default, Clone)]
struct Agg {
    cents: i64,
    n: i64,
}

#[derive(Clone)]
struct LeafAgg {
    group: &'static str,
    chart: String,
    cents: i64,
    n: i64,
}

struct Coverage {
    vanquis_max: Option<NaiveDate>,
    tsb_months: HashSet<YearMonth>,
    smile_months: HashSet<YearMonth>,
    earliest: Option<YearMonth>,
}

pub fn summarise(rows: &[SpendRow], as_of: NaiveDate) -> ExpenditureReport {
    let months = complete_months(rows, as_of);
    let n = months.len();
    let month_set: HashSet<YearMonth> = months.iter().copied().collect();
    let cov = coverage(rows);
    let current = YearMonth {
        year: as_of.year(),
        month: as_of.month(),
    };

    if n == 0 {
        return ExpenditureReport {
            months: vec![],
            month_count: 0,
            is_rolling_12: false,
            caption: "Personal only. No complete months yet. A month counts once it has ended, Vanquis has a transaction on or after month-end, and TSB Main Account and Smile both have activity that month.".to_string(),
            total_gbp_per_month: 0.0,
            groups: empty_groups(),
            categories: vec![],
            skipped_fx: vec![],
            counted: vec![],
            excluded: excluded_outside_window(rows, &month_set, &cov, current, as_of, 0),
        };
    }

    let mut group_cents: HashMap<&'static str, i64> = HashMap::from([
        (GROUP_HOUSEHOLD, 0),
        (GROUP_LEISURE, 0),
        (GROUP_CAR, 0),
        (GROUP_TRAVEL, 0),
    ]);
    let mut category_cents: HashMap<String, i64> = HashMap::new();
    let mut fx_skip: HashMap<String, (i64, i64)> = HashMap::new();
    let mut counted_leaves: HashMap<String, LeafAgg> = HashMap::new();
    let mut counted_accounts: HashMap<String, Agg> = HashMap::new();
    let mut excluded_in_window: HashMap<(String, String), Agg> = HashMap::new();

    for row in rows {
        let Some(day) = utc_date(row.timestamp) else {
            continue;
        };
        let Some(ym) = YearMonth::new(day.year(), day.month()) else {
            continue;
        };
        if !month_set.contains(&ym) {
            continue;
        }
        if let Some((reason, name)) = in_window_exclude(row) {
            if let Some(cur) = reason.strip_prefix("fx:") {
                let e = fx_skip.entry(cur.to_string()).or_insert((0, 0));
                e.0 += 1;
                e.1 += row.amount_cents;
            }
            let key = (name, reason);
            let signed = signed_gbp(row).unwrap_or(0);
            let e = excluded_in_window.entry(key).or_default();
            e.cents += signed;
            e.n += 1;
            continue;
        }
        let Ok(signed) = signed_gbp(row) else {
            continue;
        };
        let leaf = row.category_name.as_deref();
        let group = group_for_leaf(leaf);
        *group_cents.entry(group).or_insert(0) += signed;
        let chart = category_label(leaf);
        *category_cents.entry(chart.clone()).or_insert(0) += signed;
        let leaf_name = leaf_display(leaf);
        let e = counted_leaves.entry(leaf_name).or_insert(LeafAgg {
            group,
            chart,
            cents: 0,
            n: 0,
        });
        e.cents += signed;
        e.n += 1;
        let acct = account_label(row);
        let a = counted_accounts.entry(acct).or_default();
        a.cents += signed;
        a.n += 1;
    }

    let groups = [GROUP_HOUSEHOLD, GROUP_LEISURE, GROUP_CAR, GROUP_TRAVEL]
        .into_iter()
        .map(|name| NamedAmount {
            name: name.to_string(),
            gbp_per_month: per_month(group_cents.get(name).copied().unwrap_or(0), n),
        })
        .collect::<Vec<_>>();
    let total_cents: i64 = group_cents.values().copied().sum();
    let mut categories: Vec<NamedAmount> = category_cents
        .into_iter()
        .filter(|(_, cents)| *cents != 0)
        .map(|(name, cents)| NamedAmount {
            name,
            gbp_per_month: per_month(cents, n),
        })
        .collect();
    categories.sort_by(|a, b| {
        b.gbp_per_month
            .partial_cmp(&a.gbp_per_month)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut skipped_fx: Vec<SkippedFx> = fx_skip
        .into_iter()
        .map(|(currency, (transaction_count, native_cents))| SkippedFx {
            currency,
            transaction_count,
            native_cents,
        })
        .collect();
    skipped_fx.sort_by(|a, b| a.currency.cmp(&b.currency));

    let mut counted = Vec::new();
    for (name, a) in counted_accounts {
        if a.cents == 0 && a.n == 0 {
            continue;
        }
        counted.push(CountedLine {
            name,
            detail: format!(
                "Account · {} transaction{}",
                a.n,
                if a.n == 1 { "" } else { "s" }
            ),
            gbp_per_month: per_month(a.cents, n),
            transaction_count: a.n,
        });
    }
    for (name, a) in counted_leaves {
        if a.cents == 0 && a.n == 0 {
            continue;
        }
        let detail = if a.chart == name {
            format!(
                "{} · {} transaction{}",
                a.group,
                a.n,
                if a.n == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} · {} on the category chart · {} transaction{}",
                a.group,
                a.chart,
                a.n,
                if a.n == 1 { "" } else { "s" }
            )
        };
        counted.push(CountedLine {
            name,
            detail,
            gbp_per_month: per_month(a.cents, n),
            transaction_count: a.n,
        });
    }
    counted.sort_by(|a, b| {
        let a_acct = a.detail.starts_with("Account");
        let b_acct = b.detail.starts_with("Account");
        b_acct
            .cmp(&a_acct)
            .then_with(|| {
                b.gbp_per_month
                    .partial_cmp(&a.gbp_per_month)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut excluded: Vec<ExcludedLine> = excluded_in_window
        .into_iter()
        .map(|((name, reason), a)| {
            let reason = if let Some(cur) = reason.strip_prefix("fx:") {
                format!("Other currency ({cur}) — not converted")
            } else {
                reason
            };
            ExcludedLine {
                name,
                reason,
                gbp_value: Some(per_month(a.cents, n)),
                transaction_count: a.n,
                per_month: true,
            }
        })
        .collect();
    excluded.extend(excluded_outside_window(
        rows, &month_set, &cov, current, as_of, n,
    ));
    excluded.sort_by(|a, b| {
        b.gbp_value
            .unwrap_or(0.0)
            .abs()
            .partial_cmp(&a.gbp_value.unwrap_or(0.0).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    ExpenditureReport {
        months: months.iter().map(|m| m.iso()).collect(),
        month_count: n,
        is_rolling_12: n == MAX_COMPLETE_MONTHS,
        caption: caption(&months, n),
        total_gbp_per_month: per_month(total_cents, n),
        groups,
        categories,
        skipped_fx,
        counted,
        excluded,
    }
}

pub fn complete_months(rows: &[SpendRow], as_of: NaiveDate) -> Vec<YearMonth> {
    let cov = coverage(rows);
    let current = YearMonth {
        year: as_of.year(),
        month: as_of.month(),
    };
    let Some(start) = cov.earliest else {
        return vec![];
    };
    let mut found = Vec::new();
    let mut ym = start;
    while ym < current {
        let last = ym.last_day();
        if last >= as_of {
            break;
        }
        if month_is_complete(ym, &cov) {
            found.push(ym);
        }
        ym = next_month(ym);
    }
    if found.len() > MAX_COMPLETE_MONTHS {
        found = found[found.len() - MAX_COMPLETE_MONTHS..].to_vec();
    }
    found
}

pub fn to_gbp_cents(amount_cents: i64, currency: &str) -> Result<i64, String> {
    let cur = currency.trim().to_ascii_uppercase();
    match cur.as_str() {
        "GBP" | "" => Ok(amount_cents),
        "USD" => Ok(((amount_cents as f64) * WISE_USD_TO_GBP).round() as i64),
        other => Err(other.to_string()),
    }
}

fn signed_gbp(row: &SpendRow) -> Result<i64, String> {
    let gbp = to_gbp_cents(row.amount_cents, &row.currency)?;
    Ok(if row.is_credit != 0 { -gbp } else { gbp })
}

fn coverage(rows: &[SpendRow]) -> Coverage {
    let mut vanquis_dates: Vec<NaiveDate> = Vec::new();
    let mut tsb_months: HashSet<YearMonth> = HashSet::new();
    let mut smile_months: HashSet<YearMonth> = HashSet::new();
    let mut earliest: Option<YearMonth> = None;
    for row in rows {
        if row.is_pending != 0 {
            continue;
        }
        let Some(day) = utc_date(row.timestamp) else {
            continue;
        };
        let Some(ym) = YearMonth::new(day.year(), day.month()) else {
            continue;
        };
        earliest = Some(match earliest {
            Some(e) => e.min(ym),
            None => ym,
        });
        if labels_eq(row, VANQUIS) {
            vanquis_dates.push(day);
        }
        if labels_eq(row, TSB_MAIN) {
            tsb_months.insert(ym);
        }
        if labels_eq(row, SMILE) {
            smile_months.insert(ym);
        }
    }
    Coverage {
        vanquis_max: vanquis_dates.into_iter().max(),
        tsb_months,
        smile_months,
        earliest,
    }
}

fn month_is_complete(ym: YearMonth, cov: &Coverage) -> bool {
    let vanquis_ok = cov
        .vanquis_max
        .map(|d| d >= ym.last_day())
        .unwrap_or(false);
    vanquis_ok && cov.tsb_months.contains(&ym) && cov.smile_months.contains(&ym)
}

fn month_incomplete_reason(ym: YearMonth, cov: &Coverage, current: YearMonth, as_of: NaiveDate) -> Option<String> {
    if ym >= current || ym.last_day() >= as_of {
        return Some("Current month — not averaged".to_string());
    }
    if month_is_complete(ym, cov) {
        return None;
    }
    let vanquis_ok = cov
        .vanquis_max
        .map(|d| d >= ym.last_day())
        .unwrap_or(false);
    let mut bits = Vec::new();
    if !vanquis_ok {
        bits.push("Vanquis not through month-end");
    }
    if !cov.tsb_months.contains(&ym) {
        bits.push("no TSB Main Account activity");
    }
    if !cov.smile_months.contains(&ym) {
        bits.push("no Smile activity");
    }
    Some(format!("Incomplete month — {}", bits.join("; ")))
}

fn in_window_exclude(row: &SpendRow) -> Option<(String, String)> {
    if row.is_pending != 0 {
        return Some((
            "Pending".to_string(),
            account_label(row),
        ));
    }
    if is_business_account(row) {
        return Some(("Business account".to_string(), account_label(row)));
    }
    if row.transfer_pair_id.is_some() {
        return Some(("Paired transfer".to_string(), "Transfers".to_string()));
    }
    if let Some(reason) = category_reason(row.category_name.as_deref(), row.parent_name.as_deref())
    {
        let name = match (row.parent_name.as_deref(), row.category_name.as_deref()) {
            (Some(p), Some(l)) => format!("{p} / {l}"),
            (_, Some(l)) => l.to_string(),
            _ => "Uncategorised".to_string(),
        };
        return Some((reason, name));
    }
    if let Err(cur) = to_gbp_cents(row.amount_cents, &row.currency) {
        return Some((format!("fx:{cur}"), account_label(row)));
    }
    None
}

fn category_reason(leaf: Option<&str>, parent: Option<&str>) -> Option<String> {
    let l = leaf.unwrap_or("").trim();
    let p = parent.unwrap_or("").trim();
    if l.to_ascii_lowercase().contains("business") || p.to_ascii_lowercase().contains("business") {
        return Some("Business category".to_string());
    }
    if p.eq_ignore_ascii_case("RemixMix") {
        return Some("RemixMix".to_string());
    }
    if p.eq_ignore_ascii_case("RemixMix Income") {
        return Some("RemixMix Income".to_string());
    }
    if l.eq_ignore_ascii_case("Transfer") || p.eq_ignore_ascii_case("Transfer") {
        return Some("Transfer category".to_string());
    }
    if INCOME_PARENTS.iter().any(|n| p.eq_ignore_ascii_case(n)) {
        return Some(format!("Income ({p})"));
    }
    if INCOME_LEAVES.iter().any(|n| l.eq_ignore_ascii_case(n)) || l.to_ascii_lowercase().contains("income")
    {
        return Some("Income".to_string());
    }
    None
}

fn leaf_display(leaf: Option<&str>) -> String {
    leaf.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Uncategorised")
        .to_string()
}

fn account_label(row: &SpendRow) -> String {
    account_labels(row)
        .next()
        .unwrap_or(row.display_name.trim())
        .to_string()
}

fn would_count_personal(row: &SpendRow) -> bool {
    in_window_exclude(row).is_none()
}

fn excluded_outside_window(
    rows: &[SpendRow],
    month_set: &HashSet<YearMonth>,
    cov: &Coverage,
    current: YearMonth,
    as_of: NaiveDate,
    _n: usize,
) -> Vec<ExcludedLine> {
    let mut by_month: HashMap<YearMonth, Agg> = HashMap::new();
    for row in rows {
        let Some(day) = utc_date(row.timestamp) else {
            continue;
        };
        let Some(ym) = YearMonth::new(day.year(), day.month()) else {
            continue;
        };
        if month_set.contains(&ym) {
            continue;
        }
        if !would_count_personal(row) {
            continue;
        }
        let Ok(signed) = signed_gbp(row) else {
            continue;
        };
        let e = by_month.entry(ym).or_default();
        e.cents += signed;
        e.n += 1;
    }
    let mut months: Vec<YearMonth> = by_month.keys().copied().collect();
    months.sort();
    let mut out = Vec::new();
    let mut i = 0;
    while i < months.len() {
        let start = months[i];
        let reason = month_incomplete_reason(start, cov, current, as_of)
            .unwrap_or_else(|| "Outside the 12-month window".to_string());
        let mut end = start;
        let mut agg = by_month.get(&start).cloned().unwrap_or_default();
        let mut j = i + 1;
        while j < months.len() {
            let nxt = months[j];
            if nxt != next_month(end) {
                break;
            }
            let r = month_incomplete_reason(nxt, cov, current, as_of)
                .unwrap_or_else(|| "Outside the 12-month window".to_string());
            if r != reason {
                break;
            }
            let extra = by_month.get(&nxt).cloned().unwrap_or_default();
            agg.cents += extra.cents;
            agg.n += extra.n;
            end = nxt;
            j += 1;
        }
        let name = if start == end {
            format!("{} {}", month_abbr(start.month), start.year)
        } else {
            format!(
                "{} {}–{} {}",
                month_abbr(start.month),
                start.year,
                month_abbr(end.month),
                end.year
            )
        };
        out.push(ExcludedLine {
            name,
            reason,
            gbp_value: Some(agg.cents as f64 / 100.0),
            transaction_count: agg.n,
            per_month: false,
        });
        i = j;
    }
    out
}

fn empty_groups() -> Vec<NamedAmount> {
    [GROUP_HOUSEHOLD, GROUP_LEISURE, GROUP_CAR, GROUP_TRAVEL]
        .into_iter()
        .map(|name| NamedAmount {
            name: name.to_string(),
            gbp_per_month: 0.0,
        })
        .collect()
}

fn per_month(cents: i64, n: usize) -> f64 {
    if n == 0 {
        0.0
    } else {
        (cents as f64) / (n as f64) / 100.0
    }
}

fn utc_date(ts: i64) -> Option<NaiveDate> {
    Utc.timestamp_opt(ts, 0).single().map(|dt| dt.date_naive())
}

fn next_month(ym: YearMonth) -> YearMonth {
    if ym.month == 12 {
        YearMonth {
            year: ym.year + 1,
            month: 1,
        }
    } else {
        YearMonth {
            year: ym.year,
            month: ym.month + 1,
        }
    }
}

fn account_labels(row: &SpendRow) -> impl Iterator<Item = &str> {
    row.custom_display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .into_iter()
        .chain(std::iter::once(row.display_name.trim()).filter(|s| !s.is_empty()))
}

fn labels_eq(row: &SpendRow, want: &str) -> bool {
    account_labels(row).any(|n| n.eq_ignore_ascii_case(want))
}

fn is_business_account(row: &SpendRow) -> bool {
    BUSINESS_ACCOUNT_NAMES.iter().any(|listed| {
        let listed_l = listed.to_ascii_lowercase();
        account_labels(row).any(|n| {
            let n = n.to_ascii_lowercase();
            n == listed_l || n.starts_with(&format!("{listed_l} "))
        })
    })
}

fn eq_any(name: &str, list: &[&str]) -> bool {
    list.iter().any(|n| name.eq_ignore_ascii_case(n))
}

fn group_for_leaf(leaf: Option<&str>) -> &'static str {
    let Some(name) = leaf.map(str::trim).filter(|s| !s.is_empty()) else {
        return GROUP_HOUSEHOLD;
    };
    if eq_any(name, LEISURE_LEAVES) {
        GROUP_LEISURE
    } else if eq_any(name, CAR_LEAVES) {
        GROUP_CAR
    } else if eq_any(name, TRAVEL_LEAVES) {
        GROUP_TRAVEL
    } else {
        GROUP_HOUSEHOLD
    }
}

fn category_label(leaf: Option<&str>) -> String {
    let Some(name) = leaf.map(str::trim).filter(|s| !s.is_empty()) else {
        return HOUSEHOLD_BILLS.to_string();
    };
    if name.eq_ignore_ascii_case("Loan Interest") {
        return "Mortgage".to_string();
    }
    if name.eq_ignore_ascii_case("Car Insurance") || name.eq_ignore_ascii_case("Home Insurance") {
        return "Insurance".to_string();
    }
    if let Some(canon) = canonical_named(name, LEISURE_LEAVES)
        .or_else(|| canonical_named(name, CAR_LEAVES))
        .or_else(|| canonical_named(name, TRAVEL_LEAVES))
        .or_else(|| canonical_named(name, NAMED_HOUSEHOLD_CHART))
    {
        return canon.to_string();
    }
    HOUSEHOLD_BILLS.to_string()
}

fn canonical_named(name: &str, list: &[&'static str]) -> Option<&'static str> {
    list.iter().copied().find(|n| name.eq_ignore_ascii_case(n))
}

fn caption(months: &[YearMonth], n: usize) -> String {
    let listed = format_months(months);
    if n == MAX_COMPLETE_MONTHS {
        format!("Personal only. {listed} (N={n}). This is a 12-month rolling average.")
    } else {
        format!("Personal only. {listed} (N={n}). Becomes a 12-month rolling average once 12 complete months exist.")
    }
}

fn month_abbr(month: u32) -> &'static str {
    match month {
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
        12 => "Dec",
        _ => "???",
    }
}

fn format_months(months: &[YearMonth]) -> String {
    if months.is_empty() {
        return String::new();
    }
    let same_year = months.iter().all(|m| m.year == months[0].year);
    if same_year {
        let year = months[0].year;
        let names: Vec<&str> = months.iter().map(|m| month_abbr(m.month)).collect();
        format!("{} {year}", join_and(&names))
    } else {
        let parts: Vec<String> = months
            .iter()
            .map(|m| format!("{} {}", month_abbr(m.month), m.year))
            .collect();
        let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        join_and(&refs)
    }
}

fn join_and(parts: &[&str]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        2 => format!("{} and {}", parts[0], parts[1]),
        n => format!("{} and {}", parts[..n - 1].join(", "), parts[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp()
    }

    fn row(
        day: (i32, u32, u32),
        amount: i64,
        currency: &str,
        credit: bool,
        pending: bool,
        pair: Option<i64>,
        display: &str,
        custom: Option<&str>,
        cat: Option<&str>,
        parent: Option<&str>,
    ) -> SpendRow {
        SpendRow {
            timestamp: ts(day.0, day.1, day.2),
            amount_cents: amount,
            currency: currency.into(),
            is_credit: if credit { 1 } else { 0 },
            is_pending: if pending { 1 } else { 0 },
            transfer_pair_id: pair,
            display_name: display.into(),
            custom_display_name: custom.map(|s| s.into()),
            category_name: cat.map(|s| s.into()),
            parent_name: parent.map(|s| s.into()),
        }
    }

    /// Completeness anchors: Vanquis on last day (or later), plus TSB + Smile in-month.
    fn complete_anchors(y: i32, m: u32) -> Vec<SpendRow> {
        let last = YearMonth::new(y, m).unwrap().last_day();
        vec![
            row(
                (last.year(), last.month(), last.day()),
                1,
                "GBP",
                false,
                false,
                None,
                "Vanquis",
                None,
                Some("Fuel"),
                Some("Cars"),
            ),
            row(
                (y, m, 5),
                1,
                "GBP",
                false,
                false,
                None,
                "Classic Plus Account",
                Some("TSB Main Account"),
                Some("Loan Interest"),
                Some("Borrowing"),
            ),
            row(
                (y, m, 6),
                1,
                "GBP",
                false,
                false,
                None,
                "Smile",
                None,
                Some("Council Tax"),
                Some("Bills"),
            ),
        ]
    }

    fn as_of_sep() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 6).unwrap()
    }

    #[test]
    fn wise_usd_rate_matches_20_aug_convert() {
        assert_eq!(WISE_USD_TO_GBP, 58.87 / 79.92);
        assert!((WISE_USD_TO_GBP - 0.7366).abs() < 0.0001);
        assert_eq!(to_gbp_cents(10_000, "USD").unwrap(), 7366);
        assert_eq!(to_gbp_cents(2500, "GBP").unwrap(), 2500);
        assert_eq!(to_gbp_cents(1, "eur").unwrap_err(), "EUR");
    }

    #[test]
    fn complete_months_require_ended_vanquis_through_and_household_accounts() {
        let mut rows = Vec::new();
        // Vanquis-only history in 2025 must not count.
        rows.push(row(
            (2025, 12, 31),
            50_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Travel"),
            Some("Travel"),
        ));
        rows.extend(complete_anchors(2026, 5));
        rows.extend(complete_anchors(2026, 6));
        rows.extend(complete_anchors(2026, 7));
        // August: TSB + Smile present, Vanquis only through 8 Aug.
        rows.push(row(
            (2026, 8, 8),
            10_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Fuel"),
            Some("Cars"),
        ));
        rows.push(row(
            (2026, 8, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Groceries"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 8, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Smile",
            None,
            Some("Water"),
            Some("Bills"),
        ));
        // Current month has household activity but has not ended — and no Vanquis
        // after 8 Aug, so August stays incomplete (same as live data).
        rows.push(row(
            (2026, 9, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Groceries"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 9, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Smile",
            None,
            Some("Water"),
            Some("Bills"),
        ));

        let months = complete_months(&rows, as_of_sep());
        assert_eq!(
            months
                .iter()
                .map(|m| m.iso())
                .collect::<Vec<_>>(),
            vec!["2026-05", "2026-06", "2026-07"]
        );
    }

    #[test]
    fn vanquis_txn_after_month_end_completes_prior_month() {
        let mut rows = complete_anchors(2026, 6);
        // No Vanquis on 31 May, but June 1 is later than May 31.
        rows.push(row(
            (2026, 5, 10),
            1,
            "GBP",
            false,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Groceries"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 5, 10),
            1,
            "GBP",
            false,
            false,
            None,
            "Smile",
            None,
            Some("Water"),
            Some("Bills"),
        ));
        let months = complete_months(&rows, as_of_sep());
        assert!(months.iter().any(|m| m.iso() == "2026-05"));
        assert!(months.iter().any(|m| m.iso() == "2026-06"));
    }

    #[test]
    fn rolling_window_keeps_most_recent_12() {
        let mut rows = Vec::new();
        for i in 0..13 {
            let d = NaiveDate::from_ymd_opt(2025, 5, 1).unwrap()
                + chrono::Months::new(i);
            rows.extend(complete_anchors(d.year(), d.month()));
        }
        let as_of = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let months = complete_months(&rows, as_of);
        assert_eq!(months.len(), 12);
        assert_eq!(months[0].iso(), "2025-06");
        assert_eq!(months.last().unwrap().iso(), "2026-05");
    }

    #[test]
    fn pending_transfers_business_and_income_are_excluded() {
        let mut rows = complete_anchors(2026, 6);
        rows.push(row(
            (2026, 6, 10),
            80_00,
            "GBP",
            false,
            true,
            None,
            "Vanquis",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 11),
            90_00,
            "GBP",
            false,
            false,
            Some(9),
            "Vanquis",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 12),
            70_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 12),
            40_00,
            "GBP",
            false,
            false,
            None,
            "Tide Current Account",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 12),
            40_00,
            "GBP",
            false,
            false,
            None,
            "Capital On Tap",
            None,
            Some("Fuel"),
            Some("Cars"),
        ));
        rows.push(row(
            (2026, 6, 12),
            40_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Business: Software / SAAS"),
            Some("RemixMix"),
        ));
        rows.push(row(
            (2026, 6, 12),
            40_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Card Payment"),
            Some("Transfer"),
        ));
        rows.push(row(
            (2026, 6, 12),
            200_00,
            "GBP",
            true,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Salary"),
            Some("Personal Income"),
        ));
        rows.push(row(
            (2026, 6, 12),
            15_00,
            "GBP",
            true,
            false,
            None,
            "Vanquis",
            None,
            Some("Dining"),
            Some("Personal"),
        ));

        let r = summarise(&rows, as_of_sep());
        assert_eq!(r.month_count, 1);
        // Anchors are £0.01 each; dining 70.00 debit minus 15.00 refund = 55.00 plus tiny anchors.
        let leisure = r.groups.iter().find(|g| g.name == GROUP_LEISURE).unwrap();
        assert!((leisure.gbp_per_month - 55.0).abs() < 0.02);
        assert!(r.total_gbp_per_month < 56.0);
        assert!(r.skipped_fx.is_empty());
    }

    #[test]
    fn grouping_maps_leaves_and_category_chart_matches_group_total() {
        let mut rows = complete_anchors(2026, 6);
        // Overwrite tiny anchors' categories with real spend.
        rows.push(row(
            (2026, 6, 15),
            100_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Loan Interest"),
            Some("Borrowing"),
        ));
        rows.push(row(
            (2026, 6, 15),
            20_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Car Insurance"),
            Some("Cars"),
        ));
        rows.push(row(
            (2026, 6, 15),
            10_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Home Insurance"),
            Some("Bills"),
        ));
        rows.push(row(
            (2026, 6, 15),
            5_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Water"),
            Some("Bills"),
        ));
        rows.push(row(
            (2026, 6, 15),
            8_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Camping"),
            Some("Travel"),
        ));
        rows.push(row(
            (2026, 6, 15),
            4_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Parking"),
            Some("Travel"),
        ));
        rows.push(row(
            (2026, 6, 15),
            7_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Car Wash"),
            Some("Cars"),
        ));
        rows.push(row(
            (2026, 6, 15),
            12_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Flights"),
            Some("Travel"),
        ));
        rows.push(row(
            (2026, 6, 15),
            3_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            None,
            None,
        ));

        let r = summarise(&rows, as_of_sep());
        let by_g: HashMap<_, _> = r.groups.iter().map(|g| (g.name.as_str(), g.gbp_per_month)).collect();
        assert!(by_g[GROUP_LEISURE] > 7.9 && by_g[GROUP_LEISURE] < 8.2);
        assert!(by_g[GROUP_TRAVEL] > 11.9 && by_g[GROUP_TRAVEL] < 12.2);
        // Parking + Car wash in Car; car insurance is Household.
        assert!(by_g[GROUP_CAR] > 10.9 && by_g[GROUP_CAR] < 11.2);

        let by_c: HashMap<_, _> = r.categories.iter().map(|c| (c.name.as_str(), c.gbp_per_month)).collect();
        assert!((by_c["Mortgage"] - 100.01).abs() < 0.02 || (by_c["Mortgage"] - 100.00).abs() < 0.02);
        assert!((by_c["Insurance"] - 30.0).abs() < 0.02);
        assert_eq!(by_c.get("Camping").copied().unwrap_or(0.0), 8.0);
        assert_eq!(by_c.get("Parking").copied().unwrap_or(0.0), 4.0);
        assert_eq!(by_c.get("Car wash").copied().unwrap_or(0.0), 7.0);
        assert_eq!(by_c.get("Flights").copied().unwrap_or(0.0), 12.0);
        assert!(by_c[HOUSEHOLD_BILLS] >= 5.0);

        let group_sum: f64 = r.groups.iter().map(|g| g.gbp_per_month).sum();
        let cat_sum: f64 = r.categories.iter().map(|c| c.gbp_per_month).sum();
        assert!((group_sum - cat_sum).abs() < 1e-9);
        assert!((group_sum - r.total_gbp_per_month).abs() < 1e-9);
        assert!(r.caption.contains("Jun 2026"));
        assert!(r.caption.contains("N=1"));
        assert!(r.caption.contains("12-month rolling"));
        assert!(!r.is_rolling_12);
    }

    #[test]
    fn usd_converts_and_other_fx_is_reported() {
        let mut rows = complete_anchors(2026, 6);
        rows.push(row(
            (2026, 6, 20),
            10_000,
            "USD",
            false,
            false,
            None,
            "Personal USD",
            Some("Wise USD"),
            Some("Entertainment"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 20),
            5_000,
            "JPY",
            false,
            false,
            None,
            "Personal JPY",
            Some("Wise JPY"),
            Some("Dining"),
            Some("Personal"),
        ));
        let r = summarise(&rows, as_of_sep());
        let leisure = r.groups.iter().find(|g| g.name == GROUP_LEISURE).unwrap();
        assert!((leisure.gbp_per_month - 73.67).abs() < 0.02);
        assert_eq!(r.skipped_fx.len(), 1);
        assert_eq!(r.skipped_fx[0].currency, "JPY");
        assert_eq!(r.skipped_fx[0].native_cents, 5_000);
    }

    #[test]
    fn tide_current_matches_tide_current_account() {
        let r = SpendRow {
            timestamp: 0,
            amount_cents: 1,
            currency: "GBP".into(),
            is_credit: 0,
            is_pending: 0,
            transfer_pair_id: None,
            display_name: "Tide Current Account".into(),
            custom_display_name: None,
            category_name: None,
            parent_name: None,
        };
        assert!(is_business_account(&r));
        let saver = SpendRow {
            display_name: "Tide Instant Saver".into(),
            ..r.clone()
        };
        assert!(is_business_account(&saver));
        let smile = SpendRow {
            display_name: "Smile".into(),
            ..r
        };
        assert!(!is_business_account(&smile));
    }

    #[test]
    fn caption_lists_three_months_and_rolling_copy() {
        let months = vec![
            YearMonth::new(2026, 5).unwrap(),
            YearMonth::new(2026, 6).unwrap(),
            YearMonth::new(2026, 7).unwrap(),
        ];
        let c = caption(&months, 3);
        assert!(c.contains("May, Jun and Jul 2026"));
        assert!(c.contains("N=3"));
        assert!(c.contains("Becomes a 12-month rolling average"));
    }

    #[test]
    fn counted_and_excluded_lists_support_sense_check() {
        let mut rows = complete_anchors(2026, 6);
        rows.push(row(
            (2026, 6, 12),
            70_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 12),
            40_00,
            "GBP",
            false,
            false,
            None,
            "Tide Current Account",
            None,
            Some("Dining"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 6, 12),
            200_00,
            "GBP",
            true,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Salary"),
            Some("Personal Income"),
        ));
        rows.push(row(
            (2026, 8, 8),
            10_00,
            "GBP",
            false,
            false,
            None,
            "Vanquis",
            None,
            Some("Fuel"),
            Some("Cars"),
        ));
        rows.push(row(
            (2026, 8, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Classic Plus Account",
            Some("TSB Main Account"),
            Some("Groceries"),
            Some("Personal"),
        ));
        rows.push(row(
            (2026, 8, 5),
            1,
            "GBP",
            false,
            false,
            None,
            "Smile",
            None,
            Some("Water"),
            Some("Bills"),
        ));

        let r = summarise(&rows, as_of_sep());
        assert!(r.counted.iter().any(|c| c.name == "Dining" && c.detail.contains("Leisure")));
        assert!(r.counted.iter().any(|c| c.name == "Vanquis" && c.detail.starts_with("Account")));
        assert!(!r.counted.iter().any(|c| c.name.contains("Tide")));
        let tide = r
            .excluded
            .iter()
            .find(|e| e.name == "Tide Current Account")
            .unwrap();
        assert_eq!(tide.reason, "Business account");
        assert!(tide.per_month);
        assert!(r
            .excluded
            .iter()
            .any(|e| e.reason.contains("Income") && e.name.contains("Salary")));
        let aug = r
            .excluded
            .iter()
            .find(|e| e.name.contains("Aug"))
            .unwrap();
        assert!(aug.reason.contains("Vanquis not through"));
        assert!(!aug.per_month);
        let counted_leaves: f64 = r
            .counted
            .iter()
            .filter(|c| !c.detail.starts_with("Account"))
            .map(|c| c.gbp_per_month)
            .sum();
        assert!((counted_leaves - r.total_gbp_per_month).abs() < 1e-9);
    }
}
