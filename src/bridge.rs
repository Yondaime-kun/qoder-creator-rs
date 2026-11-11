use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

/// Bridge menyalakan `bridge.py` (camoufox) sebagai subprocess persisten dan
/// mengirim satu JSON per baris ke stdin-nya.
pub struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: Option<BufReader<ChildStdout>>,
    _python: PathBuf,
}

impl Bridge {
    /// Menyalakan proses bridge. Path script dicari di env BRIDGE_PY, lalu
    /// di sebelah binary, lalu di CWD.
    pub fn spawn() -> Result<Self> {
        let script = locate_bridge()?;
        let python = crate::config::env_or("BRIDGE_PYTHON", "python3");

        let mut child = Command::new(&python)
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("PYTHONUNBUFFERED", "1")
            .spawn()
            .map_err(|e| anyhow!("gagal menjalankan {} {}: {}", python, script.display(), e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("stdin bridge tidak tersedia"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("stdout bridge tidak tersedia"))?;

        Ok(Bridge {
            child,
            stdin,
            stdout: Some(BufReader::new(stdout)),
            _python: PathBuf::from(python),
        })
    }

    /// Mengirim satu command dan menunggu satu baris JSON balasan.
    ///
    /// Bridge bisa menggantung kalau aksi browser memicu loading yang tidak
    /// pernah selesai (mis. klik tombol yang memuat SDK CAPTCHA). Batas waktu
    /// di sini memastikan proses tidak menggantung selamanya: kalau lewat,
    /// proses bridge dimatikan dan pemanggil menerima error.
    pub fn call(&mut self, payload: Value) -> Result<Value> {
        let line = serde_json::to_string(&payload)?;
        writeln!(self.stdin, "{}", line)?;
        self.stdin.flush()?;

        let timeout = std::time::Duration::from_secs(
            std::env::var("BRIDGE_TIMEOUT")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(180),
        );

        // Baca baris balasan di thread terpisah supaya bisa diberi batas waktu.
        let stdout = match self.stdout.take() {
            Some(s) => s,
            None => return Err(anyhow!("bridge tidak siap (stdout kosong)")),
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut reader = stdout;
            let mut resp = String::new();
            let n = reader.read_line(&mut resp);
            let _ = tx.send((n, resp, reader));
        });

        match rx.recv_timeout(timeout) {
            Ok((n, resp, reader)) => {
                self.stdout = Some(reader);
                let _ = handle.join();

                let n = n?;
                if n == 0 {
                    return Err(anyhow!("bridge menutup koneksi tanpa balasan"));
                }
                let parsed: Value = serde_json::from_str(resp.trim())
                    .map_err(|e| anyhow!("balasan bridge bukan JSON: {}", e))?;

                if parsed.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    let msg = parsed
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    let trace = parsed.get("trace").and_then(|v| v.as_str()).unwrap_or("");
                    if !trace.is_empty() {
                        crate::ui::pc(trace, crate::ui::YELLOW);
                    }
                    return Err(anyhow!("{}", msg));
                }
                Ok(parsed)
            }
            Err(_) => {
                // Timeout: matikan bridge; pemanggil akan membuat yang baru.
                let _ = self.child.kill();
                let _ = self.child.wait();
                Err(anyhow!(
                    "bridge timeout setelah {}s (aksi browser menggantung)",
                    timeout.as_secs()
                ))
            }
        }
    }

    pub fn start(&mut self, headless: bool) -> Result<String> {
        let r = self.call(json!({"action": "start", "headless": headless}))?;
        Ok(r.get("userAgent")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string())
    }

    pub fn navigate(&mut self, url: &str) -> Result<()> {
        self.call(json!({"action": "navigate", "url": url}))?;
        Ok(())
    }

    /// Tutup browser lalu buka lagi, kembali ke halaman sign-up yang sudah
    /// terisi. Sesi CAPTCHA terikat ke instance browser + cookie, jadi setelah
    /// percobaan yang ditolak satu-satunya cara mendapat penilaian bersih
    /// adalah instance baru.
    pub fn restart(&mut self, headless: bool) -> Result<String> {
        self.call(json!({"action": "close"}))?;
        std::thread::sleep(std::time::Duration::from_millis(400));
        let ua = self.start(headless)?;
        self.navigate(crate::config::QODER_SIGNUP)?;
        Ok(ua)
    }

