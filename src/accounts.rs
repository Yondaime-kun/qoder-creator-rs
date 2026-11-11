use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    pub success: bool,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub otp: String,
    #[serde(default)]
    pub pat: String,
    #[serde(default)]
    pub trial_claimed: bool,
    #[serde(default)]
    pub qoderwake_login: bool,
    #[serde(default)]
    pub router_injected: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub created_at: String,
}

pub fn load() -> Result<Vec<Account>> {
    let path = crate::config::accounts_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&raw)?)
}

pub fn save(account: &Account) -> Result<()> {
    let mut list = load().unwrap_or_default();
    list.push(Account {
        success: account.success,
        email: account.email.clone(),
        password: account.password.clone(),
        otp: account.otp.clone(),
        pat: account.pat.clone(),
        trial_claimed: account.trial_claimed,
        qoderwake_login: account.qoderwake_login,
        router_injected: account.router_injected,
        error: account.error.clone(),
        created_at: account.created_at.clone(),
    });
    let path = crate::config::accounts_path();
    std::fs::write(path, serde_json::to_string_pretty(&list)?)?;
    Ok(())
}

pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::ui::iso_utc(secs)
}

pub fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[allow(dead_code)]
pub fn accounts_file() -> PathBuf {
    crate::config::accounts_path()
}

pub fn require_file(path: &PathBuf, label: &str) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(anyhow!("{} tidak ditemukan: {}", label, path.display()))
    }
}
