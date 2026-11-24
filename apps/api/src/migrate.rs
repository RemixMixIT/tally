//! Schema migrations for the libsql backend, replacing `sqlx::migrate!`.
//!
//! Version-presence only: a migration is applied iff its version is absent from the
//! `_sqlx_migrations` ledger with `success = 1`. Checksums are intentionally NEVER compared —
//! that structurally eliminates the historical "migration N was previously applied but has
//! been modified" crash class. The ledger keeps a schema compatible with sqlx's, so a database
//! already migrated by the old sqlx path (e.g. the production Pi) reports every
//! version applied and re-runs nothing.

use anyhow::{Context, Result};
use libsql::Connection;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// All migrations, embedded in the binary so the container needs no migrations directory.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "full_features",
        sql: include_str!("../migrations/0002_full_features.sql"),
    },
    Migration {
        version: 3,
        name: "auth",
        sql: include_str!("../migrations/0003_auth.sql"),
    },
    Migration {
        version: 4,
        name: "ai_settings",
        sql: include_str!("../migrations/0004_ai_settings.sql"),
    },
    Migration {
        version: 5,
        name: "holdings",
        sql: include_str!("../migrations/0005_holdings.sql"),
    },
    Migration {
        version: 6,
        name: "balances_renames",
        sql: include_str!("../migrations/0006_balances_renames.sql"),
    },
    Migration {
        version: 7,
        name: "oauth",
        sql: include_str!("../migrations/0007_oauth.sql"),
    },
    Migration {
        version: 8,
        name: "drop_legacy_auth",
        sql: include_str!("../migrations/0008_drop_legacy_auth.sql"),
    },
    Migration {
        version: 9,
        name: "drop_api_tokens",
        sql: include_str!("../migrations/0009_drop_api_tokens.sql"),
    },
    Migration {
        version: 10,
        name: "reminders",
        sql: include_str!("../migrations/0010_reminders.sql"),
    },
    Migration {
        version: 11,
        name: "watchlist",
        sql: include_str!("../migrations/0011_watchlist.sql"),
    },
    Migration {
        version: 12,
        name: "drop_watchlist",
        sql: include_str!("../migrations/0012_drop_watchlist.sql"),
    },
    Migration {
        version: 13,
        name: "manual_accounts",
        sql: include_str!("../migrations/0013_manual_accounts.sql"),
    },
    Migration {
        version: 14,
        name: "financial_items",
        sql: include_str!("../migrations/0014_financial_items.sql"),
    },
    Migration {
        version: 15,
        name: "mortgage_property_index",
        sql: include_str!("../migrations/0015_mortgage_property_index.sql"),
    },
    Migration {
        version: 16,
        name: "transfer_pairs",
        sql: include_str!("../migrations/0016_transfer_pairs.sql"),
    },
    Migration {
        version: 17,
        name: "transfer_unmatchable",
        sql: include_str!("../migrations/0017_transfer_unmatchable.sql"),
    },
    Migration {
        version: 18,
        name: "transfer_await",
        sql: include_str!("../migrations/0018_transfer_await.sql"),
    },
    Migration {
        version: 19,
        name: "bill_account_funding",
        sql: include_str!("../migrations/0019_bill_account_funding.sql"),
    },
    Migration {
        version: 20,
        name: "package_b",
        sql: include_str!("../migrations/0020_package_b.sql"),
    },
    Migration {
        version: 21,
        name: "claim_flags",
        sql: include_str!("../migrations/0021_claim_flags.sql"),
    },
    Migration {
        version: 22,
        name: "package_c",
        sql: include_str!("../migrations/0022_package_c.sql"),
    },
    Migration {
        version: 23,
        name: "claim_packs",
        sql: include_str!("../migrations/0023_claim_packs.sql"),
    },
];

#[cfg(test)]
const PACKAGE_B_DOWN_SQL: &str = include_str!("../migrations/down/0020_package_b.sql");
#[cfg(test)]
const CLAIM_FLAGS_DOWN_SQL: &str = include_str!("../migrations/down/0021_claim_flags.sql");
#[cfg(test)]
const PACKAGE_C_DOWN_SQL: &str = include_str!("../migrations/down/0022_package_c.sql");
#[cfg(test)]
const CLAIM_PACKS_DOWN_SQL: &str = include_str!("../migrations/down/0023_claim_packs.sql");

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);";

