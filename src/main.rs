use qoder_creator::{accounts, bridge, config, flow, router, tempmail, ui};

use std::io::{self, BufRead, Write};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ui::init_log();

    // Proxy opsional diteruskan ke bridge lewat environment. Rotasi dilakukan
    // dengan menggeser urutan file berdasarkan nomor akun (QODER_PROXY_FILE).
    if let Ok(file) = std::env::var("QODER_PROXY_FILE") {
        if !file.trim().is_empty() {
            let idx = std::env::var("QODER_PROXY_INDEX")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if let Ok(content) = std::fs::read_to_string(file.trim()) {
                let lines: Vec<&str> = content
                    .lines()
                    .map(|l| l.split("=>").next().unwrap_or(l).trim())
                    .filter(|l| !l.is_empty())
                    .collect();
                if !lines.is_empty() {
                    let pick = lines[idx % lines.len()];
                    // Format file: host:port:user:pass
                    std::env::set_var("QODER_PROXY", pick);
                    ui::info(&format!("Proxy #{}: {}", idx + 1, pick.split(':').next().unwrap_or("?")));
                }
            }
        }
    }

    if args.is_empty() {
        banner_and_menu();
        return;
    }

    match args[0].as_str() {
        "-h" | "--help" => print_help(),
        "-V" | "--version" => println!("qoder-creator {}", env!("CARGO_PKG_VERSION")),
        "check" => cmd_check(),
        "list" => cmd_list(),
        "mail" => cmd_mail(),
        "run" => {
            let count = args
                .get(1)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1);
            cmd_run(count);
        }
        "menu" => banner_and_menu(),
        other => {
            eprintln!("perintah tidak dikenal: {}", other);
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "qoder-creator {}\n\n\
         Pemakaian:\n  \
         qoder-creator                  menu interaktif\n  \
         qoder-creator run [N]          buat N akun (default 1) non-interaktif\n  \
         qoder-creator check            cek dependency (tempmail, browser, 9router)\n  \
         qoder-creator list             tampilkan accounts.json\n  \
         qoder-creator mail             tampilkan domain aktif JoyMail\n\n\
         Environment:\n  \
         TMAIL_URL          base URL JoyMail (default https://tmail.mhankbarbar.dev)\n  \
         BRIDGE_SCRIPT      path bridge.py\n  \
         BRIDGE_PYTHON      interpreter untuk bridge (default python3)\n  \
         QODER_DATA_DIR     direktori accounts.json & log\n  \
         QODER_PASSWORD     password akun baru\n  \
         HEADLESS           0 untuk menampilkan jendela browser\n  \
         CAPTCHA_ATTEMPTS   jumlah percobaan CAPTCHA (default 10)",
        env!("CARGO_PKG_VERSION")
    );
}

fn banner_and_menu() {
    ui::banner();

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    loop {
        println!(
            "{}\n╔═══════════════════════════════════════════════════════╗\n\
             ║  QODER CREATOR MENU                                   ║\n\
             ╠═══════════════════════════════════════════════════════╣\n\
             ║  1.  Register + Claim + Inject (single)               ║\n\
             ║  2.  Register + Claim + Inject (batch)                ║\n\
             ║  3.  Lihat accounts.json                              ║\n\
             ║  4.  Cek environment                                  ║\n\
             ║  0.  Keluar                                           ║\n\
             ╚═══════════════════════════════════════════════════════╝{}",
            ui::WHITE,
            ui::RESET
        );
        print!("{}Pilih (0-4): {}", ui::YELLOW, ui::RESET);
        let _ = io::stdout().flush();

        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }

        match line.trim() {
            "0" => {
                ui::ok("Bye!");
                return;
            }
            "1" => {
                ui::pc(&("\n".to_string() + &"─".repeat(50)), ui::CYAN);
                match flow::create_one() {
                    Ok(a) => report(&a),
                    Err(e) => ui::fail(&format!("{}", e)),
                }
            }
            "2" => {
                print!("  Berapa akun? ");
                let _ = io::stdout().flush();
                let mut n = String::new();
                let _ = reader.read_line(&mut n);
                let count = n.trim().parse::<usize>().unwrap_or(1).max(1);
                cmd_run(count);
            }
            "3" => cmd_list(),
            "4" => cmd_check(),
            _ => ui::fail("Pilihan tidak valid"),
        }
    }
}

