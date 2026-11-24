//! Standalone validation that all real migrations apply cleanly under libsql, mirroring the
//! runner in src/migrate.rs. Lives in tests/ (libsql-only link unit) so it sidesteps the
//! sqlx/libsql duplicate-SQLite-symbol clash while the in-binary cutover is staged.

use libsql::{params, Builder, Connection};

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "init", include_str!("../migrations/0001_init.sql")),
    (2, "full_features", include_str!("../migrations/0002_full_features.sql")),
    (3, "auth", include_str!("../migrations/0003_auth.sql")),
    (4, "ai_settings", include_str!("../migrations/0004_ai_settings.sql")),
    (5, "holdings", include_str!("../migrations/0005_holdings.sql")),
    (6, "balances_renames", include_str!("../migrations/0006_balances_renames.sql")),
    (7, "oauth", include_str!("../migrations/0007_oauth.sql")),
    (8, "drop_legacy_auth", include_str!("../migrations/0008_drop_legacy_auth.sql")),
    (9, "drop_api_tokens", include_str!("../migrations/0009_drop_api_tokens.sql")),
    (10, "reminders", include_str!("../migrations/0010_reminders.sql")),
    (11, "watchlist", include_str!("../migrations/0011_watchlist.sql")),
    (12, "drop_watchlist", include_str!("../migrations/0012_drop_watchlist.sql")),
    (13, "manual_accounts", include_str!("../migrations/0013_manual_accounts.sql")),
    (14, "financial_items", include_str!("../migrations/0014_financial_items.sql")),
    (15, "mortgage_property_index", include_str!("../migrations/0015_mortgage_property_index.sql")),
    (16, "transfer_pairs", include_str!("../migrations/0016_transfer_pairs.sql")),
    (17, "transfer_unmatchable", include_str!("../migrations/0017_transfer_unmatchable.sql")),
    (18, "transfer_await", include_str!("../migrations/0018_transfer_await.sql")),
    (19, "bill_account_funding", include_str!("../migrations/0019_bill_account_funding.sql")),
    (20, "package_b", include_str!("../migrations/0020_package_b.sql")),
    (21, "claim_flags", include_str!("../migrations/0021_claim_flags.sql")),
    (22, "package_c", include_str!("../migrations/0022_package_c.sql")),
];

async fn run(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, \
         description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);",
    )
    .await?;
    let mut applied = std::collections::HashSet::new();
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success=1", ())
        .await?;
    while let Some(r) = rows.next().await? {
        applied.insert(r.get::<i64>(0)?);
    }
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();
    for (v, name, sql) in MIGRATIONS {
        if applied.contains(v) {
            continue;
        }
        let tx = conn.transaction().await?;
        tx.execute_batch(sql).await?;
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            params![*v, *name, Vec::<u8>::new()],
        )
        .await?;
        tx.commit().await?;
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();
    Ok(())
}

async fn versions(conn: &Connection) -> Vec<i64> {
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version", ())
        .await
        .unwrap();
    let mut got = Vec::new();
    while let Some(r) = rows.next().await.unwrap() {
        got.push(r.get::<i64>(0).unwrap());
    }
    got
}

#[tokio::test]
async fn all_migrations_apply_under_libsql() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();

    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=22).collect::<Vec<_>>());

    // The real migration SQL produced real tables under libsql's dialect.
    let mut rows = conn
        .query(
            "SELECT count(*) FROM sqlite_master WHERE type='table' \
             AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
            (),
        )
        .await
        .unwrap();
    let n: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(n > 0, "migrations should have created tables");

    // Idempotent re-run.
    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=22).collect::<Vec<_>>());
}

#[tokio::test]
async fn populated_db_keeps_existing_rows_after_0014() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();

    // Apply 1..=13 only, then seed like a live database, then apply 14.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, \
         description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);",
    )
    .await
    .unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();
    for (v, name, sql) in &MIGRATIONS[..13] {
        let tx = conn.transaction().await.unwrap();
        tx.execute_batch(sql).await.unwrap();
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            params![*v, *name, Vec::<u8>::new()],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();

    conn.execute(
        "INSERT INTO consents (
            nickname, credentials_id, access_token_enc, access_token_nonce,
            refresh_token_enc, refresh_token_nonce, expires_at, scopes,
            created_at, updated_at, enabled
         ) VALUES ('seed-bank', 'creds', x'00', x'00', x'00', x'00', 1, 'accounts', 1, 1, 1)",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO accounts (
            consent_id, truelayer_id, kind, display_name, currency, enabled, created_at, updated_at
         ) VALUES (1, 'tl-seed', 'account', 'Seed current', 'GBP', 1, 1, 1)",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO transactions (
            account_id, provider_txn_id, timestamp, description, amount_cents, currency,
            is_credit, is_pending, created_at, updated_at
         ) VALUES (1, 'txn-seed', 1000, 'COFFEE', 250, 'GBP', 0, 0, 1, 1)",
        (),
    )
    .await
    .unwrap();

    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=22).collect::<Vec<_>>());

    let mut rows = conn
        .query("SELECT display_name FROM accounts WHERE truelayer_id = 'tl-seed'", ())
        .await
        .unwrap();
    let name: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(name, "Seed current");

    let mut rows = conn
        .query("SELECT count(*) FROM transactions WHERE provider_txn_id = 'txn-seed'", ())
        .await
        .unwrap();
    let n: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(n, 1);

    for table in [
        "financial_items",
        "valuation_snapshots",
        "investment_holdings",
        "property_details",
        "mortgage_details",
        "tax_liability_details",
        "receivable_details",
        "forecast_scenarios",
    ] {
        let mut rows = conn
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                params![table],
            )
            .await
            .unwrap();
        let n: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(n, 1, "{table} should exist after 0014");
    }
}

#[tokio::test]
async fn populated_0014_mortgages_survive_0015_index() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, \
         description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);",
    )
    .await
    .unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();
    for (v, name, sql) in &MIGRATIONS[..14] {
        let tx = conn.transaction().await.unwrap();
        tx.execute_batch(sql).await.unwrap();
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            params![*v, *name, Vec::<u8>::new()],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();

    conn.execute(
        "INSERT INTO financial_items (
            name, class, item_type, owner_entity, currency,
            is_asset, is_liability, is_rollup, is_active, created_at, updated_at
         ) VALUES ('IO mortgage', 'mortgage', 'interest_only', 'personal', 'GBP', 0, 1, 0, 1, 1, 1)",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO mortgage_details (
            financial_item_id, property_item_id, repayment_type, outstanding_balance
         ) VALUES (1, NULL, 'interest_only', 120000.0)",
        (),
    )
    .await
    .unwrap();

    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=22).collect::<Vec<_>>());

    let mut rows = conn
        .query(
            "SELECT property_item_id, outstanding_balance FROM mortgage_details WHERE financial_item_id = 1",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let pid: Option<i64> = row.get(0).unwrap();
    let bal: f64 = row.get(1).unwrap();
    assert_eq!(pid, None);
    assert!((bal - 120_000.0).abs() < 1e-9);

    let mut rows = conn
        .query(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_mortgage_details_property'",
            (),
        )
        .await
        .unwrap();
    let n: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(n, 1);
}
