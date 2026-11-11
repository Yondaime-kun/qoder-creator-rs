// Binary bantu untuk mengukur akurasi deteksi CAPTCHA pada gambar tersimpan.
// Pemakaian: captcha-probe <background.png> <puzzle.png>
use qoder_creator::captcha;

use base64::{engine::general_purpose::STANDARD, Engine};
use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("pakai: captcha-probe <bg.png> <puzzle.png>");
        std::process::exit(2);
    }

    let bg_raw = match fs::read(&args[0]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("baca {}: {}", args[0], e);
            std::process::exit(1);
        }
    };
    let pz_raw = match fs::read(&args[1]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("baca {}: {}", args[1], e);
            std::process::exit(1);
        }
    };

    let bg_url = format!("data:image/png;base64,{}", STANDARD.encode(&bg_raw));
    let pz_url = format!("data:image/png;base64,{}", STANDARD.encode(&pz_raw));

    match captcha::detect_gap(&bg_url, &pz_url) {
        Ok(gap) => {
            let adjusted = gap.target_x - gap.puzzle_offset_x;
            println!("target_x={}", gap.target_x);
            println!("puzzle_offset_x={}", gap.puzzle_offset_x);
            println!("adjusted={}", adjusted);
            println!("score={:.4}", gap.score);
            println!("method={}", gap.method);
            for c in &gap.candidates {
                println!("candidate {} x={} s={:.4}", c.m, c.x, c.s);
            }
            if let Some(exp) = args.get(2).and_then(|v| v.trim().parse::<i64>().ok()) {
                let err = (adjusted - exp).abs();
                println!("expected={} error={}px {}", exp, err,
                    if err <= 3 { "OK" } else { "MISS" });
            }
        }
        Err(e) => {
            eprintln!("detect_gap: {}", e);
            std::process::exit(1);
        }
    }
}
