//! Portal claim packs.
//!
//! One pack per party. RemixMix is written first, then Client-X.
//! A dual claim is a line on both packs and stores two pack ids.
//! The source amount stays on the transaction once.

use anyhow::{Context, Result};
use chrono::TimeZone;
use serde::Serialize;

use crate::claims::{self, CLIENT, REMIXMIX};
use crate::db::Db;

/// One flagged transaction or pending receipt, ready to place on a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimSource {
    pub kind: String,
    pub id: i64,
    pub amount_cents: i64,
    pub timestamp: Option<i64>,
    pub supplier: String,
    pub category: String,
    pub receipt_link: String,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackLine {
    pub source_kind: String,
    pub source_id: i64,
    pub amount_cents: i64,
    pub amount: String,
    pub date: String,
    pub supplier: String,
    pub category: String,
    pub receipt_link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimPack {
    pub id: String,
    pub party: String,
    pub created_at: i64,
    pub lines: Vec<PackLine>,
    pub total_cents: i64,
    pub total: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackAssignment {
    pub kind: String,
    pub id: i64,
    pub claim_pack_id: Option<String>,
    pub claim_pack_id_2: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSet {
    pub packs: Vec<ClaimPack>,
    pub assignments: Vec<PackAssignment>,
}

impl PackSet {
    /// Sum of source amounts, each transaction or receipt once.
    /// A dual line sits on two packs and is still counted once here.
    pub fn source_total_cents(&self) -> i64 {
        let mut seen = std::collections::BTreeSet::new();
        let mut total = 0;
        for pack in &self.packs {
            for line in &pack.lines {
                if seen.insert((line.source_kind.clone(), line.source_id)) {
                    total += line.amount_cents;
                }
            }
        }
        total
    }
}

pub fn format_gbp(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.abs();
    format!("{sign}{}.{:02}", cents / 100, cents % 100)
}

pub fn date_ymd(ts: i64) -> String {
    chrono::Utc
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Photo id as a receipt link. An existing http(s) link is kept.
pub fn receipt_link(photo_id: Option<&str>) -> String {
    match photo_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => String::new(),
        Some(id) if id.starts_with("http://") || id.starts_with("https://") => id.to_string(),
        Some(id) => format!("immich:{id}"),
    }
}

fn line_from(source: &ClaimSource) -> PackLine {
    PackLine {
        source_kind: source.kind.clone(),
        source_id: source.id,
        amount_cents: source.amount_cents,
        amount: format_gbp(source.amount_cents),
        date: source.timestamp.map(date_ymd).unwrap_or_default(),
        supplier: source.supplier.clone(),
        category: source.category.clone(),
        receipt_link: source.receipt_link.clone(),
    }
}

fn finish_pack(id: String, party: &str, created_at: i64, lines: Vec<PackLine>) -> ClaimPack {
    let total_cents = lines.iter().map(|line| line.amount_cents).sum();
    ClaimPack {
        id,
        party: party.to_string(),
        created_at,
        total: format_gbp(total_cents),
        total_cents,
        lines,
    }
}

/// RemixMix pack first, then Client-X. A single flag uses one pack id.
/// A dual flag stores the RemixMix id, then the Client-X id.
pub fn build_packs(sources: &[ClaimSource], created_at: i64) -> PackSet {
    let mut remix_lines = Vec::new();
    let mut client_lines = Vec::new();
    for source in sources {
        let remix = source.flags.iter().any(|flag| flag == REMIXMIX);
        let client = source.flags.iter().any(|flag| flag == CLIENT);
        if remix {
            remix_lines.push(line_from(source));
        }
        if client {
            client_lines.push(line_from(source));
        }
    }

    let mut packs = Vec::new();
    let remix_id = if remix_lines.is_empty() {
        None
    } else {
        let id = format!("{REMIXMIX}-{created_at}");
        packs.push(finish_pack(id.clone(), REMIXMIX, created_at, remix_lines));
        Some(id)
    };
    let client_id = if client_lines.is_empty() {
        None
    } else {
        let id = format!("{CLIENT}-{created_at}");
        packs.push(finish_pack(id.clone(), CLIENT, created_at, client_lines));
        Some(id)
    };

    let mut assignments = Vec::new();
    for source in sources {
        let remix = source.flags.iter().any(|flag| flag == REMIXMIX);
        let client = source.flags.iter().any(|flag| flag == CLIENT);
        let (claim_pack_id, claim_pack_id_2) = match (remix, client) {
            (true, true) => (remix_id.clone(), client_id.clone()),
            (true, false) => (remix_id.clone(), None),
            (false, true) => (client_id.clone(), None),
            (false, false) => continue,
        };
        assignments.push(PackAssignment {
            kind: source.kind.clone(),
            id: source.id,
            claim_pack_id,
            claim_pack_id_2,
        });
    }
    PackSet { packs, assignments }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn render_csv(pack: &ClaimPack) -> String {
    let mut out = String::from("pack_id,party,amount,date,supplier,category,receipt_link\n");
    for line in &pack.lines {
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(&pack.id),
            csv_field(&pack.party),
            csv_field(&line.amount),
            csv_field(&line.date),
            csv_field(&line.supplier),
            csv_field(&line.category),
            csv_field(&line.receipt_link),
        ));
    }
    out
}

pub fn render_markdown(pack: &ClaimPack) -> String {
    let mut out = format!("# Claim pack {}\n\nParty: {}\n\n", pack.id, pack.party);
    out.push_str("| Amount | Date | Supplier | Category | Receipt link |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for line in &pack.lines {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            line.amount.replace('|', "\\|"),
            line.date,
            line.supplier.replace('|', "\\|"),
            line.category.replace('|', "\\|"),
            line.receipt_link.replace('|', "\\|"),
        ));
    }
    out.push_str(&format!("\nPack total: {}\n", pack.total));
    out.push_str("A dual claim is also on the other party's pack. This total is this pack only.\n");
    out
}