fn cmd_run(count: usize) {
    ui::pc(&format!("\n  Target: {} akun", count), ui::CYAN);
    let mut success = 0usize;

    for i in 0..count {
        ui::pc(&"─".repeat(50), ui::CYAN);
        ui::pc(&format!("  Account {}/{}", i + 1, count), ui::BOLD);

        match flow::create_one() {
            Ok(a) => {
                if a.success {
                    success += 1;
                }
                report(&a);
            }
            Err(e) => ui::fail(&format!("{}", e)),
        }

        // Rotasi proxy untuk akun berikutnya.
        if std::env::var("QODER_PROXY_FILE").is_ok() {
            let cur = std::env::var("QODER_PROXY_INDEX")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            std::env::set_var("QODER_PROXY_INDEX", (cur + 1).to_string());
        }

        if i + 1 < count {
            let delay = 5 + (rand::random::<u64>() % 10);
            ui::info(&format!("⏳ Tunggu {}s...", delay));
            std::thread::sleep(Duration::from_secs(delay));
        }
    }

    ui::pc(&"═".repeat(50), ui::CYAN);
    let color = if success > 0 { ui::GREEN } else { ui::RED };
    ui::pc(&format!("  {}/{} berhasil", success, count), color);
}

fn report(a: &accounts::Account) {
    if a.success {
        ui::ok(&format!("Berhasil: {}", a.email));
        if a.qoderwake_login {
            ui::pc("     qoderwake login: ✅", ui::GREEN);
        }
        if a.trial_claimed {
            ui::pc("     Pro Trial: ✅", ui::GREEN);
        }
        if a.router_injected {
            ui::pc("     9Router: ✅", ui::GREEN);
        }
        return;
    }
    ui::fail(&format!("Gagal: {}", a.error));
}

fn cmd_list() {
    match accounts::load() {
        Ok(list) if list.is_empty() => ui::warn("Belum ada akun"),
        Ok(list) => {
            ui::pc(&format!("\n  📋 {} akun:", list.len()), ui::CYAN);
            for a in list {
                let mark = if a.success { "✅" } else { "❌" };
                ui::pc(
                    &format!(
                        "    {} {} | waker:{} | trial:{} | router:{}",
                        mark, a.email, a.qoderwake_login, a.trial_claimed, a.router_injected
                    ),
                    ui::WHITE,
                );
            }
        }
        Err(e) => ui::fail(&format!("Baca accounts.json: {}", e)),
    }
}

fn cmd_mail() {
    match tempmail::domains() {
        Ok(d) => {
            ui::ok(&format!("{} domain aktif", d.len()));
            for dom in d {
                ui::info(&dom);
            }
        }
        Err(e) => ui::fail(&format!("{}", e)),
    }
}

fn cmd_check() {
    ui::banner();
    let mut bad = false;

    match tempmail::domains() {
        Ok(d) => ui::ok(&format!("JoyMail OK — {} domain aktif ({})", d.len(), d[0])),
        Err(e) => {
            ui::fail(&format!("JoyMail: {}", e));
            bad = true;
        }
    }

    match std::env::var("BRIDGE_SCRIPT").ok().map(std::path::PathBuf::from) {
        Some(p) if p.exists() => ui::ok(&format!("Bridge script: {}", p.display())),
        _ => ui::info("BRIDGE_SCRIPT tidak diset, dicari otomatis di sebelah binary"),
    }

    match bridge::Bridge::spawn() {
        Ok(mut b) => {
            let headless = config::env_or("HEADLESS", "1") != "0";
            match b.start(headless) {
                Ok(ua) => {
                    ui::ok(&format!("Camoufox siap — UA: {}", &ua[..ua.len().min(80)]));
                    b.shutdown();
                }
                Err(e) => {
                    ui::fail(&format!("Camoufox: {}", e));
                    bad = true;
                }
            }
        }
        Err(e) => {
            ui::fail(&format!("Bridge: {}", e));
            bad = true;
        }
    }

    let db = config::router_db_path();
    if db.exists() {
        if router::schema_ok(&db) {
            ui::ok(&format!("9Router DB OK: {}", db.display()));
        } else {
            ui::warn("DB 9Router ada tapi tabel providerConnections tidak ditemukan");
        }
    } else {
        ui::warn(&format!("DB 9Router tidak ada: {}", db.display()));
    }

    if bad {
        std::process::exit(1);
    }
    ui::ok("Semua komponen siap");
}
