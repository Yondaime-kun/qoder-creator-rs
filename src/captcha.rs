use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, GenericImageView, RgbaImage};
use serde::Serialize;

/// Matematika CAPTCHA Aliyun, seluruhnya native Rust:
/// decode PNG dari data URL -> grayscale -> blur 3x3 -> masked NCC
/// (domain grayscale dan domain tepi/Sobel) -> voting jarak <15px.

#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f64>,
}

impl Gray {
    fn at(&self, x: isize, y: isize) -> f64 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0.0;
        }
        self.px[y as usize * self.w + x as usize]
    }
}

pub fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let idx = data_url
        .find(',')
        .ok_or_else(|| anyhow!("data URL tidak valid"))?;
    STANDARD
        .decode(&data_url[idx + 1..])
        .map_err(|e| anyhow!("decode base64: {}", e))
}

fn to_gray(img: &DynamicImage) -> Gray {
    let (w, h) = img.dimensions();
    let rgba: RgbaImage = img.to_rgba8();
    let mut px = Vec::with_capacity((w * h) as usize);
    for p in rgba.pixels() {
        let [r, g, b, _] = p.0;
        px.push(r as f64 * 0.299 + g as f64 * 0.587 + b as f64 * 0.114);
    }
    Gray {
        w: w as usize,
        h: h as usize,
        px,
    }
}

/// Kernel Gaussian 3x3 [1 2 1; 2 4 2; 1 2 1]/16.
fn blur3(src: &Gray) -> Gray {
    let mut out = Gray {
        w: src.w,
        h: src.h,
        px: vec![0.0; src.px.len()],
    };
    for y in 1..src.h.saturating_sub(1) {
        for x in 1..src.w.saturating_sub(1) {
            let v = src.at(x as isize - 1, y as isize - 1)
                + 2.0 * src.at(x as isize, y as isize - 1)
                + src.at(x as isize + 1, y as isize - 1)
                + 2.0 * src.at(x as isize - 1, y as isize)
                + 4.0 * src.at(x as isize, y as isize)
                + 2.0 * src.at(x as isize + 1, y as isize)
                + src.at(x as isize - 1, y as isize + 1)
                + 2.0 * src.at(x as isize, y as isize + 1)
                + src.at(x as isize + 1, y as isize + 1);
            out.px[y * src.w + x] = v / 16.0;
        }
    }
    out
}