async fn column_exists(conn: &libsql::Connection, table: &str, column: &str) -> Result<bool> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn load_sources(db: &Db) -> Result<Vec<ClaimSource>> {
    let mut sources = Vec::new();
    let mut rows = db
        .conn
        .query(
            "SELECT t.id, t.amount_cents, t.timestamp,
                    COALESCE(NULLIF(t.merchant_name, ''), NULLIF(t.counterparty_name, ''), t.description, ''),
                    COALESCE(c.name, ''),
                    t.receipt_photo_id,
                    t.claim_flags
             FROM transactions t
             LEFT JOIN categories c ON c.id = t.category_id
             WHERE t.claim_flags IS NOT NULL
               AND trim(t.claim_flags) NOT IN ('', '[]')
             ORDER BY t.timestamp, t.id",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let flags = claims::parse_stored(row.get::<Option<String>>(6)?.as_deref());
        if !flags.iter().any(|flag| flag == REMIXMIX || flag == CLIENT) {
            continue;
        }
        sources.push(ClaimSource {
            kind: "transaction".into(),
            id: row.get(0)?,
            amount_cents: row.get(1)?,
            timestamp: Some(row.get(2)?),
            supplier: row.get(3)?,
            category: row.get(4)?,
            receipt_link: receipt_link(row.get::<Option<String>>(5)?.as_deref()),
            flags,
        });
    }

    let supplier_sql =
        if column_exists(db.conn.as_ref(), "pending_receipt", "extracted_supplier").await? {
            "COALESCE(extracted_supplier, '')"
        } else {
            "''"
        };
    let sql = format!(
        "SELECT id, COALESCE(extracted_amount, 0), extracted_date, {supplier_sql},
                photo_id, claim_flags
         FROM pending_receipt
         WHERE claim_flags IS NOT NULL
           AND trim(claim_flags) NOT IN ('', '[]')
         ORDER BY id"
    );
    let mut rows = db.conn.query(&sql, ()).await?;
    while let Some(row) = rows.next().await? {
        let flags = claims::parse_stored(row.get::<Option<String>>(5)?.as_deref());
        if !flags.iter().any(|flag| flag == REMIXMIX || flag == CLIENT) {
            continue;
        }
        sources.push(ClaimSource {
            kind: "pending_receipt".into(),
            id: row.get(0)?,
            amount_cents: row.get(1)?,
            timestamp: row.get(2)?,
            supplier: row.get(3)?,
            category: String::new(),
            receipt_link: receipt_link(Some(&row.get::<String>(4)?)),
            flags,
        });
    }
    Ok(sources)
}

