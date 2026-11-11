use anyhow::Result;
use std::time::{Duration, Instant};

use crate::bridge::Bridge;
use crate::captcha;
use crate::config;
use crate::ui;

pub struct FlowOutcome {
    pub trial_claimed: bool,
    pub router_injected: bool,
}

/// Alur penuh satu akun: buat alamat -> registrasi (camoufox) -> OTP ->
/// PAT + injeksi 9Router -> klaim Pro Trial.
pub fn create_one() -> Result<crate::accounts::Account> {
    let password = config::default_password();
    let mut account = crate::accounts::Account {
        success: false,
        email: String::new(),
        password: password.clone(),
        otp: String::new(),
        pat: String::new(),
        trial_claimed: false,
        qoderwake_login: false,
        router_injected: false,
        error: String::new(),
        created_at: crate::accounts::now_iso(),
    };

    ui::pc("  [1] Membuat alamat email...", ui::CYAN);
    let mut email = match crate::tempmail::create_address() {
        Ok(e) => e,
        Err(e) => {
            ui::fail(&format!("Gagal membuat alamat: {}", e));
            account.error = format!("tempmail: {}", e);
            let _ = crate::accounts::save(&account);
            return Ok(account);
        }
    };
    account.email = email.clone();
    ui::ok(&format!("Email: {}", email));

    ui::pc("  [2] Menyalakan browser (camoufox)...", ui::CYAN);
    let mut bridge = match Bridge::spawn() {
        Ok(b) => b,
        Err(e) => {
            ui::fail(&format!("Bridge gagal: {}", e));
            account.error = format!("bridge: {}", e);
            let _ = crate::accounts::save(&account);
            return Ok(account);
        }
    };

    let headless = config::env_or("HEADLESS", "1") != "0";
    match bridge.start(headless) {
        Ok(ua) => ui::ok(&format!("Browser siap — UA: {}", truncate(&ua, 70))),
        Err(e) => {
            ui::fail(&format!("Browser gagal start: {}", e));
            account.error = format!("browser: {}", e);
            let _ = crate::accounts::save(&account);
            return Ok(account);
        }
    }

    ui::pc("  [3] Membuka halaman sign-up...", ui::CYAN);
    if let Err(e) = bridge.navigate(config::QODER_SIGNUP) {
        ui::fail(&format!("Navigasi gagal: {}", e));
        account.error = format!("navigate: {}", e);
        bridge.shutdown();
        let _ = crate::accounts::save(&account);
        return Ok(account);
    }

    ui::pc("  [4] Mengisi form registrasi...", ui::CYAN);
    match bridge.fill_registration(&email, &password) {
        Ok(name) => ui::ok(&format!("Identitas: {}", name)),
        Err(e) => {
            ui::fail(&format!("Form registrasi gagal: {}", e));
            account.error = format!("register: {}", e);
            bridge.shutdown();
            let _ = crate::accounts::save(&account);
            return Ok(account);
        }
    }

    ui::pc("  [5] Menyelesaikan CAPTCHA...", ui::CYAN);
    if !solve_captcha(&mut bridge, &mut email, &password, headless) {
        ui::fail("CAPTCHA gagal");
        account.error = "CAPTCHA gagal".into();
        bridge.shutdown();
        let _ = crate::accounts::save(&account);
        return Ok(account);
    }

    ui::pc("  [6] Menunggu form OTP...", ui::CYAN);
    match bridge.wait_otp_form() {
        Ok(true) => ui::ok("Form OTP siap"),
        Ok(false) => ui::warn("Form OTP tidak terdeteksi, lanjut saja"),
        Err(e) => ui::warn(&format!("Cek form OTP: {}", e)),
    }

    ui::pc("  [7] Menunggu email verifikasi...", ui::CYAN);
    let otp = match crate::tempmail::wait_for_otp(
        &email,
        Duration::from_secs(config::env_int("OTP_TIMEOUT", 420) as u64),
        Duration::from_secs(4),
    ) {
        Ok(code) => code,
        Err(e) => {
            ui::fail(&format!("{}", e));
            account.error = "OTP timeout".into();
            bridge.shutdown();
            let _ = crate::accounts::save(&account);
            return Ok(account);
        }
    };
    account.otp = otp.clone();
    ui::ok(&format!("OTP: {}", otp));

    ui::pc("  [8] Submit OTP...", ui::CYAN);
    // Verifikasi hanya lolos kalau TMD menerima submit. Bila IP proxy sudah
    // ditandai, halaman diarahkan ke /_____tmd_____/punish dan sesi tidak
    // pernah terbit; akun ini tidak bisa dipakai, jadi dihentikan di sini
    // daripada menghabiskan 4 percobaan PAT yang pasti 401.
    match bridge.submit_otp(&otp, &email, &password) {
        Ok(res) => {
            if let Some(st) = res.api_status {
                ui::info(&format!("GET /api/v1/me: {}", st));
            }
            if res.punished {
                ui::fail("TMD punish — IP proxy ditolak, akun dibuang");
                account.error = "tmd_punish".into();
                bridge.shutdown();
                let _ = crate::accounts::save(&account);
                return Ok(account);
            }
            if res.verified {
                ui::ok("Akun terverifikasi");
            } else {
                ui::warn("Sesi belum terbit, PAT kemungkinan 401");
            }
        }
        Err(e) => ui::warn(&format!("Submit OTP: {}", e)),
    }

    ui::pc("  [9] Membuat PAT dari sesi registrasi...", ui::CYAN);
    // PAT dibuat SEKARANG, saat sesi registrasi masih hidup. Sesi ini hilang
    // begitu proses berakhir, dan login ulang butuh OTP baru yang belum tentu
    // bisa dikirim (domain email sementara sering kena purge lebih dulu).
    let mut pat_token = String::new();
    // Sesi butuh waktu untuk benar-benar aktif setelah submit OTP; satu
    // percobaan langsung sering ditolak 401 walau akun sudah terverifikasi.
    let mut pat_result = None;
    for attempt in 1..=4 {
        match bridge.create_pat() {
            Ok(p) if !p.token.is_empty() => {
                pat_result = Some(p);
                break;
            }
            Ok(p) => {
                ui::warn(&format!(
                    "    PAT attempt {}/4 kosong (HTTP {})",
                    attempt, p.status
                ));
                pat_result = Some(p);
            }
            Err(e) => ui::warn(&format!("    PAT attempt {}/4: {}", attempt, e)),
        }
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    match pat_result {
        Some(pat) => {
            if pat.token.is_empty() {
                ui::fail(&format!(
                    "PAT kosong (HTTP {}, csrf:{})",
                    pat.status,
                    if pat.csrf_found { "ada" } else { "tidak ada" }
                ));
                ui::info(&truncate(&pat.body, 200));
            } else {
                ui::ok(&format!("PAT: {}...", truncate(&pat.token, 28)));
                pat_token = pat.token.clone();
                account.pat = pat.token.clone();
                match crate::router::inject(&email, &pat.token) {
                    Ok(injected) => account.router_injected = injected,
                    Err(e) => ui::fail(&format!("Injeksi 9Router: {}", e)),
                }
            }
        }
        None => ui::fail("Buat PAT: tidak ada respons"),
    }

    ui::pc("  [10] Verifikasi login (opsional)...", ui::CYAN);
    let signed_in = bridge.check_session().unwrap_or(false);
    if signed_in {
        ui::ok("Sesi aktif");
        // Simpan cookie untuk dipakai Qoder GUI. Trial Pro hanya diberikan
        // saat login di client, jadi sesi ini harus bisa dipindahkan ke sana.
        let path = std::env::var("QODER_COOKIE_PATH")
            .unwrap_or_else(|_| "/tmp/qoder_cookies.json".to_string());
        match bridge.export_cookies(&path) {
            Ok(n) => ui::ok(&format!("Cookie sesi disimpan ({} cookie)", n)),
            Err(e) => ui::warn(&format!("Ekspor cookie: {}", e)),
        }
    } else {
        ui::warn("Sesi tidak aktif, PAT tetap dipakai kalau sudah didapat");
    }
    let _ = signed_in;
    let _ = (&email, &password);

    bridge.shutdown();

    ui::pc("  [11] Login qoderwake via PAT...", ui::CYAN);
    // Yang diverifikasi di sini adalah autentikasi CLI (whoami sukses), bukan
    // aktivasi Pro Trial. Claim trial dilakukan lewat halaman pricing Qoder
    // dan tidak punya endpoint API publik, jadi tidak diklaim di sini.
    account.qoderwake_login = claim_trial(&pat_token);

    account.success = true;
    let _ = crate::accounts::save(&account);
    Ok(account)
}

/// Setengah lebar potongan puzzle (bbox alpha), dipakai untuk menggeser titik
/// tengah lubang ke posisi tepi kiri elemen yang digeser.
fn puzzle_half_width(png: &[u8]) -> i64 {
    let img = match image::load_from_memory(png) {
        Ok(i) => i.to_rgba8(),
        Err(_) => return 26,
    };
    let (w, h) = img.dimensions();
    let mut min_x = w;
    let mut max_x = 0u32;
    let mut found = false;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 128 {
                found = true;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
    }
    if !found {
        return 26;
    }
    (((min_x + max_x) / 2) as i64).max(1)
}

/// Loop CAPTCHA: ambil gambar lewat bridge, hitung gap di Rust, lalu drag.
/// Mode sesi saat mengulang CAPTCHA.
#[derive(Clone, Copy, PartialEq)]
enum SessionMode {
    /// Registrasi: alamat email baru + browser baru tiap percobaan. Aliyun
    /// menandai pasangan email+sesi yang gagal, jadi keduanya harus diganti.
    FreshAccount,
    /// Login: browser baru saja, alamat tetap. Akunnya sudah ada, jadi
    /// mengganti alamat justru menghilangkan akun yang mau dipakai.
    FreshBrowser,
    /// Tanpa restart: dipakai kalau sesi tidak boleh hilang sama sekali.
    KeepSession,
}

fn solve_captcha(
    bridge: &mut Bridge,
    email: &mut String,
    password: &str,
    headless: bool,
) -> bool {
    solve_captcha_mode(bridge, email, password, headless, SessionMode::FreshAccount, true)
}

fn solve_captcha_mode(
    bridge: &mut Bridge,
    email: &mut String,
    password: &str,
    headless: bool,
    mode: SessionMode,
    always_reload: bool,
) -> bool {
    let attempts = config::env_int("CAPTCHA_ATTEMPTS", 10);

    // Solver eksternal opsional. Kalau kuncinya ada, koordinat dari solver
    // dipakai ketika NCC lokal tidak yakin; kalau tidak ada, alur tetap jalan
    // dengan NCC saja.
    let tc2 = crate::twocaptcha::TwoCaptcha::from_env();
    if let Some(t) = tc2.as_ref() {
        match t.balance() {
            Ok(b) => ui::info(&format!("2captcha aktif, saldo ${:.2}", b)),
            Err(e) => ui::warn(&format!("2captcha saldo: {}", e)),
        }
    }

    for attempt in 1..=attempts {
        ui::pc(
            &format!("    ▸ CAPTCHA attempt {}/{}", attempt, attempts),
            ui::YELLOW,
        );

        // Percobaan pertama memakai sesi dan alamat yang sudah disiapkan
        // pemanggil. Percobaan berikutnya butuh sesi bersih: Aliyun menandai
        // kombinasi email + sesi browser yang gagal, jadi mengulang di sesi
        // yang sama selalu ditolak walau posisi drag sudah tepat.
        if attempt > 1 {
            if mode == SessionMode::KeepSession {
                // Login: sesi tidak boleh hilang, cukup minta soal baru.
                let _ = bridge.captcha_refresh(email, password);
                std::thread::sleep(Duration::from_millis(2000));
            } else {
            if mode == SessionMode::FreshAccount {
                match crate::tempmail::create_address() {
                    Ok(fresh) => {
                        *email = fresh;
                        ui::info(&format!("    Alamat baru: {}", email));
                    }
                    Err(e) => {
                        ui::warn(&format!("    Gagal buat alamat baru: {}", e));
                        return false;
                    }
                }
            }
            ui::info("    ↺ Sesi baru (reload) untuk CAPTCHA bersih");
            // Reload hanya untuk percobaan CAPTCHA PERTAMA. Setelah akun
            // terverifikasi, sesi harus dipertahankan supaya PAT bisa dibuat —
            // reload di titik itu membuang cookie yang baru diterbitkan.
            if always_reload {
                let filled = bridge
                    .navigate(crate::config::QODER_SIGNUP)
                    .and_then(|_| {
                        std::thread::sleep(Duration::from_millis(2000));
                        bridge.fill_registration(email, password)
                    });
                if let Err(e) = filled {
                    ui::warn(&format!("    Siapkan sesi gagal: {}", e));
                    continue;
                }
            } else {
                // Sudah terverifikasi: minta soal baru di sesi yang sama.
                let _ = bridge.captcha_refresh(email, password);
            }
            std::thread::sleep(Duration::from_millis(1500));
            }
        }

        let shot = match bridge.captcha_geometry() {
            Ok(Some(s)) => s,
            Ok(None) => {
                ui::warn("Gambar CAPTCHA tidak siap, minta soal baru");
                match bridge.captcha_refresh(email, password) {
                    Ok(true) => ui::info("Soal baru dimuat"),
                    Ok(false) => {
                        ui::warn("Refresh tidak menghasilkan gambar");
                        std::thread::sleep(Duration::from_millis(1500));
                    }
                    Err(e) => ui::warn(&format!("Refresh gagal: {}", e)),
                }
                continue;
            }
            Err(e) => {
                ui::warn(&format!("Ambil gambar: {}", e));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        // Target gap: 2captcha kalau tersedia, NCC lokal sebagai fallback.
        // Solver manusia lebih akurat pada latar bertekstur yang membingungkan
        // NCC, tapi tiap permintaan berbayar — jadi hanya dipakai ketika
        // kuncinya diset dan NCC tidak yakin.
        let mut target_x = 0i64;
        let mut offset_x = 0i64;
        let mut method = String::new();
        let mut score = 0.0f64;

        match captcha::detect_gap(&shot.bg, &shot.puzzle) {
            Ok(g) => {
                target_x = g.target_x;
                offset_x = g.puzzle_offset_x;
                method = g.method.clone();
                score = g.score;
            }
            Err(e) => ui::warn(&format!("Deteksi gap: {}", e)),
        }

        if let Some(tc) = tc2.as_ref() {
            // `CAPTCHA_SOLVER=always` memaksa setiap soal diselesaikan lewat
            // 2captcha. Berguna untuk menguji apakah penolakan berasal dari
            // akurasi posisi: kalau solver manusia pun ditolak, berarti yang
            // dinilai server bukan posisi.
            let force = std::env::var("CAPTCHA_SOLVER")
                .map(|v| v.trim().eq_ignore_ascii_case("always"))
                .unwrap_or(false);
            let need_solver = force || score < 0.55 || target_x < 15;
            if need_solver {
                ui::info("    → 2captcha: minta koordinat");
                match (shot.bg_bytes(), shot.puzzle_bytes()) {
                    (Ok(bg_png), Ok(pz_png)) => {
                        // Lebar potongan puzzle menentukan jarak antara titik
                        // yang ditunjuk solver (tengah lubang) dan tepi kiri
                        // elemen yang digeser.
                        let pz_center = puzzle_half_width(&pz_png);
                        match tc.solve_slider(&bg_png, &pz_png) {
                            Ok(x) => {
                                // Solver menunjuk tengah lubang, sedangkan yang
                                // digeser adalah tepi kiri elemen puzzle. Kurangi
                                // setengah lebar potongan supaya tepi kirinya
                                // yang jatuh di tengah lubang.
                                target_x = (x - pz_center).max(0);
                                offset_x = 0;
                                method = "2captcha".into();
                                score = 1.0;
                            }
                            Err(e) => ui::warn(&format!("    2captcha gagal: {}", e)),
                        }
                    }
                    _ => ui::warn("    decode gambar gagal"),
                }
            }
        }

        if target_x < 15 {
            ui::warn("Gap tidak terdeteksi");
            continue;
        }

        let natural = shot
            .geometry
            .get("naturalW")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let display = shot
            .geometry
            .get("displayW")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let scale = if natural > 0.0 && display > 0.0 {
            display / natural
        } else {
            1.0
        };
        // NCC mengembalikan posisi tepi kiri potongan di dalam gambar latar,
        // sedangkan elemen yang digeser adalah gambar puzzle (yang punya
        // offset sendiri). Kurangkan offset itu sebelum konversi.
        let adjusted = (target_x - offset_x) as f64;
        let target_display = adjusted * scale;
        if target_display < 10.0 {
            ui::warn(&format!(
                "Target terlalu dekat: {:.0}px (raw {} offset {})",
                target_display, target_x, offset_x
            ));
            continue;
        }

        // Skor NCC rendah berarti pencocokan ambigu (tekstur latar mirip di
        // banyak tempat). Geser satu piksel bisa jatuh ke posisi salah, jadi
        // minta soal lain daripada mempertaruhkan percobaan.
        let min_score = std::env::var("CAPTCHA_MIN_SCORE")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.35);
        if score < min_score {
            ui::warn(&format!(
                "Skor NCC rendah ({:.3} < {:.2}), ganti soal",
                score, min_score
            ));
            let _ = bridge.captcha_refresh(email, password);
            continue;
        }

        ui::pc(
            &format!(
                "    Target={}px offset={} → {:.1}px display ({}, score={:.4})",
                target_x, offset_x, target_display, method, score
            ),
            ui::CYAN,
        );

        match bridge.captcha_drag(target_display, shot.geometry.clone()) {
            Ok(res) => {
                if res.solved {
                    ui::ok("CAPTCHA solved!");
                    return true;
                }

                // Target di luar jangkauan slider: soal ini mustahil
                // diselesaikan, langsung minta soal baru tanpa membuang sisa
                // percobaan pada posisi yang tidak mungkin dicapai.
                if res.impossible {
                    ui::warn(&format!(
                        "Target {:.0}px di luar jangkauan (maks {:.0}px), ganti soal",
                        res.target, res.max_offset
                    ));
                    let _ = bridge.captcha_refresh(email, password);
                    continue;
                }

                // Posisi sudah sejajar tapi server belum menerima: coba lepas
                // dan geser sekali lagi di posisi yang sama sebelum membuang
                // soal ini. Sering kali percobaan kedua lolos karena gerakan
                // pertama tercatat sebagai "belum selesai".
                if res.aligned && attempt < attempts {
                    ui::pc("    ↻ Posisi pas, ulangi drag di soal yang sama", ui::YELLOW);
                    std::thread::sleep(Duration::from_millis(1200));

                    if let Ok(Some(again)) = bridge.captcha_geometry() {
                        let scale2 = {
                            let n = again
                                .geometry
                                .get("naturalW")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let d = again
                                .geometry
                                .get("displayW")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            if n > 0.0 && d > 0.0 {
                                d / n
                            } else {
                                1.0
                            }
                        };
                        if let Ok(g2) = captcha::detect_gap(&again.bg, &again.puzzle) {
                            let adj = (g2.target_x - g2.puzzle_offset_x) as f64;
                            if let Ok(r2) = bridge.captcha_drag(adj * scale2, again.geometry) {
                                if r2.solved {
                                    ui::ok("CAPTCHA solved (percobaan ulang)!");
                                    return true;
                                }
                            }
                        }
                    }
                }

                if res.aligned {
                    ui::pc("    ✗ Posisi pas tapi ditolak, ganti soal", ui::YELLOW);
                } else {
                    ui::pc("    ✗ Posisi salah, ganti soal", ui::YELLOW);
                }
            }
            Err(e) => ui::warn(&format!("Drag gagal: {}", e)),
        }
    }
    false
}

/// Klaim Pro Trial lewat qoderwake.
///
/// `qoderwake login --method file --token-file <pat>` menerima Personal Access
/// Token sebagai kredensial, jadi PAT yang dibuat di langkah sebelumnya
/// langsung dipakai untuk mengautentikasi CLI — tidak perlu login browser
/// manual. Setelah login, `account` menampilkan plan/kuota akun.
fn claim_trial(pat: &str) -> bool {
    let bin = config::qoderwake_path();
    if !std::path::Path::new(&bin).exists() {
        ui::warn(&format!("qoderwake tidak ditemukan ({})", bin));
        return false;
    }

    if pat.is_empty() {
        ui::warn("PAT kosong, tidak bisa login qoderwake");
        return false;
    }

    // Selalu login dengan PAT akun ini. `whoami` bisa melaporkan sesi akun
    // sebelumnya yang masih tersimpan, jadi melewati login ketika "sudah
    // login" membuat plan yang dibaca milik akun lain — plan akun baru tidak
    // pernah terlihat.
    //
    // Logout dulu: `login` menolak mengganti sesi yang sudah ada
    // ("Already logged in"), sehingga PAT akun baru tidak pernah terpakai.
    let _ = run_with_timeout(&bin, &["logout"], Duration::from_secs(45));
    std::thread::sleep(Duration::from_millis(800));

    let token_file = std::env::temp_dir().join(format!("qoder-pat-{}.txt", crate::accounts::new_uuid()));
    if let Err(e) = write_private(&token_file, pat) {
        ui::warn(&format!("tulis token file: {}", e));
        return false;
    }

    ui::info("[*] Login qoderwake via PAT...");
    let login = run_with_timeout(
        &bin,
        &["login", "--method", "file", "--token-file", token_file.to_string_lossy().as_ref()],
        Duration::from_secs(120),
    );
    let _ = std::fs::remove_file(&token_file);

    match login {
        Ok(out) => {
            let t = out.trim();
            if !t.is_empty() {
                ui::info(&truncate(t, 300));
            }
        }
        Err(e) => {
            ui::warn(&format!("qoderwake login: {}", e));
            return false;
        }
    }

    // Verifikasi hasil login.
    match run_with_timeout(&bin, &["whoami"], Duration::from_secs(20)) {
        Ok(out) => {
            let s = out.trim().to_string();
            if s.is_empty() || s.contains("Not logged in") {
                ui::warn(&format!("qoderwake belum login: {}", s));
                return false;
            }
            ui::ok(&format!("qoderwake login OK: {}", s));
        }
        Err(e) => {
            ui::warn(&format!("whoami: {}", e));
            return false;
        }
    }

    // Klaim Pro Trial lalu baca plan yang benar-benar aktif. Login saja bukan
    // bukti trial didapat — plan harus berubah jadi Pro/trial.
    ui::info("[*] Klaim Pro Trial...");
    // `account plan` butuh daemon lokal; tanpa itu perintahnya hanya
    // mengembalikan pesan "Daemon is not running" dan plan tidak terbaca.
    let _ = run_with_timeout(&bin, &["start"], Duration::from_secs(90));
    if let Ok(out) = run_with_timeout(&bin, &["account", "plan"], Duration::from_secs(45)) {
        let s = out.trim();
        if !s.is_empty() {
            ui::info(&truncate(s, 500));
        }
        let low = s.to_lowercase();
        if low.contains("trial") || low.contains("pro") {
            ui::ok("Plan Pro/trial aktif");
            return true;
        }
    }

    ui::warn("Plan Pro/trial belum aktif");
    false
}

/// Tulis file dengan permission 0600 (kredensial tidak boleh world-readable).
fn write_private(path: &std::path::Path, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()
}

fn run_with_timeout(bin: &str, args: &[&str], timeout: Duration) -> Result<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn {}: {}", bin, e))?;

    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(_) => break,
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow::anyhow!("timeout"));
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        }
    }

    let out = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
