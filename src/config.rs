use std::env;
use std::path::PathBuf;

pub const QODER_SIGNUP: &str = "https://qoder.com/users/sign-up";
pub const QODER_SIGNIN: &str = "https://qoder.com/users/sign-in";
pub const QODER_ACCOUNTS: &str = "https://qoder.com/device/selectAccounts";
pub const QODER_OPENAPI: &str = "https://openapi.qoder.sh";
pub const QODER_PAT_API: &str = "https://qoder.com/api/v1/me/personal-access-tokens";
pub const QODER_USERS_API: &str = "https://qoder.com/api/v1/users";
pub const DEVICE_TOKEN_URL: &str = "https://openapi.qoder.sh/api/v1/deviceToken/poll";
pub const USERINFO_URL: &str = "https://openapi.qoder.sh/api/v1/userinfo";

pub const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub fn env_or(key: &str, def: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => def.to_string(),
    }
}

pub fn env_int(key: &str, def: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(def)
}

pub fn tempmail_url() -> String {
    env_or("TEMPMAIL_URL", "http://localhost:6020")
}

pub fn default_password() -> String {
    env_or("QODER_PASSWORD", "ChangeMe123!Xyz")
}

pub fn home_dir() -> PathBuf {
    env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/root"))
}

/// Direktori data (accounts.json, log). Default: CWD, override QODER_DATA_DIR.
pub fn work_dir() -> PathBuf {
    if let Ok(d) = env::var("QODER_DATA_DIR") {
        if !d.trim().is_empty() {
            let p = PathBuf::from(d.trim());
            let _ = std::fs::create_dir_all(&p);
            return p;
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn accounts_path() -> PathBuf {
    work_dir().join("accounts.json")
}

pub fn log_path() -> PathBuf {
    work_dir().join("qoder_log.txt")
}

pub fn router_db_path() -> PathBuf {
    home_dir().join(".9router").join("db").join("data.sqlite")
}

pub fn qoderwake_path() -> String {
    if let Ok(p) = env::var("QODERWAKE_PATH") {
        if !p.trim().is_empty() {
            return p.trim().to_string();
        }
    }
    home_dir().join(".qoderwake").join("qoderwake").to_string_lossy().to_string()
}