pub async fn save_packs(db: &Db, set: &PackSet) -> Result<()> {
    let tx = db.conn.transaction().await.context("begin claim pack tx")?;
    tx.execute("DELETE FROM claim_pack_line", ()).await?;
    tx.execute("DELETE FROM claim_pack", ()).await?;
    tx.execute(
        "UPDATE transactions SET claim_pack_id = NULL, claim_pack_id_2 = NULL",
        (),
    )
    .await?;
    tx.execute(
        "UPDATE pending_receipt SET claim_pack_id = NULL, claim_pack_id_2 = NULL",
        (),
    )
    .await?;
    for pack in &set.packs {
        tx.execute(
            "INSERT INTO claim_pack (id, party, created_at) VALUES (?1, ?2, ?3)",
            libsql::params![pack.id.clone(), pack.party.clone(), pack.created_at],
        )
        .await?;
        for line in &pack.lines {
            tx.execute(
                "INSERT INTO claim_pack_line (
                    pack_id, source_kind, source_id, amount_cents, occurred_on,
                    supplier, category, receipt_link
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                libsql::params![
                    pack.id.clone(),
                    line.source_kind.clone(),
                    line.source_id,
                    line.amount_cents,
                    line.date.clone(),
                    line.supplier.clone(),
                    line.category.clone(),
                    line.receipt_link.clone(),
                ],
            )
            .await?;
        }
    }
    for assignment in &set.assignments {
        let sql = if assignment.kind == "pending_receipt" {
            "UPDATE pending_receipt SET claim_pack_id = ?1, claim_pack_id_2 = ?2 WHERE id = ?3"
        } else {
            "UPDATE transactions SET claim_pack_id = ?1, claim_pack_id_2 = ?2 WHERE id = ?3"
        };
        tx.execute(
            sql,
            libsql::params![
                assignment.claim_pack_id.clone(),
                assignment.claim_pack_id_2.clone(),
                assignment.id,
            ],
        )
        .await?;
    }
    tx.commit().await.context("commit claim pack tx")?;
    Ok(())
}

pub async fn list_saved_packs(db: &Db) -> Result<Vec<ClaimPack>> {
    let mut packs = Vec::new();
    let mut rows = db
        .conn
        .query(
            "SELECT id, party, created_at FROM claim_pack
             ORDER BY created_at, CASE party WHEN 'RemixMix' THEN 0 ELSE 1 END, id",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let mut lines = Vec::new();
        let mut line_rows = db
            .conn
            .query(
                "SELECT source_kind, source_id, amount_cents, occurred_on, supplier, category, receipt_link
                 FROM claim_pack_line WHERE pack_id = ?1 ORDER BY id",
                libsql::params![id.clone()],
            )
            .await?;
        while let Some(line) = line_rows.next().await? {
            let amount_cents: i64 = line.get(2)?;
            lines.push(PackLine {
                source_kind: line.get(0)?,
                source_id: line.get(1)?,
                amount_cents,
                amount: format_gbp(amount_cents),
                date: line.get(3)?,
                supplier: line.get(4)?,
                category: line.get(5)?,
                receipt_link: line.get(6)?,
            });
        }
        let party: String = row.get(1)?;
        let created_at: i64 = row.get(2)?;
        packs.push(finish_pack(id, &party, created_at, lines));
    }
    Ok(packs)
}