/// Apply every embedded migration whose version is not yet recorded as successful.
pub async fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(LEDGER_DDL)
        .await
        .context("create _sqlx_migrations ledger")?;

    let mut applied = std::collections::HashSet::new();
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success = 1", ())
        .await
        .context("read migration ledger")?;
    while let Some(row) = rows.next().await? {
        applied.insert(row.get::<i64>(0)?);
    }

    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();
    if pending.is_empty() {
        tracing::info!("migrations: up to date ({} applied)", applied.len());
        return Ok(());
    }

    // PRAGMA foreign_keys is a no-op inside a transaction, so toggle it here in autocommit.
    // Migrations that rebuild tables can transiently violate FKs; the app connection runs FK-on.
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();

    for m in pending {
        tracing::info!("migrations: applying {} ({})", m.version, m.name);
        let tx = conn.transaction().await.context("begin migration tx")?;
        tx.execute_batch(m.sql)
            .await
            .with_context(|| format!("migration {} ({}) failed", m.version, m.name))?;
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            libsql::params![m.version, m.name, Vec::<u8>::new()],
        )
        .await
        .context("record migration in ledger")?;
        tx.commit().await.context("commit migration tx")?;
    }

    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsql::Builder;

    async fn mem() -> Connection {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        db.connect().unwrap()
    }

    async fn applied_versions(conn: &Connection) -> Vec<i64> {
        let mut rows = conn
            .query(
                "SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version",
                (),
            )
            .await
            .unwrap();
        let mut v = Vec::new();
        while let Some(r) = rows.next().await.unwrap() {
            v.push(r.get::<i64>(0).unwrap());
        }
        v
    }

    async fn table_exists(conn: &Connection, name: &str) -> bool {
        let mut rows = conn
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                libsql::params![name],
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap() == 1
    }

    async fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        let mut rows = conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let name: String = row.get(1).unwrap();
            if name == column {
                return true;
            }
        }
        false
    }

    async fn app_table_count(conn: &Connection) -> i64 {
        let mut rows = conn
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type='table' \
                 AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
                (),
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    #[tokio::test]
    async fn fresh_db_applies_all_migrations() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        assert_eq!(applied_versions(&conn).await, (1..=23).collect::<Vec<_>>());
        // The real migration SQL produced real tables under libsql (dialect smoke check).
        assert!(app_table_count(&conn).await > 0);
        assert!(table_exists(&conn, "pending_receipt").await);
        assert!(table_exists(&conn, "director_classification").await);
        assert!(table_exists(&conn, "receipt_asset_seen").await);
        assert!(table_exists(&conn, "claim_pack").await);
        assert!(table_exists(&conn, "claim_pack_line").await);
        for col in [
            "receipt_photo_id",
            "claim_flags",
            "claim_pack_id",
            "claim_pack_id_2",
        ] {
            assert!(column_exists(&conn, "transactions", col).await, "{col}");
        }
        assert!(column_exists(&conn, "pending_receipt", "claim_flags").await);
        assert!(column_exists(&conn, "pending_receipt", "extracted_supplier").await);
        assert!(column_exists(&conn, "pending_receipt", "flagged_for_review").await);
        assert!(column_exists(&conn, "pending_receipt", "claim_pack_id").await);
        assert!(column_exists(&conn, "pending_receipt", "claim_pack_id_2").await);
    }

    #[tokio::test]
    async fn rerun_is_idempotent() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        run(&conn).await.unwrap(); // second run must be a no-op, not an error
        assert_eq!(applied_versions(&conn).await, (1..=23).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn package_b_down_script_removes_only_package_b_objects() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        let before = app_table_count(&conn).await;
        // 0023, 0022, then 0021 sit on pending_receipt, so their downs run before Package B.
        conn.execute_batch(CLAIM_PACKS_DOWN_SQL).await.unwrap();
        conn.execute_batch(PACKAGE_C_DOWN_SQL).await.unwrap();
        conn.execute_batch(CLAIM_FLAGS_DOWN_SQL).await.unwrap();
        conn.execute_batch(PACKAGE_B_DOWN_SQL).await.unwrap();
        assert!(!table_exists(&conn, "pending_receipt").await);
        assert!(!table_exists(&conn, "director_classification").await);
        for col in [
            "receipt_photo_id",
            "claim_flags",
            "claim_pack_id",
            "claim_pack_id_2",
        ] {
            assert!(!column_exists(&conn, "transactions", col).await, "{col}");
        }
        assert!(table_exists(&conn, "transactions").await);
        assert!(table_exists(&conn, "accounts").await);
        assert!(!table_exists(&conn, "receipt_asset_seen").await);
        assert!(!table_exists(&conn, "claim_pack").await);
        assert!(!table_exists(&conn, "claim_pack_line").await);
        assert_eq!(app_table_count(&conn).await, before - 5);
        assert_eq!(applied_versions(&conn).await, (1..=19).collect::<Vec<_>>());
        run(&conn).await.unwrap();
        assert!(table_exists(&conn, "director_classification").await);
        assert!(column_exists(&conn, "pending_receipt", "claim_flags").await);
        assert!(table_exists(&conn, "receipt_asset_seen").await);
        assert!(table_exists(&conn, "claim_pack").await);
        assert_eq!(applied_versions(&conn).await, (1..=23).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn claim_flags_down_removes_only_the_pending_column() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        // 0023 and 0022 also alter pending_receipt; take them off first so version 21 can re-apply alone.
        conn.execute_batch(CLAIM_PACKS_DOWN_SQL).await.unwrap();
        conn.execute_batch(PACKAGE_C_DOWN_SQL).await.unwrap();
        conn.execute_batch(CLAIM_FLAGS_DOWN_SQL).await.unwrap();
        assert!(!column_exists(&conn, "pending_receipt", "claim_flags").await);
        assert!(column_exists(&conn, "transactions", "claim_flags").await);
        assert!(table_exists(&conn, "pending_receipt").await);
        assert!(table_exists(&conn, "director_classification").await);
        assert_eq!(applied_versions(&conn).await, (1..=20).collect::<Vec<_>>());
        run(&conn).await.unwrap();
        assert!(column_exists(&conn, "pending_receipt", "claim_flags").await);
        assert_eq!(applied_versions(&conn).await, (1..=23).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn package_c_down_removes_seen_set_and_keeps_pending_receipt() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        conn.execute_batch(CLAIM_PACKS_DOWN_SQL).await.unwrap();
        conn.execute_batch(PACKAGE_C_DOWN_SQL).await.unwrap();
        assert!(!table_exists(&conn, "receipt_asset_seen").await);
        assert!(!column_exists(&conn, "pending_receipt", "extracted_supplier").await);
        assert!(!column_exists(&conn, "pending_receipt", "flagged_for_review").await);
        assert!(table_exists(&conn, "pending_receipt").await);
        assert!(column_exists(&conn, "pending_receipt", "claim_flags").await);
        assert_eq!(applied_versions(&conn).await, (1..=21).collect::<Vec<_>>());
        run(&conn).await.unwrap();
        assert!(table_exists(&conn, "receipt_asset_seen").await);
        assert!(column_exists(&conn, "pending_receipt", "extracted_supplier").await);
        assert!(table_exists(&conn, "claim_pack").await);
        assert_eq!(applied_versions(&conn).await, (1..=23).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn preseeded_ledger_runs_nothing() {
        // Simulate a DB whose ledger already records every known version as applied (with a
        // non-empty checksum that must never be read), but the tables do NOT exist. A
        // version-presence runner must skip all of them; a checksum-reading runner would crash,
        // and a re-running one would build tables. We assert no app table was created.
        let conn = mem().await;
        conn.execute_batch(LEDGER_DDL).await.unwrap();
        for v in 1..=23i64 {
            conn.execute(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (?1, 'x', 1, ?2, 0)",
                libsql::params![v, vec![0xde_u8, 0xad]],
            )
            .await
            .unwrap();
        }
        run(&conn).await.unwrap();
        assert_eq!(
            app_table_count(&conn).await,
            0,
            "no migration should have run"
        );
    }
}
