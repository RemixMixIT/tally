//! Claim flags on a transaction or a pending receipt.
//!
//! A row stores a JSON array. The two flags are `RemixMix` and `Client-X`.
//! One row may carry both (a dual claim). Pack ids are written later, in B8.

use serde_json::Value;

pub const REMIXMIX: &str = "RemixMix";
pub const CLIENT: &str = "Client-X";

/// Canonical order: RemixMix, then Client-X. Empty input clears the column.
/// Unknown values are rejected. Duplicates collapse.
pub fn normalize_claim_flags(flags: &[String]) -> Result<Option<String>, String> {
    let mut remix = false;
    let mut client = false;
    for flag in flags {
        match flag.trim() {
            "" => {}
            REMIXMIX => remix = true,
            CLIENT => client = true,
            other => return Err(format!("unknown claim flag: {other}")),
        }
    }
    let mut out: Vec<&str> = Vec::new();
    if remix {
        out.push(REMIXMIX);
    }
    if client {
        out.push(CLIENT);
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&out).expect("string vec serialises")))
}

/// Read a stored JSON array. A missing or unreadable value is no flags.
/// Values are returned as stored so an older spelling is not rewritten on read.
pub fn parse_stored(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn is_dual(flags: &[String]) -> bool {
    flags.iter().any(|f| f == REMIXMIX) && flags.iter().any(|f| f == CLIENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::db::Db;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    #[test]
    fn normalise_orders_and_rejects() {
        assert_eq!(normalize_claim_flags(&[]).unwrap(), None);
        assert_eq!(
            normalize_claim_flags(&[REMIXMIX.into()]).unwrap().as_deref(),
            Some("[\"RemixMix\"]")
        );
        assert_eq!(
            normalize_claim_flags(&[" Client-X ".into(), "Client-X".into()]).unwrap().as_deref(),
            Some("[\"Client-X\"]")
        );
        assert_eq!(
            normalize_claim_flags(&["Client-X".into(), "RemixMix".into(), "RemixMix".into()])
                .unwrap()
                .as_deref(),
            Some("[\"RemixMix\",\"Client-X\"]")
        );
        assert!(normalize_claim_flags(&["remixmix".into()]).is_err());
        assert!(normalize_claim_flags(&["Other".into()]).is_err());
    }

    #[test]
    fn dual_needs_both_canonical_flags() {
        assert!(!is_dual(&parse_stored(Some("[\"RemixMix\"]"))));
        assert!(!is_dual(&parse_stored(Some("[\"remixmix\"]"))));
        assert!(is_dual(&parse_stored(Some(
            "[\"RemixMix\",\"Client-X\"]"
        ))));
    }

    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([7u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db { conn: std::sync::Arc::new(conn), crypto }
    }

    async fn flag_json(db: &Db, id: i64) -> Option<String> {
        let mut rows = db
            .conn
            .query(
                "SELECT claim_flags, claim_pack_id, claim_pack_id_2 FROM transactions WHERE id = ?1",
                libsql::params![id],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(row.get::<Option<String>>(1).unwrap().is_none());
        assert!(row.get::<Option<String>>(2).unwrap().is_none());
        row.get(0).unwrap()
    }

    #[tokio::test]
    async fn marks_one_remixmix_one_client_and_one_dual() {
        let db = test_db().await;
        let account = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for (pid, desc) in [("b7-a", "TRAIN"), ("b7-b", "HOTEL"), ("b7-c", "TAXI")] {
            let id = db
                .upsert_transaction(
                    account.id, pid, 1_700_000_000, desc, 1000, "GBP", false, false, None, None,
                    None, None, None, None,
                )
                .await
                .unwrap();
            ids.push(id);
        }

        let remix = normalize_claim_flags(&[REMIXMIX.into()]).unwrap();
        let client = normalize_claim_flags(&[CLIENT.into()]).unwrap();
        let both = normalize_claim_flags(&[CLIENT.into(), REMIXMIX.into()]).unwrap();
        assert!(db.update_transaction_claim_flags(ids[0], remix.as_deref()).await.unwrap());
        assert!(db.update_transaction_claim_flags(ids[1], client.as_deref()).await.unwrap());
        assert!(db.update_transaction_claim_flags(ids[2], both.as_deref()).await.unwrap());

        assert_eq!(flag_json(&db, ids[0]).await.as_deref(), Some("[\"RemixMix\"]"));
        assert_eq!(flag_json(&db, ids[1]).await.as_deref(), Some("[\"Client-X\"]"));
        assert_eq!(
            flag_json(&db, ids[2]).await.as_deref(),
            Some("[\"RemixMix\",\"Client-X\"]")
        );

        let dual = db.get_transaction(ids[2]).await.unwrap().unwrap();
        assert!(is_dual(&dual.claim_flags));
        assert_eq!(dual.claim_flags, vec![REMIXMIX, CLIENT]);

        let only = [account.id];
        let before = db
            .count_transactions_filtered(Some(&only), None, None, None)
            .await
            .unwrap();
        db.upsert_transaction(
            account.id, "b7-c", 1_700_000_100, "TAXI UPDATED", 1250, "GBP", false, false, None,
            None, None, None, None, None,
        )
        .await
        .unwrap();
        let after = db
            .count_transactions_filtered(Some(&only), None, None, None)
            .await
            .unwrap();
        assert_eq!(before, 3);
        assert_eq!(after, 3);
        let again = db.get_transaction(ids[2]).await.unwrap().unwrap();
        assert_eq!(again.description, "TAXI UPDATED");
        assert_eq!(again.amount_cents, 1250);
        assert!(is_dual(&again.claim_flags));
        assert_eq!(
            flag_json(&db, ids[2]).await.as_deref(),
            Some("[\"RemixMix\",\"Client-X\"]")
        );

        let receipt_id = db
            .insert_pending_receipt("immich-photo-1", Some(1250), Some(1_700_000_000), "unmatched")
            .await
            .unwrap();
        assert!(db
            .update_pending_receipt_claim_flags(receipt_id, both.as_deref())
            .await
            .unwrap());
        let receipt = db.get_pending_receipt(receipt_id).await.unwrap().unwrap();
        assert!(is_dual(&receipt.claim_flags));
        assert_eq!(receipt.status, "unmatched");
        assert!(receipt.linked_transaction_id.is_none());
    }
}