pub async fn generate(db: &Db, created_at: i64) -> Result<PackSet> {
    let sources = load_sources(db).await?;
    let set = build_packs(&sources, created_at);
    save_packs(db, &set).await?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([7u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
    }

    async fn pack_ids(db: &Db, id: i64) -> (Option<String>, Option<String>) {
        let mut rows = db
            .conn
            .query(
                "SELECT claim_pack_id, claim_pack_id_2 FROM transactions WHERE id = ?1",
                libsql::params![id],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        (row.get(0).unwrap(), row.get(1).unwrap())
    }

    #[tokio::test]
    async fn packs_one_each_and_two_ids_for_the_dual_without_double_counting() {
        let db = test_db().await;
        let account = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let travel = db
            .create_category("Travel", None, None, None)
            .await
            .unwrap();
        let created = 1_700_000_000_i64;
        let mut ids = Vec::new();
        for (pid, desc, merchant, cents) in [
            ("b8-a", "TRAIN", "STATION", 1000_i64),
            ("b8-b", "HOTEL", "HOTEL", 2000),
            ("b8-c", "TAXI", "TAXI", 3000),
            ("b8-d", "LUNCH", "CAFE", 4000),
        ] {
            let id = db
                .upsert_transaction(
                    account.id,
                    pid,
                    created,
                    desc,
                    cents,
                    "GBP",
                    false,
                    false,
                    Some(merchant),
                    None,
                    None,
                    Some(travel),
                    None,
                    None,
                )
                .await
                .unwrap();
            ids.push(id);
        }
        for (id, photo) in [
            (ids[0], "photo-a"),
            (ids[1], "photo-b"),
            (ids[2], "photo-c"),
        ] {
            db.conn
                .execute(
                    "UPDATE transactions SET receipt_photo_id = ?1 WHERE id = ?2",
                    libsql::params![photo, id],
                )
                .await
                .unwrap();
        }

        let remix = claims::normalize_claim_flags(&[REMIXMIX.into()]).unwrap();
        let client = claims::normalize_claim_flags(&[CLIENT.into()]).unwrap();
        let both = claims::normalize_claim_flags(&[REMIXMIX.into(), CLIENT.into()]).unwrap();
        db.update_transaction_claim_flags(ids[0], remix.as_deref())
            .await
            .unwrap();
        db.update_transaction_claim_flags(ids[1], client.as_deref())
            .await
            .unwrap();
        db.update_transaction_claim_flags(ids[2], both.as_deref())
            .await
            .unwrap();

        let receipt_id = db
            .insert_pending_receipt("photo-d", Some(500), Some(created), "cash")
            .await
            .unwrap();
        db.conn
            .execute(
                "UPDATE pending_receipt SET extracted_supplier = 'KIOSK' WHERE id = ?1",
                libsql::params![receipt_id],
            )
            .await
            .unwrap();
        db.update_pending_receipt_claim_flags(receipt_id, both.as_deref())
            .await
            .unwrap();

        let before = db
            .count_transactions_filtered(Some(&[account.id]), None, None, None)
            .await
            .unwrap();
        let set = generate(&db, created).await.unwrap();
        let after = db
            .count_transactions_filtered(Some(&[account.id]), None, None, None)
            .await
            .unwrap();
        assert_eq!(before, 4);
        assert_eq!(after, 4);

        assert_eq!(set.packs.len(), 2);
        assert_eq!(set.packs[0].party, REMIXMIX);
        assert_eq!(set.packs[1].party, CLIENT);
        assert_ne!(set.packs[0].id, set.packs[1].id);
        assert!(set.packs[0].id.starts_with("RemixMix-"));
        assert!(set.packs[1].id.starts_with("Client-X-"));
        assert_eq!(set.packs[0].total_cents, 1000 + 3000 + 500);
        assert_eq!(set.packs[1].total_cents, 2000 + 3000 + 500);
        assert_eq!(set.source_total_cents(), 1000 + 2000 + 3000 + 500);
        assert_ne!(
            set.source_total_cents(),
            set.packs[0].total_cents + set.packs[1].total_cents
        );

        let mut amount_rows = db
            .conn
            .query(
                "SELECT amount_cents FROM transactions WHERE account_id = ?1 ORDER BY provider_txn_id",
                libsql::params![account.id],
            )
            .await
            .unwrap();
        let mut amounts = Vec::new();
        while let Some(row) = amount_rows.next().await.unwrap() {
            amounts.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(amounts, vec![1000, 2000, 3000, 4000]);

        assert_eq!(
            pack_ids(&db, ids[0]).await,
            (Some(set.packs[0].id.clone()), None)
        );
        assert_eq!(
            pack_ids(&db, ids[1]).await,
            (Some(set.packs[1].id.clone()), None)
        );
        assert_eq!(
            pack_ids(&db, ids[2]).await,
            (Some(set.packs[0].id.clone()), Some(set.packs[1].id.clone()))
        );
        assert_eq!(pack_ids(&db, ids[3]).await, (None, None));

        let mut receipt_rows = db
            .conn
            .query(
                "SELECT claim_pack_id, claim_pack_id_2 FROM pending_receipt WHERE id = ?1",
                libsql::params![receipt_id],
            )
            .await
            .unwrap();
        let receipt_row = receipt_rows.next().await.unwrap().unwrap();
        assert_eq!(
            receipt_row.get::<Option<String>>(0).unwrap().as_deref(),
            Some(set.packs[0].id.as_str())
        );
        assert_eq!(
            receipt_row.get::<Option<String>>(1).unwrap().as_deref(),
            Some(set.packs[1].id.as_str())
        );

        let remix_csv = render_csv(&set.packs[0]);
        assert!(remix_csv.starts_with("pack_id,party,amount,date,supplier,category,receipt_link\n"));
        assert!(remix_csv.contains(",RemixMix,"));
        assert!(remix_csv.contains("10.00"));
        assert!(remix_csv.contains("2023-11-14"));
        assert!(remix_csv.contains("STATION"));
        assert!(remix_csv.contains("Travel"));
        assert!(remix_csv.contains("immich:photo-a"));
        assert!(remix_csv.contains("30.00"));
        assert!(remix_csv.contains("KIOSK"));
        assert!(!remix_csv.contains("HOTEL"));
        assert!(!remix_csv.contains("CAFE"));

        let client_csv = render_csv(&set.packs[1]);
        assert!(client_csv.contains(",Client-X,"));
        assert!(client_csv.contains("HOTEL"));
        assert!(client_csv.contains("20.00"));
        assert!(client_csv.contains("immich:photo-b"));
        assert!(client_csv.contains("KIOSK"));
        assert!(!client_csv.contains("STATION"));

        let remix_md = render_markdown(&set.packs[0]);
        assert!(remix_md.contains("Party: RemixMix"));
        assert!(remix_md.contains("immich:photo-c"));
        let client_md = render_markdown(&set.packs[1]);
        assert!(client_md.contains("Party: Client-X"));
        assert_ne!(remix_md, client_md);

        let saved = list_saved_packs(&db).await.unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].id, set.packs[0].id);
        assert_eq!(saved[1].party, CLIENT);
    }
}