/// Magnitudo gradien Sobel untuk menonjolkan tepi.
fn sobel_mag(src: &Gray) -> Gray {
    let mut out = Gray {
        w: src.w,
        h: src.h,
        px: vec![0.0; src.px.len()],
    };
    for y in 1..src.h.saturating_sub(1) {
        for x in 1..src.w.saturating_sub(1) {
            let (xi, yi) = (x as isize, y as isize);
            let gx = -src.at(xi - 1, yi - 1) + src.at(xi + 1, yi - 1)
                - 2.0 * src.at(xi - 1, yi)
                + 2.0 * src.at(xi + 1, yi)
                - src.at(xi - 1, yi + 1)
                + src.at(xi + 1, yi + 1);
            let gy = -src.at(xi - 1, yi - 1)
                - 2.0 * src.at(xi, yi - 1)
                - src.at(xi + 1, yi - 1)
                + src.at(xi - 1, yi + 1)
                + 2.0 * src.at(xi, yi + 1)
                + src.at(xi + 1, yi + 1);
            out.px[y * src.w + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct NccHit {
    x: i64,
    score: f64,
}

/// Masked normalized cross-correlation: hanya piksel bertopeng yang dihitung.
///
/// Rentang Y dicari di seluruh tinggi gambar, bukan hanya di sekitar posisi
/// awal potongan. Potongan pada soal Aliyun bisa berada di ketinggian yang
/// berbeda dari posisi elemen puzzle di halaman, dan membatasi rentang Y
/// membuat pencarian mengunci offset yang salah (selisih beberapa piksel).
fn masked_ncc(src: &Gray, tpl: &Gray, mask: &[u8], off_y: i64) -> NccHit {
    let mut best = NccHit {
        x: -1,
        score: -2.0,
    };
    let (tw, th) = (tpl.w as i64, tpl.h as i64);
    let mask_n: usize = mask.iter().filter(|v| **v == 1).count();
    if mask_n == 0 || tw <= 0 || th <= 0 {
        return best;
    }

    let _ = off_y;
    let y_min = 0i64;
    let y_max = (src.h as i64 - th).max(0);
    let x_max = src.w as i64 - tw - 1;

    let mut ox = 15i64;
    while ox < x_max {
        let mut oy = y_min;
        while oy <= y_max {
            let mut t_sum = 0.0;
            let mut s_sum = 0.0;
            let mut n = 0.0;

            for (i, m) in mask.iter().enumerate() {
                if *m == 0 {
                    continue;
                }
                let tx = (i % tpl.w) as isize;
                let ty = (i / tpl.w) as isize;
                t_sum += tpl.px[i];
                s_sum += src.at(ox as isize + tx, oy as isize + ty);
                n += 1.0;
            }
            if n == 0.0 {
                oy += 1;
                continue;
            }
            let t_mean = t_sum / n;
            let s_mean = s_sum / n;

            let mut t_var = 0.0;
            let mut s_var = 0.0;
            let mut cov = 0.0;
            for (i, m) in mask.iter().enumerate() {
                if *m == 0 {
                    continue;
                }
                let tx = (i % tpl.w) as isize;
                let ty = (i / tpl.w) as isize;
                let td = tpl.px[i] - t_mean;
                let sd = src.at(ox as isize + tx, oy as isize + ty) - s_mean;
                t_var += td * td;
                s_var += sd * sd;
                cov += sd * td;
            }
            t_var /= n;
            s_var /= n;

            let t_std = t_var.sqrt();
            let s_std = s_var.sqrt();
            if t_std >= 0.001 && s_std >= 0.001 {
                let score = cov / (n * s_std * t_std);
                if score > best.score {
                    best = NccHit { x: ox, score };
                }
            }
            oy += 1;
        }
        ox += 1;
    }
    best
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub x: i64,
    pub s: f64,
    pub m: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gap {
    #[serde(rename = "targetX")]
    pub target_x: i64,
    pub score: f64,
    pub method: String,
    /// Offset horizontal potongan di dalam gambar puzzle (bounding box alpha).
    /// Posisi yang dicocokkan NCC adalah tepi kiri *potongan*, sedangkan yang
    /// digeser adalah elemen gambar puzzle-nya, jadi nilai ini harus
    /// dikurangkan sebelum dikonversi ke koordinat layar.
    #[serde(rename = "puzzleOffsetX")]
    pub puzzle_offset_x: i64,
    pub candidates: Vec<Candidate>,
}

/// Menghitung posisi gap dari dua data URL PNG (background & puzzle).
pub fn detect_gap(bg_url: &str, pz_url: &str) -> Result<Gap> {
    let bg_bytes = decode_data_url(bg_url)?;
    let pz_bytes = decode_data_url(pz_url)?;

    let bg_img = image::load_from_memory(&bg_bytes)
        .map_err(|e| anyhow!("decode background: {}", e))?;
    let pz_img = image::load_from_memory(&pz_bytes)
        .map_err(|e| anyhow!("decode puzzle: {}", e))?;

    let bg_gray = to_gray(&bg_img);
    let pz_rgba = pz_img.to_rgba8();
    let (pw, ph) = pz_rgba.dimensions();

    // Cari bounding box alpha > 128 pada potongan puzzle.
    let (mut min_x, mut max_x) = (pw as i64, 0i64);
    let (mut min_y, mut max_y) = (ph as i64, 0i64);
    for y in 0..ph {
        for x in 0..pw {
            if pz_rgba.get_pixel(x, y).0[3] > 128 {
                let (xi, yi) = (x as i64, y as i64);
                if xi < min_x {
                    min_x = xi;
                }
                if xi > max_x {
                    max_x = xi;
                }
                if yi < min_y {
                    min_y = yi;
                }
                if yi > max_y {
                    max_y = yi;
                }
            }
        }
    }

    let crop_w = (max_x - min_x + 1).max(0) as usize;
    let crop_h = (max_y - min_y + 1).max(0) as usize;
    if crop_w < 10 || crop_h < 10 {
        return Err(anyhow!("bounding box puzzle terlalu kecil ({}x{})", crop_w, crop_h));
    }

    let mut crop = Gray {
        w: crop_w,
        h: crop_h,
        px: vec![0.0; crop_w * crop_h],
    };
    let mut mask = vec![0u8; crop_w * crop_h];

    for cy in 0..crop_h {
        for cx in 0..crop_w {
            let sx = (min_x + cx as i64) as u32;
            let sy = (min_y + cy as i64) as u32;
            let p = pz_rgba.get_pixel(sx, sy).0;
            let di = cy * crop_w + cx;
            crop.px[di] = p[0] as f64 * 0.299 + p[1] as f64 * 0.587 + p[2] as f64 * 0.114;
            mask[di] = if p[3] > 128 { 1 } else { 0 };
        }
    }

    let bg_blur = &bg_gray;
    let pz_blur = &crop;

    let gray_hit = masked_ncc(bg_blur, pz_blur, &mask, min_y);
    let edge_hit = masked_ncc(
        &sobel_mag(bg_blur),
        &sobel_mag(pz_blur),
        &mask,
        min_y,
    );

    let mut cands: Vec<Candidate> = Vec::new();
    if gray_hit.x >= 15 {
        cands.push(Candidate {
            x: gray_hit.x,
            s: gray_hit.score,
            m: "gray-ncc".into(),
        });
    }
    if edge_hit.x >= 15 {
        cands.push(Candidate {
            x: edge_hit.x,
            s: edge_hit.score,
            m: "edge-ncc".into(),
        });
    }

    let mut target_x = -1i64;
    let mut score = 0.0;
    let mut method = String::new();

    // Voting: kalau dua strategi sepakat dalam 15px, pakai rata-ratanya.
    for i in 0..cands.len() {
        for j in (i + 1)..cands.len() {
            if (cands[i].x - cands[j].x).abs() < 15 {
                let avg = ((cands[i].x + cands[j].x) as f64 / 2.0).round() as i64;
                let sc = (cands[i].s + cands[j].s) / 2.0;
                if sc > score {
                    target_x = avg;
                    score = sc;
                    method = format!("{}+{}", cands[i].m, cands[j].m);
                }
            }
        }
    }

    // Kalau kedua strategi tidak sepakat, jangan pilih berdasarkan skor saja:
    // NCC tepi mudah menang di posisi salah karena gradien latar bisa sangat
    // mirip di banyak tempat, sedangkan NCC grayscale membandingkan seluruh
    // isi potongan. Karena itu gray-ncc diprioritaskan saat keduanya berbeda
    // jauh, dan hasil tepi hanya dipakai kalau gray tidak menghasilkan apa pun.
    if target_x < 0 && !cands.is_empty() {
        let gray = cands.iter().find(|c| c.m == "gray-ncc");
        let chosen = match gray {
            Some(g) if g.x >= 15 => g.clone(),
            _ => cands
                .iter()
                .max_by(|a, b| a.s.partial_cmp(&b.s).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap()
                .clone(),
        };
        target_x = chosen.x;
        score = chosen.s;
        method = chosen.m;
    }

    Ok(Gap {
        target_x,
        score,
        method,
        puzzle_offset_x: min_x,
        candidates: cands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_data_url() {
        assert!(decode_data_url("notadataurl").is_err());
    }

    #[test]
    fn decodes_plain_base64_payload() {
        // "hello"
        let url = "data:image/png;base64,aGVsbG8=";
        assert_eq!(decode_data_url(url).unwrap(), b"hello");
    }
}
