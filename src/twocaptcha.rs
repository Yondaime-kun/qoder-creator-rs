//! Klien 2captcha untuk CAPTCHA slider Aliyun.
//!
//! Aliyun tidak punya task type khusus di 2captcha (tidak ada
//! `AliyunSliderTask`). Yang bisa dipakai adalah `CoordinatesTask`: gambar
//! latar dikirim, human solver menunjuk koordinat lubang puzzle, lalu
//! koordinat itu dipakai sebagai target drag. Hasil solver diverifikasi
//! terhadap deteksi NCC lokal — kalau keduanya jauh berbeda, soal dilewati
//! supaya tidak membuang percobaan pada posisi yang salah.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const API: &str = "https://2captcha.com";

pub struct TwoCaptcha {
    key: String,
    agent: ureq::Agent,
}

impl TwoCaptcha {
    /// `None` kalau kunci tidak diset — pemanggil lanjut dengan NCC saja.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("TWOCAPTCHA_KEY").ok()?;
        let key = key.trim().to_string();
        if key.is_empty() {
            return None;
        }
        Some(Self {
            key,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(60))
                .build(),
        })
    }

    /// Kirim gambar latar + potongan puzzle, tunggu human solver menunjuk
    /// lubang. Mengembalikan koordinat x (dalam piksel gambar asli).
    ///
    /// Gambar yang dikirim adalah komposisi: latar penuh di atas, potongan
    /// puzzle di bawahnya. Tanpa potongan, solver tidak tahu bentuk lubang
    /// yang dicari dan sering menunjuk tekstur yang salah.
    pub fn solve_slider(&self, bg_png: &[u8], pz_png: &[u8]) -> Result<i64> {
        let composed = compose(bg_png, pz_png).unwrap_or_else(|_| bg_png.to_vec());
        let body = base64_encode(&composed);
        let hint = "Top image: a background with a puzzle-piece shaped hole. \
                    Bottom image: the puzzle piece that fits that hole. \
                    Click the CENTER of the hole in the TOP image. \
                    Reply with x,y in pixels of the top image, origin top-left.";

        let resp = self
            .agent
            .post(&format!("{}/createTask", API))
            .send_json(json!({
                "clientKey": self.key,
                "task": {
                    "type": "CoordinatesTask",
                    "body": body,
                    "comment": hint,
                }
            }))
            .map_err(|e| anyhow!("createTask: {}", e))?
            .into_json::<Value>()
            .map_err(|e| anyhow!("createTask json: {}", e))?;

        if let Some(id) = resp.get("errorId").and_then(|v| v.as_i64()) {
            if id != 0 {
                return Err(anyhow!(
                    "createTask: {}",
                    resp.get("errorCode").and_then(|v| v.as_str()).unwrap_or("?")
                ));
            }
        }
        let task_id = resp
            .get("taskId")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("createTask tanpa taskId"))?;

        self.poll(task_id)
    }

    fn poll(&self, task_id: i64) -> Result<i64> {
        let deadline = Instant::now() + Duration::from_secs(240);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(6));
            let resp = self
                .agent
                .post(&format!("{}/getTaskResult", API))
                .send_json(json!({"clientKey": self.key, "taskId": task_id}))
                .map_err(|e| anyhow!("getTaskResult: {}", e))?
                .into_json::<Value>()
                .map_err(|e| anyhow!("getTaskResult json: {}", e))?;

            if let Some(code) = resp.get("errorCode").and_then(|v| v.as_str()) {
                if code == "CAPCHA_NOT_READY" {
                    continue;
                }
                return Err(anyhow!("solver: {}", code));
            }
            if resp.get("status").and_then(|v| v.as_str()) == Some("ready") {
                let x = resp
                    .pointer("/solution/coordinates/0/x")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| anyhow!("solution tanpa koordinat x"))?;
                return Ok(x);
            }
        }
        Err(anyhow!("solver timeout"))
    }

    /// Saldo akun, dipakai untuk memastikan kunci masih aktif sebelum
    /// menjalankan alur panjang.
    pub fn balance(&self) -> Result<f64> {
        let resp = self
            .agent
            .get(&format!(
                "{}/res.php?key={}&action=getbalance&json=1",
                API, self.key
            ))
            .call()
            .map_err(|e| anyhow!("balance: {}", e))?
            .into_json::<Value>()
            .map_err(|e| anyhow!("balance json: {}", e))?;
        resp.get("request")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .ok_or_else(|| anyhow!("balance tidak terbaca"))
    }
}

/// Gabungkan gambar latar dan potongan puzzle jadi satu gambar vertikal.
/// Solver perlu melihat bentuk potongan untuk tahu lubang mana yang dicari.
fn compose(bg_png: &[u8], pz_png: &[u8]) -> Result<Vec<u8>> {
    use image::{ImageFormat, RgbaImage};
    use std::io::Cursor;

    let bg = image::load_from_memory_with_format(bg_png, ImageFormat::Png)
        .map_err(|e| anyhow!("decode bg: {}", e))?
        .to_rgba8();
    let pz = image::load_from_memory_with_format(pz_png, ImageFormat::Png)
        .map_err(|e| anyhow!("decode puzzle: {}", e))?
        .to_rgba8();

    let (w, bh) = (bg.width(), bg.height());
    let (pw, ph) = (pz.width(), pz.height());
    let gap = 8u32;

    let mut out = RgbaImage::from_pixel(w.max(pw), bh + gap + ph, image::Rgba([24, 24, 24, 255]));
    image::imageops::overlay(&mut out, &bg, 0, 0);
    image::imageops::overlay(&mut out, &pz, 0, (bh + gap) as i64);

    let mut buf = Cursor::new(Vec::new());
    out.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| anyhow!("encode composed: {}", e))?;
    Ok(buf.into_inner())
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
