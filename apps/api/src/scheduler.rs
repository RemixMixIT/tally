//! Background scheduler that periodically iterates enabled consents and runs the importer.
//!
//! Default schedule: four times a day at minute 7 (00:07, 06:07, 12:07, 18:07).
//! UK Open Banking caps unattended AIS at about 4 calls/day per endpoint unless
//! the user's IP is sent as `X-PSU-IP`. The old hourly cron exhausted Tide's
//! quota by mid-morning, after which transaction fetches 429'd and were stored
//! as a successful empty sync.
//! Configurable via `TALLY_SYNC_CRON` env var (5- or 6-field cron expression).

use crate::importer::Importer;
use crate::AppState;
use anyhow::Result;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

pub async fn start_scheduler(state: Arc<AppState>) -> Result<JobScheduler> {
    let sched = JobScheduler::new().await?;

    let cron_expr =
        std::env::var("TALLY_SYNC_CRON").unwrap_or_else(|_| "0 7 0,6,12,18 * * *".to_string()); // 4× daily at :07

    let state_clone = state.clone();
    let job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            run_all_consents(state).await;
        })
    })?;

    sched.add(job).await?;

    // Reminders: hourly check for due/unticked reminders + period rollover.
    let state_clone = state.clone();
    let reminders_job = Job::new_async("0 15 * * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            check_reminders(state).await;
        })
    })?;
    sched.add(reminders_job).await?;

    // Direct debits due soon: daily Telegram digest at 08:00.
    let state_clone = state.clone();
    let bills_job = Job::new_async("0 0 8 * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            notify_due_bills(state).await;
        })
    })?;
    sched.add(bills_job).await?;

    // Batty valuation CSV (rclone-copied onto /app/inbound). Cadence is twice daily
    // on Batty; poll every 15 minutes so a 06:45 / 14:20 copy is picked up quickly.
    let state_clone = state.clone();
    let feed_job = Job::new_async("0 5,20,35,50 * * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            crate::valuation_feed::run_scheduled(&state.db).await;
        })
    })?;
    sched.add(feed_job).await?;

    // Vanquis PDFs copied onto /app/inbound/vanquis by host rclone
    // (Dropbox drop folder; OneDrive archive after import).
    let state_clone = state.clone();
    let vanquis_job = Job::new_async("0 8,23,38,53 * * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            crate::vanquis::run_scheduled(&state.db).await;
        })
    })?;
    sched.add(vanquis_job).await?;

    // Capital on Tap PDFs copied onto /app/inbound/capital-on-tap by host rclone
    // (Dropbox drop folder; OneDrive archive after import).
    let state_clone = state.clone();
    let cot_job = Job::new_async("0 9,24,39,54 * * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            crate::capital_on_tap::run_scheduled(&state.db).await;
        })
    })?;
    sched.add(cot_job).await?;

    // Receipt intake stays off unless TALLY_IMMICH_RECEIPTS=1. Every 2 minutes is inside
    // the five-minute REQ-001 window. The job is not registered while the adapter is off.
    if crate::receipt::immich_receipts_enabled() {
        let state_clone = state.clone();
        let receipt_cron = std::env::var("TALLY_RECEIPT_POLL_CRON")
            .unwrap_or_else(|_| "0 */2 * * * *".to_string());
        let receipt_job = Job::new_async(receipt_cron.as_str(), move |_uuid, _l| {
            let state = state_clone.clone();
            Box::pin(async move {
                if let Err(e) = crate::receipt::poll_immich(&state.db).await {
                    error!("immich receipt poll failed: {e:#}");
                }
            })
        })?;
        sched.add(receipt_job).await?;
        info!(cron = %receipt_cron, "immich receipt poller enabled");
    } else {
        info!("immich receipt poller off (TALLY_IMMICH_RECEIPTS is not 1)");
    }

    sched.start().await?;
    info!(cron = %cron_expr, "scheduler started");

    let startup_state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        crate::valuation_feed::run_scheduled(&startup_state.db).await;
        crate::vanquis::run_scheduled(&startup_state.db).await;
        crate::capital_on_tap::run_scheduled(&startup_state.db).await;
        if crate::receipt::immich_receipts_enabled() {
            if let Err(e) = crate::receipt::poll_immich(&startup_state.db).await {
                error!("startup immich receipt poll failed: {e:#}");
            }
        }
    });
    Ok(sched)
}

/// Hourly: ping reminders that are due-and-unticked, then roll elapsed periods forward.
pub async fn check_reminders(state: Arc<AppState>) {
    let now = Utc::now().timestamp();

    match state.db.reminders_to_notify(now).await {
        Ok(rs) => {
            for r in rs {
                let text = format!("⏰ *Reminder:* {} — due {}", r.title, fmt_day(r.due_at));
                state.notifier.send_telegram_text(&text, false).await;
                let _ = state.db.mark_reminder_notified(r.id, now).await;
            }
        }
        Err(e) => error!("reminders_to_notify failed: {e:#}"),
    }

    match state.db.reminders_to_roll(now).await {
        Ok(rs) => {
            for r in rs {
                // Advance the deadline until it's in the future (handles long-dormant reminders).
                let mut due =
                    crate::recurrence::next_occurrence(&r.freq, r.every_n, r.anchor_day, r.due_at);
                while due <= now {
                    due = crate::recurrence::next_occurrence(&r.freq, r.every_n, r.anchor_day, due);
                }
                let _ = state.db.roll_reminder(r.id, due).await;
            }
        }
        Err(e) => error!("reminders_to_roll failed: {e:#}"),
    }
}

fn fmt_day(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%a %e %b").to_string())
        .unwrap_or_default()
}

/// Daily: Telegram digest of direct debits / bills due in the next few days.
pub async fn notify_due_bills(state: Arc<AppState>) {
    let days = 5;
    let bills = match state.db.list_bills_due_within(days).await {
        Ok(b) => b,
        Err(e) => {
            error!("notify_due_bills: {e:#}");
            return;
        }
    };
    if bills.is_empty() {
        return;
    }
    let mut lines = vec![format!("💸 *Direct debits due in {days} days:*")];
    for b in bills {
        let when = b.next_expected_date.map(fmt_day).unwrap_or_default();
        let amt = if b.expected_amount_max_cents > 0 {
            format!(" ~£{:.2}", b.expected_amount_max_cents as f64 / 100.0)
        } else {
            String::new()
        };
        lines.push(format!("• {}{} ({})", b.name, amt, when));
    }
    state
        .notifier
        .send_telegram_text(&lines.join("\n"), false)
        .await;
}

pub async fn run_all_consents(state: Arc<AppState>) {
    let consents = match state.db.list_enabled_consents().await {
        Ok(c) => c,
        Err(e) => {
            error!("scheduler: failed to list consents: {e:#}");
            return;
        }
    };
    info!("scheduled tick — syncing {} consent(s)", consents.len());

    let importer = Importer {
        db: state.db.clone(),
        tl: state.tl.clone(),
        notifier: state.notifier.clone(),
        ai: state.ai.clone(),
    };

    for consent in consents {
        match importer.sync_consent(&consent).await {
            Ok(r) => info!(
                consent = %consent.nickname,
                imported = r.transactions_imported,
                "consent sync ok"
            ),
            Err(e) => error!(consent = %consent.nickname, "consent sync failed: {e:#}"),
        }
    }
}
