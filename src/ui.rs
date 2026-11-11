use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

pub const RED: &str = "\x1b[91m";
pub const GREEN: &str = "\x1b[92m";
pub const YELLOW: &str = "\x1b[93m";
pub const BLUE: &str = "\x1b[94m";
pub const CYAN: &str = "\x1b[96m";
pub const WHITE: &str = "\x1b[97m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";

static LOG_LOCK: Mutex<()> = Mutex::new(());

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn log_to_file(msg: &str) {
    let _guard = LOG_LOCK.lock();
    let path = crate::config::log_path();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let ts = chrono_like_now();
        let _ = writeln!(f, "{} [INFO] {}", ts, strip_ansi(msg));
    }
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Format detik epoch menjadi "YYYY-MM-DD HH:MM:SS" UTC tanpa crate chrono.
pub fn format_utc(epoch: u64) -> String {
    let days = epoch / 86_400;
    let rem = epoch % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let mut year = 1970i64;
    let mut d = days as i64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let yd = if leap { 366 } else { 365 };
        if d < yd {
            break;
        }
        d -= yd;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1;
    for (i, len) in months.iter().enumerate() {
        if d < *len {
            month = i + 1;
            break;
        }
        d -= len;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, d + 1, h, m, s)
}

/// Cetak berwarna + tulis ke log.
pub fn pc(text: &str, color: &str) {
    println!("{}{}{}", color, text, RESET);
    log_to_file(text);
}

pub fn ok(msg: &str) {
    pc(&format!("  ✅ {}", msg), GREEN);
}

pub fn warn(msg: &str) {
    pc(&format!("  ⚠  {}", msg), YELLOW);
}

pub fn fail(msg: &str) {
    pc(&format!("  ❌ {}", msg), RED);
}

pub fn info(msg: &str) {
    pc(&format!("  {}", msg), CYAN);
}

pub fn banner() {
    println!(
        "{}\n╔══════════════════════════════════════════════════════╗\n\
         ║     Qoder Creator — Rust Edition                      ║\n\
         ║     Register + Claim Pro Trial + Inject 9Router       ║\n\
         ╚══════════════════════════════════════════════════════╝{}",
        CYAN, RESET
    );
}

pub fn init_log() {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::config::log_path());
}

/// ISO-8601 UTC ("YYYY-MM-DDTHH:MM:SSZ") untuk field created_at.
pub fn iso_utc(epoch: u64) -> String {
    format_utc(epoch).replace(' ', "T") + "Z"
}
