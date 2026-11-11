use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Klien JoyMail (tmail.mhankbarbar.dev).
///
/// Modelnya beda dengan TempMail lokal: alamat tidak "dibuat" lewat API,
/// kita bebas memilih alamat apa pun pada domain aktif lalu membacanya.
/// Endpoint publik tidak butuh auth: GET /api/inbox/{address}.

#[derive(Debug, Deserialize)]
struct DomainsResp {
    #[serde(default)]
    enabled: bool,
    #[serde(default, rename = "customDomains")]
    custom_domains: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Mail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub html: String,
    #[serde(default)]
    pub sent_at: String,
}

#[derive(Debug, Deserialize)]
struct InboxResp {
    #[serde(default)]
    emails: Vec<Mail>,
}

fn base_url() -> String {
    crate::config::env_or("TMAIL_URL", "https://tmail.mhankbarbar.dev")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
}

/// Mengambil daftar domain aktif yang bisa dipakai.
pub fn domains() -> Result<Vec<String>> {
    let url = format!("{}/api/domains", base_url());
    let resp = agent()
        .get(&url)
        .call()
        .map_err(|e| anyhow!("GET /api/domains: {}", e))?;
    let parsed: DomainsResp = resp
        .into_json()
        .map_err(|e| anyhow!("parse /api/domains: {}", e))?;

    if !parsed.enabled {
        return Err(anyhow!("JoyMail melaporkan domain dinonaktifkan"));
    }
    if parsed.custom_domains.is_empty() {
        return Err(anyhow!("tidak ada domain aktif di JoyMail"));
    }
    Ok(parsed.custom_domains)
}

/// Membuat alamat acak pada domain yang benar-benar bisa menerima email.
///
/// `/api/domains` di JoyMail bisa melaporkan domain yang MX record-nya sudah
/// hilang (mis. domain expired) sebagai "active". Kalau alamat dibuat di
/// domain seperti itu, email verifikasi tidak akan pernah sampai dan alur
/// berhenti di tahap OTP. Karena itu tiap domain diperiksa MX-nya lebih dulu.
pub fn create_address() -> Result<String> {
    let doms = domains()?;

    // Override manual: pakai domain tertentu kalau user menentukannya.
    if let Ok(forced) = std::env::var("TMAIL_DOMAIN") {
        let forced = forced.trim().to_string();
        if !forced.is_empty() {
            return Ok(format!("{}@{}", random_tag(10), forced));
        }
    }

    let mut usable: Vec<String> = Vec::new();
    for d in &doms {
        match has_mx(d) {
            Ok(true) => usable.push(d.clone()),
            Ok(false) => crate::ui::warn(&format!("domain {} tidak punya MX, dilewati", d)),
            Err(_) => crate::ui::warn(&format!("MX {} tidak bisa dicek, dilewati", d)),
        }
    }

    // Kalau pemeriksaan MX gagal total (mis. resolver diblokir), pakai daftar
    // asli supaya tetap ada kesempatan jalan.
    if usable.is_empty() {
        crate::ui::warn("tidak ada domain dengan MX valid, memakai daftar apa adanya");
        usable = doms;
    }

    let domain = usable
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("domain kosong"))?;

    Ok(format!("{}@{}", random_tag(10), domain))
}

/// Cek keberadaan MX record lewat DNS-over-HTTPS Cloudflare.
/// Mengembalikan Ok(false) kalau domain jelas tidak punya MX.
fn has_mx(domain: &str) -> Result<bool> {
    let url = format!(
        "https://cloudflare-dns.com/dns-query?name={}&type=MX",
        domain
    );
    let resp = agent()
        .get(&url)
        .set("Accept", "application/dns-json")
        .call()
        .map_err(|e| anyhow!("doh {}: {}", domain, e))?;

    let body: serde_json::Value = resp
        .into_json()
        .map_err(|e| anyhow!("parse doh: {}", e))?;

    // Status 0 = NOERROR. Answer kosong berarti tidak ada MX.
    let status = body.get("Status").and_then(|v| v.as_i64()).unwrap_or(-1);
    if status != 0 {
        return Ok(false);
    }
    let answers = body
        .get("Answer")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    Ok(answers)
}

