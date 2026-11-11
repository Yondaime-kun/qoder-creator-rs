use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;

use crate::accounts::new_uuid;

#[derive(Serialize)]
struct ConnectionData {
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "apiKey")]
    api_key: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
    #[serde(rename = "testStatus")]
    test_status: String,
}

/// Menulis koneksi provider 'qoder' ke DB SQLite milik 9Router.
/// Idempoten: kalau email sudah ada, baris itu di-update.
pub fn inject(email: &str, api_key: &str) -> Result<bool> {
    let db_path = crate::config::router_db_path();
    if !db_path.exists() {
        crate::ui::warn(&format!(
            "DB 9Router tidak ada ({}), injeksi dilewati",
            db_path.display()
        ));
        return Ok(false);
    }

    let conn = Connection::open(&db_path)
        .map_err(|e| anyhow!("buka DB 9Router: {}", e))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;

    let name = email.split('@').next().unwrap_or(email).to_string();
    let now = crate::accounts::now_iso();
    let expires = "2027-08-03T09:59:59Z".to_string();

    let payload = serde_json::to_string(&ConnectionData {
        display_name: name.clone(),
        api_key: api_key.to_string(),
        expires_at: expires,
        test_status: "active".to_string(),
    })?;

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM providerConnections WHERE email=?1 AND provider='qoder'",
            params![email],
            |row| row.get(0),
        )
        .ok();

    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE providerConnections SET data=?1, name=?2, updatedAt=?3, isActive=1 WHERE id=?4",
                params![payload, name, now, id],
            )
            .map_err(|e| anyhow!("update koneksi: {}", e))?;
            crate::ui::ok("9Router: koneksi diperbarui");
        }
        None => {
            let id = new_uuid();
            conn.execute(
                "INSERT INTO providerConnections
                 (id, provider, authType, name, email, priority, isActive, data, createdAt, updatedAt)
                 VALUES (?1, 'qoder', 'oauth', ?2, ?3, 1, 1, ?4, ?5, ?5)",
                params![id, name, email, payload, now],
            )
            .map_err(|e| anyhow!("insert koneksi: {}", e))?;
            crate::ui::ok(&format!("9Router: koneksi baru ({})", &id[..8.min(id.len())]));
        }
    }

    Ok(true)
}

/// Cek apakah file SQLite punya tabel providerConnections (diagnostik).
pub fn schema_ok(path: &Path) -> bool {
    Connection::open(path)
        .and_then(|c| {
            c.query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='providerConnections'",
                [],
                |r| r.get::<_, String>(0),
            )
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_update_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("qoder-test-{}", new_uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("data.sqlite");

        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE providerConnections (
                    id TEXT PRIMARY KEY, provider TEXT, authType TEXT, name TEXT,
                    email TEXT, priority INTEGER, isActive INTEGER, data TEXT,
                    createdAt TEXT, updatedAt TEXT);",
            )
            .unwrap();
        }

        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO providerConnections
             (id, provider, authType, name, email, priority, isActive, data, createdAt, updatedAt)
             VALUES ('x','qoder','oauth','a','a@b.c',1,1,'{}','t','t')",
            [],
        )
        .unwrap();
        let count: i64 = c
            .query_row("SELECT COUNT(*) FROM providerConnections", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert!(schema_ok(&db));

        std::fs::remove_dir_all(&dir).ok();
    }
}
