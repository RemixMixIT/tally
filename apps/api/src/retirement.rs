//! 2030 TMS income test: trailing net P&L vs the wiki checkpoint path.
//!
//! P&L for a book in a month is equity change minus contributions plus withdrawals.
//! Blank cashflow on a snapshot is unknown, not zero.

use crate::models::{FinancialItem, ValuationSnapshot};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

const PATH: &[(&str, f64)] = &[
    ("2026-09-07", 0.0),
    ("2026-12-31", 529.0),
    ("2027-06-30", 1_588.0),
    ("2027-12-31", 2_647.0),
    ("2028-06-30", 3_706.0),
    ("2028-12-31", 4_765.0),
    ("2029-06-30", 5_824.0),
    ("2029-12-31", 6_882.0),
    ("2030-06-30", 7_941.0),
    ("2030-12-31", 9_000.0),
];

const OFF_TRACK_RATIO: f64 = 0.70;
const TRAILING_MONTHS: usize = 6;

#[derive(Debug, Clone, Serialize)]
pub struct RetirementBook {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub planning_annual_return: Option<f64>,
    pub capital_gbp: Option<f64>,
    pub valuation_date: Option<String>,
    pub this_month: MonthPnl,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthPnl {
    pub year_month: String,
    pub start_gbp: Option<f64>,
    pub end_gbp: Option<f64>,
    pub equity_change_gbp: Option<f64>,
    pub contributions_gbp: Option<f64>,
    pub withdrawals_gbp: Option<f64>,
    pub cashflow_complete: bool,
    pub pnl_gbp: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirementMonth {
    pub year_month: String,
    pub pnl_gbp: Option<f64>,
    pub cashflow_complete: bool,
    pub equity_change_gbp: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirementTrackerPoint {
    pub date: String,
    pub year_month: String,
    pub estimate_gbp: f64,
    pub off_track_below_gbp: f64,
    pub month_pnl_gbp: Option<f64>,
    pub trailing_6m_gbp: Option<f64>,
    pub is_checkpoint: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirementReport {
    pub as_of: String,
    pub tms_capital_gbp: f64,
    pub this_month_pnl_gbp: Option<f64>,
    pub this_month_cashflow_complete: bool,
    pub trailing_6m_avg_gbp: Option<f64>,
    pub path_target_gbp: f64,
    pub off_track_below_gbp: f64,
    pub on_track: Option<bool>,
    pub trailing_months_ready: usize,
    pub books: Vec<RetirementBook>,
    pub months: Vec<RetirementMonth>,
    pub tracker: Vec<RetirementTrackerPoint>,
    pub notes: Vec<String>,
}

pub fn path_target(on: NaiveDate) -> f64 {
    let points: Vec<(NaiveDate, f64)> = PATH
        .iter()
        .map(|(s, v)| (NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("path date"), *v))
        .collect();
    if on <= points[0].0 {
        return points[0].1;
    }
    let last = *points.last().expect("path");
    if on >= last.0 {
        return last.1;
    }
    for w in points.windows(2) {
        if on >= w[0].0 && on <= w[1].0 {
            let span = (w[1].0 - w[0].0).num_days() as f64;
            if span <= 0.0 {
                return w[1].1;
            }
            let gone = (on - w[0].0).num_days() as f64;
            return w[0].1 + (w[1].1 - w[0].1) * (gone / span);
        }
    }
    last.1
}

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (y, m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).expect("month") - chrono::Duration::days(1)
}

fn checkpoint_dates() -> Vec<NaiveDate> {
    PATH.iter()
        .map(|(s, _)| NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("path date"))
        .collect()
}

fn trailing_at(months: &[RetirementMonth], ym: &str) -> Option<f64> {
    let complete: Vec<f64> = months
        .iter()
        .filter(|m| m.year_month.as_str() <= ym && m.cashflow_complete)
        .filter_map(|m| m.pnl_gbp)
        .collect();
    if complete.len() >= TRAILING_MONTHS {
        let slice = &complete[complete.len() - TRAILING_MONTHS..];
        Some(slice.iter().sum::<f64>() / TRAILING_MONTHS as f64)
    } else {
        None
    }
}

fn tracker_series(as_of: NaiveDate, months: &[RetirementMonth]) -> Vec<RetirementTrackerPoint> {
    let origin = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
    let end = NaiveDate::from_ymd_opt(2030, 12, 31).unwrap();
    let checkpoints = checkpoint_dates();
    let mut dates = checkpoints.clone();
    dates.push(as_of);
    let mut year = 2026i32;
    let mut month = 9u32;
    loop {
        let d = last_day_of_month(year, month);
        if d >= origin && d <= end {
            dates.push(d);
        }
        if year == 2030 && month == 12 {
            break;
        }
        month += 1;
        if month == 13 {
            month = 1;
            year += 1;
        }
    }
    dates.sort();
    dates.dedup();

    dates
        .into_iter()
        .map(|d| {
            let ym = year_month(d);
            let estimate = path_target(d);
            let month_end = last_day_of_month(d.year(), d.month());
            let at_close = d == month_end;
            let month_row = months.iter().find(|m| m.year_month == ym);
            RetirementTrackerPoint {
                date: d.format("%Y-%m-%d").to_string(),
                year_month: ym.clone(),
                estimate_gbp: estimate,
                off_track_below_gbp: estimate * OFF_TRACK_RATIO,
                month_pnl_gbp: if at_close {
                    month_row.filter(|m| m.cashflow_complete).and_then(|m| m.pnl_gbp)
                } else {
                    None
                },
                trailing_6m_gbp: if at_close {
                    trailing_at(months, &ym)
                } else {
                    None
                },
                is_checkpoint: checkpoints.contains(&d),
            }
        })
        .collect()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn year_month(d: NaiveDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

fn book_kind(item: &FinancialItem) -> Option<(&'static str, Option<f64>)> {
    let name = item.name.to_ascii_lowercase();
    if name.contains("sipp") {
        return Some(("sipp", Some(0.13)));
    }
    if name.contains("global prime") {
        return Some(("fx", Some(0.30)));
    }
    if item.item_type.eq_ignore_ascii_case("isa") || name.contains("isa") {
        return Some(("isa", None));
    }
    None
}

fn latest<'a>(snaps: &'a [ValuationSnapshot]) -> Option<&'a ValuationSnapshot> {
    snaps.iter().max_by(|a, b| {
        a.valuation_date
            .cmp(&b.valuation_date)
            .then(a.id.cmp(&b.id))
    })
}

fn month_pnl(snaps: &[ValuationSnapshot], ym: &str) -> MonthPnl {
    let mut in_month: Vec<&ValuationSnapshot> = snaps
        .iter()
        .filter(|s| parse_date(&s.valuation_date).is_some_and(|d| year_month(d) == ym))
        .collect();
    in_month.sort_by(|a, b| a.valuation_date.cmp(&b.valuation_date).then(a.id.cmp(&b.id)));

    let first_day = NaiveDate::parse_from_str(&format!("{ym}-01"), "%Y-%m-%d").ok();
    let prior = first_day.and_then(|start| {
        snaps
            .iter()
            .filter(|s| parse_date(&s.valuation_date).is_some_and(|d| d < start))
            .max_by(|a, b| a.valuation_date.cmp(&b.valuation_date).then(a.id.cmp(&b.id)))
    });

    let end = in_month.last().copied();
    let start = prior.or_else(|| {
        let first_date = in_month.first()?.valuation_date.as_str();
        in_month
            .iter()
            .rev()
            .find(|s| s.valuation_date == first_date)
            .copied()
    });
    let start_gbp = start.map(|s| s.gbp_value);
    let end_gbp = end.map(|s| s.gbp_value);
    let equity_change_gbp = match (start_gbp, end_gbp) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };

    let cashflow_complete = end
        .is_some_and(|s| s.contributions.is_some() && s.withdrawals.is_some());
    let contributions_gbp = if cashflow_complete {
        Some(in_month.iter().filter_map(|s| s.contributions).sum())
    } else {
        None
    };
    let withdrawals_gbp = if cashflow_complete {
        Some(in_month.iter().filter_map(|s| s.withdrawals).sum())
    } else {
        None
    };
    let pnl_gbp = match (equity_change_gbp, contributions_gbp, withdrawals_gbp) {
        (Some(eq), Some(c), Some(w)) => Some(eq - c + w),
        _ => None,
    };

    MonthPnl {
        year_month: ym.to_string(),
        start_gbp,
        end_gbp,
        equity_change_gbp,
        contributions_gbp,
        withdrawals_gbp,
        cashflow_complete,
        pnl_gbp,
    }
}

fn all_months(snaps: &[ValuationSnapshot]) -> Vec<String> {
    let mut months: Vec<String> = snaps
        .iter()
        .filter_map(|s| parse_date(&s.valuation_date).map(year_month))
        .collect();
    months.sort();
    months.dedup();
    months
}

pub fn build_report(
    items: &[FinancialItem],
    snapshots: &[ValuationSnapshot],
    as_of: NaiveDate,
) -> RetirementReport {
    let this_ym = year_month(as_of);
    let mut books = Vec::new();
    let mut notes = Vec::new();
    let mut tms_capital = 0.0;

    let mut tms: Vec<(&FinancialItem, Vec<ValuationSnapshot>)> = Vec::new();
    for item in items {
        if item.is_active == 0 {
            continue;
        }
        if book_kind(item).is_none() {
            continue;
        }
        let snaps: Vec<ValuationSnapshot> = snapshots
            .iter()
            .filter(|s| s.financial_item_id == item.id)
            .cloned()
            .collect();
        tms.push((item, snaps));
    }
    tms.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    let mut month_keys: Vec<String> = Vec::new();
    for (_, snaps) in &tms {
        for m in all_months(snaps) {
            if !month_keys.contains(&m) {
                month_keys.push(m);
            }
        }
    }
    month_keys.sort();

    for (item, snaps) in &tms {
        let (kind, rate) = book_kind(item).unwrap();
        let last = latest(snaps);
        if let Some(s) = last {
            tms_capital += s.gbp_value;
        } else {
            notes.push(format!("{} has no valuation yet", item.name));
        }
        books.push(RetirementBook {
            id: item.id,
            name: item.name.clone(),
            kind: kind.to_string(),
            planning_annual_return: rate,
            capital_gbp: last.map(|s| s.gbp_value),
            valuation_date: last.map(|s| s.valuation_date.clone()),
            this_month: month_pnl(snaps, &this_ym),
        });
    }

    let months: Vec<RetirementMonth> = month_keys
        .iter()
        .map(|ym| {
            let mut counted: Vec<MonthPnl> = Vec::new();
            let mut blocked = false;
            for (_, snaps) in &tms {
                if snaps.is_empty() {
                    continue;
                }
                let p = month_pnl(snaps, ym);
                let expected = snaps.iter().any(|s| {
                    parse_date(&s.valuation_date).is_some_and(|d| year_month(d) <= *ym)
                        && s.contributions.is_some()
                        && s.withdrawals.is_some()
                });
                if p.cashflow_complete && p.pnl_gbp.is_some() {
                    counted.push(p);
                } else if expected {
                    blocked = true;
                }
            }
            let complete = !blocked && !counted.is_empty();
            let pnl = if complete {
                Some(counted.iter().filter_map(|p| p.pnl_gbp).sum())
            } else {
                None
            };
            let eq = if complete && counted.iter().all(|p| p.equity_change_gbp.is_some()) {
                Some(counted.iter().filter_map(|p| p.equity_change_gbp).sum())
            } else {
                None
            };
            RetirementMonth {
                year_month: ym.clone(),
                pnl_gbp: pnl,
                cashflow_complete: complete,
                equity_change_gbp: eq,
            }
        })
        .collect();

    let this_month = months.iter().find(|m| m.year_month == this_ym);
    let complete_pnls: Vec<f64> = months
        .iter()
        .filter(|m| m.cashflow_complete)
        .filter_map(|m| m.pnl_gbp)
        .collect();
    let trailing = if complete_pnls.len() >= TRAILING_MONTHS {
        let slice = &complete_pnls[complete_pnls.len() - TRAILING_MONTHS..];
        Some(slice.iter().sum::<f64>() / TRAILING_MONTHS as f64)
    } else {
        None
    };
    let path = path_target(as_of);
    let off_track_below = path * OFF_TRACK_RATIO;
    let on_track = trailing.map(|t| t >= off_track_below);

    if trailing.is_none() {
        notes.push(format!(
            "Trailing 6-month average needs {TRAILING_MONTHS} months with complete cashflow; {} so far.",
            complete_pnls.len()
        ));
    }
    notes.push(
        "ISA stays out of the Batty feed until funded. Do not add Tide/TSB transfers as contributions.".into(),
    );

    RetirementReport {
        as_of: as_of.format("%Y-%m-%d").to_string(),
        tms_capital_gbp: tms_capital,
        this_month_pnl_gbp: this_month.and_then(|m| m.pnl_gbp),
        this_month_cashflow_complete: this_month.map(|m| m.cashflow_complete).unwrap_or(false),
        trailing_6m_avg_gbp: trailing,
        path_target_gbp: path,
        off_track_below_gbp: off_track_below,
        on_track,
        trailing_months_ready: complete_pnls.len(),
        books,
        months: months.clone(),
        tracker: tracker_series(as_of, &months),
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(
        id: i64,
        item: i64,
        date: &str,
        gbp: f64,
        contrib: Option<f64>,
        wd: Option<f64>,
    ) -> ValuationSnapshot {
        ValuationSnapshot {
            id,
            financial_item_id: item,
            valuation_date: date.into(),
            native_value: gbp,
            currency: "GBP".into(),
            gbp_value: gbp,
            fx_rate: None,
            fx_rate_date: None,
            source_type: "other".into(),
            source_reference: Some("batty-feed".into()),
            confidence: "confirmed".into(),
            contributions: contrib,
            withdrawals: wd,
            notes: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn item(id: i64, name: &str, item_type: &str) -> FinancialItem {
        FinancialItem {
            id,
            name: name.into(),
            class: if name.contains("SIPP") {
                "pension".into()
            } else if name.contains("ISA") {
                "investment".into()
            } else {
                "trading".into()
            },
            item_type: item_type.into(),
            owner_entity: "personal".into(),
            provider: None,
            reference: None,
            parent_item_id: None,
            currency: "GBP".into(),
            is_asset: 1,
            is_liability: 0,
            is_rollup: 0,
            is_active: 1,
            notes: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn path_is_zero_in_early_september_2026() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let t = path_target(d);
        assert!(t >= 0.0 && t < 20.0, "{t}");
    }

    #[test]
    fn path_hits_nine_thousand_at_end_2030() {
        let d = NaiveDate::from_ymd_opt(2030, 12, 31).unwrap();
        assert!((path_target(d) - 9_000.0).abs() < 1e-9);
    }

    #[test]
    fn deposit_is_not_profit() {
        let snaps = vec![
            snap(1, 1, "2026-08-31", 10_000.0, Some(0.0), Some(0.0)),
            snap(2, 1, "2026-09-15", 12_083.0, Some(2_083.0), Some(0.0)),
        ];
        let m = month_pnl(&snaps, "2026-09");
        assert!(m.cashflow_complete);
        assert!((m.equity_change_gbp.unwrap() - 2_083.0).abs() < 1e-9);
        assert!((m.pnl_gbp.unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn blank_cashflow_is_unknown() {
        let snaps = vec![
            snap(1, 1, "2026-08-31", 10_000.0, Some(0.0), Some(0.0)),
            snap(2, 1, "2026-09-08", 10_100.0, None, None),
        ];
        let m = month_pnl(&snaps, "2026-09");
        assert!(!m.cashflow_complete);
        assert!(m.pnl_gbp.is_none());
        assert!((m.equity_change_gbp.unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn trailing_blank_until_six_complete_months() {
        let items = vec![item(1, "AJ Bell SIPP", "sipp")];
        let snaps = vec![
            snap(1, 1, "2026-08-31", 10_000.0, Some(0.0), Some(0.0)),
            snap(2, 1, "2026-09-30", 10_100.0, Some(0.0), Some(0.0)),
        ];
        let r = build_report(
            &items,
            &snaps,
            NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
        );
        assert!(r.trailing_6m_avg_gbp.is_none());
        assert_eq!(r.trailing_months_ready, 2);
        assert!((r.this_month_pnl_gbp.unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn unfunded_isa_does_not_block_month_pnl() {
        let items = vec![
            item(1, "AJ Bell SIPP", "sipp"),
            item(2, "AJ Bell ISA", "isa"),
        ];
        let snaps = vec![
            snap(1, 1, "2026-08-31", 10_000.0, Some(0.0), Some(0.0)),
            snap(2, 1, "2026-09-08", 10_100.0, Some(0.0), Some(0.0)),
        ];
        let r = build_report(
            &items,
            &snaps,
            NaiveDate::from_ymd_opt(2026, 9, 8).unwrap(),
        );
        assert!(r.this_month_cashflow_complete);
        assert!((r.this_month_pnl_gbp.unwrap() - 100.0).abs() < 1e-9);
        assert!(r.books.iter().any(|b| b.kind == "isa" && b.capital_gbp.is_none()));
    }

    #[test]
    fn same_day_correction_is_opening() {
        let snaps = vec![
            snap(1, 1, "2026-09-02", 50_000.0, None, None),
            snap(2, 1, "2026-09-02", 65_000.0, None, None),
            snap(3, 1, "2026-09-08", 65_100.0, Some(0.0), Some(0.0)),
        ];
        let m = month_pnl(&snaps, "2026-09");
        assert!((m.start_gbp.unwrap() - 65_000.0).abs() < 1e-9);
        assert!((m.equity_change_gbp.unwrap() - 100.0).abs() < 1e-9);
        assert!(m.cashflow_complete);
    }

    #[test]
    fn earlier_blank_days_in_month_do_not_block() {
        let snaps = vec![
            snap(1, 1, "2026-09-02", 10_000.0, None, None),
            snap(2, 1, "2026-09-08", 10_100.0, Some(0.0), Some(0.0)),
        ];
        let m = month_pnl(&snaps, "2026-09");
        assert!(m.cashflow_complete);
        assert!((m.pnl_gbp.unwrap() - 100.0).abs() < 1e-9);
        assert!((m.contributions_gbp.unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn isa_without_cashflow_does_not_block_tms() {
        let items = vec![
            item(1, "AJ Bell SIPP", "sipp"),
            item(2, "AJ Bell ISA", "isa"),
        ];
        let snaps = vec![
            snap(1, 1, "2026-09-02", 10_000.0, None, None),
            snap(2, 1, "2026-09-08", 10_100.0, Some(0.0), Some(0.0)),
            snap(3, 2, "2026-09-02", 8.0, None, None),
        ];
        let r = build_report(
            &items,
            &snaps,
            NaiveDate::from_ymd_opt(2026, 9, 8).unwrap(),
        );
        assert!(r.this_month_cashflow_complete);
        assert!((r.this_month_pnl_gbp.unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn tracker_runs_to_nine_thousand_at_end_2030() {
        let r = build_report(&[], &[], NaiveDate::from_ymd_opt(2026, 9, 8).unwrap());
        let last = r.tracker.last().unwrap();
        assert_eq!(last.date, "2030-12-31");
        assert!((last.estimate_gbp - 9_000.0).abs() < 1e-9);
        let dec26 = r.tracker.iter().find(|p| p.date == "2026-12-31").unwrap();
        assert!(dec26.is_checkpoint);
        assert!((dec26.estimate_gbp - 529.0).abs() < 1e-9);
        let start = r.tracker.iter().find(|p| p.date == "2026-09-07").unwrap();
        assert!((start.estimate_gbp - 0.0).abs() < 1e-9);
        assert!(r.tracker.iter().any(|p| p.date == "2026-09-08"));
    }

    #[test]
    fn tracker_attaches_month_pnl_and_leaves_trailing_blank() {
        let items = vec![item(1, "AJ Bell SIPP", "sipp")];
        let snaps = vec![
            snap(1, 1, "2026-08-31", 10_000.0, Some(0.0), Some(0.0)),
            snap(2, 1, "2026-09-30", 10_100.0, Some(0.0), Some(0.0)),
        ];
        let r = build_report(
            &items,
            &snaps,
            NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
        );
        let sep = r.tracker.iter().find(|p| p.date == "2026-09-30").unwrap();
        assert!((sep.month_pnl_gbp.unwrap() - 100.0).abs() < 1e-9);
        assert!(sep.trailing_6m_gbp.is_none());
        let start = r.tracker.iter().find(|p| p.date == "2026-09-07").unwrap();
        assert!(start.month_pnl_gbp.is_none());
        let dec30 = r.tracker.iter().find(|p| p.date == "2030-12-31").unwrap();
        assert!(dec30.month_pnl_gbp.is_none());
        assert!(dec30.trailing_6m_gbp.is_none());
    }
}