    pub fn fill_registration(&mut self, email: &str, password: &str) -> Result<String> {
        let r = self.call(json!({
            "action": "fill_registration",
            "email": email,
            "password": password,
        }))?;
        Ok(r.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string())
    }

    /// Ambil gambar + geometri untuk satu percobaan CAPTCHA.
    pub fn captcha_geometry(&mut self) -> Result<Option<CaptchaShot>> {
        let r = self.call(json!({"action": "captcha_geometry"}))?;
        if r.get("ready").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(None);
        }
        Ok(Some(CaptchaShot {
            bg: r.get("bg").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            puzzle: r.get("puzzle").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            geometry: r.get("geometry").cloned().unwrap_or(Value::Null),
        }))
    }

    pub fn captcha_drag(&mut self, target_display: f64, geometry: Value) -> Result<DragResult> {
        let r = self.call(json!({
            "action": "captcha_drag",
            "targetDisplay": target_display,
            "geometry": geometry,
        }))?;
        Ok(DragResult {
            aligned: r.get("aligned").and_then(|v| v.as_bool()).unwrap_or(false),
            solved: r.get("solved").and_then(|v| v.as_bool()).unwrap_or(false),
            impossible: r
                .get("state")
                .and_then(|s| s.get("impossible"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            target: r
                .get("state")
                .and_then(|s| s.get("target"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            max_offset: r
                .get("state")
                .and_then(|s| s.get("maxOffset"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        })
    }

    /// Minta soal CAPTCHA baru setelah percobaan yang gagal.
    pub fn captcha_refresh(&mut self, email: &str, password: &str) -> Result<bool> {
        let r = self.call(json!({
            "action": "captcha_refresh",
            "email": email,
            "password": password,
        }))?;
        Ok(r.get("ready").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    pub fn wait_otp_form(&mut self) -> Result<bool> {
        let r = self.call(json!({"action": "wait_otp_form"}))?;
        Ok(r.get("ready").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    pub fn submit_otp(&mut self, otp: &str, email: &str, password: &str) -> Result<OtpResult> {
        let r = self.call(json!({
            "action": "submit_otp",
            "otp": otp,
            "email": email,
            "password": password,
        }))?;
        Ok(OtpResult {
            filled: r.get("filled").and_then(|v| v.as_bool()).unwrap_or(false),
            blocked: r.get("blocked").and_then(|v| v.as_bool()).unwrap_or(false),
            api_status: r.get("status").and_then(|v| v.as_i64()),
            verified: r.get("verified")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| r.get("status")
                    .and_then(|v| v.as_i64())
                    .map(|s| s == 200)
                    .unwrap_or(false)),
            punished: r.get("punished").and_then(|v| v.as_bool()).unwrap_or(false),
            url: r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })
    }

    /// Apakah sesi browser sudah terautentikasi. Dipakai untuk melewati login
    /// ulang setelah verifikasi OTP, yang biasanya sudah membuat sesi aktif.
    pub fn check_session(&mut self) -> Result<bool> {
        let r = self.call(json!({"action": "check_session"}))?;
        Ok(r.get("authenticated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Simpan cookie sesi qoder.com ke file. Cookie ini dipakai untuk masuk ke
    /// Qoder GUI tanpa OAuth, sehingga Pro Trial bisa diaktifkan di client.
    pub fn export_cookies(&mut self, path: &str) -> Result<usize> {
        let r = self.call(json!({"action": "export_cookies", "path": path}))?;
        if r.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            anyhow::bail!("export cookie gagal");
        }
        Ok(r.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    /// Tahap 1 login: kredensial, lalu tunggu CAPTCHA atau field kode.
    pub fn signin_credentials(&mut self, email: &str, password: &str) -> Result<SigninStage> {
        let r = self.call(json!({
            "action": "signin_credentials",
            "email": email,
            "password": password,
        }))?;
        Ok(SigninStage {
            captcha: r.get("captcha").and_then(|v| v.as_bool()).unwrap_or(false),
            code_ready: r.get("code_ready").and_then(|v| v.as_bool()).unwrap_or(false),
            url: r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })
    }

    /// Tahap akhir login: isi verification code dan tunggu sesi aktif.
    pub fn signin_code(&mut self, otp: &str, password: &str) -> Result<SigninResult> {
        let r = self.call(json!({
            "action": "signin_code",
            "otp": otp,
            "password": password,
        }))?;
        let dump = r.get("dump").map(|d| d.to_string()).unwrap_or_default();
        Ok(SigninResult {
            authenticated: r.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false),
            me_status: r.get("me_status").and_then(|v| v.as_i64()),
            code_filled: r.get("code_filled").and_then(|v| v.as_bool()).unwrap_or(false),
            url: r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            dump,
        })
    }

    /// Login penuh memakai kode verifikasi untuk mendapatkan sesi cookie valid.
    pub fn signin_full(&mut self, email: &str, password: &str, otp: &str) -> Result<SigninResult> {
        let r = self.call(json!({
            "action": "signin_full",
            "email": email,
            "password": password,
            "otp": otp,
        }))?;
        Ok(SigninResult {
            authenticated: r.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false),
            me_status: r.get("me_status").and_then(|v| v.as_i64()),
            code_filled: r.get("code_filled").and_then(|v| v.as_bool()).unwrap_or(false),
            url: r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            dump: String::new(),
        })
    }

    pub fn create_pat(&mut self) -> Result<PatResult> {
        let r = self.call(json!({"action": "create_pat"}))?;
        Ok(PatResult {
            status: r.get("status").and_then(|v| v.as_i64()).unwrap_or(0),
            token: r.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            body: r.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            csrf_found: r
                .get("csrf_found")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    pub fn shutdown(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"action": "shutdown"}));
        let _ = self.stdin.flush();
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"action": "shutdown"}));
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct CaptchaShot {
    pub bg: String,
    pub puzzle: String,
    pub geometry: Value,
}

impl CaptchaShot {
    /// Byte PNG gambar latar. Sumbernya bisa data URL (soal pertama) atau URL
    /// CDN Aliyun (soal setelah refresh), jadi kedua bentuk harus ditangani.
    pub fn bg_bytes(&self) -> anyhow::Result<Vec<u8>> {
        decode_image(&self.bg)
    }

    pub fn puzzle_bytes(&self) -> anyhow::Result<Vec<u8>> {
        decode_image(&self.puzzle)
    }
}

fn decode_image(src: &str) -> anyhow::Result<Vec<u8>> {
    if let Some(rest) = src.strip_prefix("data:") {
        let b64 = rest
            .split(',')
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("data URL tanpa payload"))?;
        return base64_decode(b64);
    }
    let resp = ureq::get(src)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| anyhow::anyhow!("unduh gambar: {}", e))?;
    let mut buf = Vec::new();
    use std::io::Read;
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| anyhow::anyhow!("baca gambar: {}", e))?;
    Ok(buf)
}

fn base64_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => continue,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

pub struct DragResult {
    pub aligned: bool,
    pub solved: bool,
    pub impossible: bool,
    pub target: f64,
    pub max_offset: f64,
}

pub struct OtpResult {
    pub filled: bool,
    pub blocked: bool,
    pub api_status: Option<i64>,
    pub verified: bool,
    pub punished: bool,
    pub url: String,
}

pub struct SigninStage {
    pub captcha: bool,
    pub code_ready: bool,
    pub url: String,
}

pub struct SigninResult {
    pub authenticated: bool,
    pub me_status: Option<i64>,
    pub code_filled: bool,
    pub url: String,
    pub dump: String,
}

pub struct PatResult {
    pub status: i64,
    pub token: String,
    pub body: String,
    pub csrf_found: bool,
}

/// Mencari bridge.py di beberapa kandidat lokasi standar.
fn locate_bridge() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("BRIDGE_SCRIPT") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("bridge.py"));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join("bridge.py"));
            }
        }
    }
    candidates.push(PathBuf::from("bridge.py"));

    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(anyhow!(
        "bridge.py tidak ditemukan (set BRIDGE_SCRIPT). Dicari di: {:?}",
        candidates
    ))
}