fn random_tag(n: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Membaca inbox. Endpoint publik sudah mengembalikan body lengkap
/// (text + html), jadi tidak perlu request detail per pesan.
pub fn fetch_inbox(address: &str) -> Result<Vec<Mail>> {
    let url = format!("{}/api/inbox/{}", base_url(), address);
    let resp = agent()
        .get(&url)
        .call()
        .map_err(|e| anyhow!("GET /api/inbox: {}", e))?;

    // 404 = inbox belum ada pesan; perlakukan sebagai kosong.
    if resp.status() == 404 {
        return Ok(Vec::new());
    }
    let parsed: InboxResp = resp
        .into_json()
        .map_err(|e| anyhow!("parse /api/inbox: {}", e))?;
    Ok(parsed.emails)
}

/// Polling sampai kode verifikasi ditemukan di salah satu pesan.
pub fn wait_for_otp(address: &str, max_wait: Duration, interval: Duration) -> Result<String> {
    let start = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut last_report = Instant::now();

    while start.elapsed() < max_wait {
        if let Ok(mails) = fetch_inbox(address) {
            // JoyMail mengurutkan terbaru dulu; periksa dari yang paling baru.
            for m in &mails {
                let key = if m.id.is_empty() {
                    format!("{}|{}", m.subject, m.text)
                } else {
                    m.id.clone()
                };
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                let blob = format!(
                    "{} {} {} {}",
                    m.text,
                    m.subject,
                    strip_html(&m.html),
                    m.from
                );
                if let Some(code) = extract_otp(&blob) {
                    return Ok(code);
                }
            }
        }

        if last_report.elapsed() >= Duration::from_secs(30) {
            crate::ui::info(&format!("Waiting OTP... ({}s)", start.elapsed().as_secs()));
            last_report = Instant::now();
        }
        std::thread::sleep(interval);
    }

    Err(anyhow!("OTP timeout setelah {}s", max_wait.as_secs()))
}

/// Buang tag HTML supaya kode yang hanya ada di markup tetap terbaca.
fn strip_html(html: &str) -> String {
    let tag = regex::Regex::new(r"<[^>]*>").unwrap();
    let ws = regex::Regex::new(r"\s+").unwrap();
    ws.replace_all(&tag.replace_all(html, " "), " ")
        .trim()
        .to_string()
}

/// Tiga lapis pencarian kode, sama seperti versi Python aslinya.
pub fn extract_otp(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let flat = regex::Regex::new(r"\s+").unwrap().replace_all(text, " ");

    let patterns = [
        r"(?i)(?:code|kode|otp|verif)[^\d]*(\d{4,8})",
        r"\b(\d{6})\b",
        r"\b(\d{4,8})\b",
    ];
    for p in patterns {
        let re = regex::Regex::new(p).unwrap();
        if let Some(c) = re.captures(&flat) {
            if let Some(m) = c.get(1) {
                return Some(m.as_str().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{extract_otp, strip_html};

    #[test]
    fn finds_contextual_code() {
        assert_eq!(
            extract_otp("Your verification code is 483920").unwrap(),
            "483920"
        );
    }

    #[test]
    fn finds_code_in_subject() {
        assert_eq!(
            extract_otp("Your Login Verification Code: 489102").unwrap(),
            "489102"
        );
    }

    #[test]
    fn falls_back_to_bare_six_digits() {
        assert_eq!(extract_otp("123456 is your Qoder code").unwrap(), "123456");
    }

    #[test]
    fn ignores_text_without_digits() {
        assert!(extract_otp("no digits here").is_none());
    }

    #[test]
    fn strips_html_tags() {
        let out = strip_html("<div><h2>489102</h2></div>");
        assert!(out.contains("489102"));
        assert!(!out.contains('<'));
    }
}
