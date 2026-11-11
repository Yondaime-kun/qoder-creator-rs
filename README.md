# qoder-creator (Rust)

Port Rust dari [qoder-creator](https://github.com/wuzzstoreservice/qoder-creator).
Alur: email sementara → registrasi Qoder → CAPTCHA slider Aliyun → OTP →
verifikasi TMD → Personal Access Token → injeksi 9Router → login qoderwake.

Logika berat (NCC captcha, SQLite, HTTP, orkestrasi) ada di Rust. Browser
automation ada di `bridge.py` lewat protokol JSON-lines stdin/stdout, karena
Playwright tidak punya binding Rust yang setara.

## Arsitektur

```
qoder-creator (Rust binary)
├── tempmail.rs    klien JoyMail, filter domain lewat MX
├── captcha.rs     decode PNG, grayscale, masked NCC + Sobel, voting
├── twocaptcha.rs  solver cadangan (2captcha CoordinatesTask)
├── router.rs      SQLite 9Router (rusqlite, bundled)
├── bridge.rs      spawn + protokol JSON-lines ke bridge.py
└── bridge.py      Playwright: navigasi, isi form, drag slider, baca sesi
```

Satu proses bridge per akun, jadi sesi browser (cookie, localStorage) bertahan
dari registrasi sampai pembuatan PAT.

## Alur

```
[1]  buat alamat email          JoyMail, domain difilter lewat MX
[2]  nyalakan browser           Chromium (default) atau Camoufox
[3]  buka halaman sign-up
[4]  isi form registrasi
[5]  selesaikan CAPTCHA         NCC di Rust, drag di browser
[6]  tunggu form OTP
[7]  tunggu email verifikasi    polling inbox
[8]  submit OTP                 auto-submit, cek GET /api/v1/me
[9]  buat PAT                   dari sesi registrasi
[10] simpan cookie sesi
[11] login qoderwake via PAT
```

## Temuan yang menentukan

Semua poin di bawah adalah hasil pengukuran langsung, bukan asumsi.

### Panel CAPTCHA tidak terbuka lewat proxy

Gejalanya `#captcha-button` tinggi 0, `#captcha-element` kosong, dan
`ERR_TUNNEL_CONNECTION_FAILED` pada
`*.captcha-open-southeast.aliyuncs.com`. Yang menyesatkan: `curl` ke domain
yang sama **berhasil**. Jadi bukan proxy memblokir, melainkan proxy tidak
sanggup menyalurkan banyak koneksi paralel yang dibuka SDK captcha.

Solusinya melewatkan domain Aliyun dari proxy:

```
--proxy-bypass-list=*.aliyuncs.com;*.alicdn.com;*.alibaba.com;*.aliapp.org
```

Setelah itu tombol dirender ke dalam `#captcha-element`. Elemen `#captcha-button`
tetap kosong, jadi jangan dipakai sebagai target klik. Target sah:
`#aliyunCaptcha-captcha-left`.

### TMD anti-bot adalah penentu utama, bukan akurasi CAPTCHA

Bila IP proxy sudah ditandai, submit verifikasi diarahkan ke:

```
https://qoder.com//api/v1/users/_____tmd_____/punish?x5secdata=...
```

Sesi tidak pernah terbit dan PAT selalu 401. Penanda lolos: halaman berpindah
ke `/download`, lalu `GET /api/v1/me` menjawab 200.

Karena itu rotasi proxy adalah bagian dari alur. Dengan proxy datacenter
premium, tingkat keberhasilan sekitar 1 dari 8–9 percobaan.

### Halaman OTP tidak punya tombol submit

Halaman verifikasi hanya punya satu `input[maxlength="6"]` dan **nol** tombol.
Kode dikirim otomatis saat field terisi.

- Jangan tekan Enter: itu memulai navigasi yang tidak pernah selesai, dan
  setiap `page.evaluate` berikutnya ikut menunggu sampai bridge timeout.
- Setter native + `dispatchEvent` **tidak** memicu auto-submit.
- `Input.insertText` lewat CDP **memang** memicu auto-submit.

Setelah field terisi, jangan panggil `page.evaluate` lagi. Baca status lewat
`ctx.request` dan listener `page.on("request")` / `page.on("framenavigated")`.

### Penanda sesi

`GET /api/v1/userinfo` menjawab **404** walau sudah login. Penanda yang benar
adalah `GET /api/v1/me` → 200. Cookie sesi: `qoder_session_cookie` dan
`qoderuid` (total 18 cookie).

### Endpoint PAT butuh header same-origin

Tanpa `Sec-Fetch-Site: same-origin` server menjawab `CSRFInvalid` meski cookie
valid. PAT dibuat lewat `fetch` in-page (`credentials: 'include'`) atau
`ctx.request` dengan `X-CSRF-Token` dari cookie `qoder_csrf_token`.

### Domain email harus punya MX

`/api/domains` JoyMail bisa melaporkan domain mati sebagai aktif
(`ichimail.biz.id` tanpa MX). Email verifikasi tidak pernah sampai dan alur
berhenti di OTP. Setiap domain diperiksa MX-nya lewat DNS-over-HTTPS.

## Kenapa CAPTCHA dihitung di Rust

Versi Python asli menjalankan NCC di dalam `page.evaluate`. Di sini gambar
diambil sebagai data URL lalu diproses di Rust:

1. decode PNG → RGBA
2. bounding box dari kanal alpha potongan puzzle
3. grayscale luma (0.299R + 0.587G + 0.114B)
4. masked NCC pada domain grayscale **dan** domain tepi (Sobel)
5. voting: dua strategi sepakat dalam 15px → pakai rata-rata

Geometri yang terukur: `naturalW=296`, `imgW=300`, thumb 40px,
`bbox=(2,36) size=49x50`, rasio mouse→puzzle 0.432, offset maksimum 70px.

Yang dicocokkan NCC adalah tepi kiri potongan, sedangkan yang digeser adalah
elemen gambar puzzle, jadi `puzzle_offset_x` harus dikurangkan. Blur 3×3 harus
dihapus — blur merusak detail tepi dan membuat NCC memilih posisi salah.

## Build

Butuh Rust (stable), Python 3.12, dan Playwright + Chromium.

```bash
# 1. Playwright + Chromium (dipakai default)
/usr/bin/python3.12 -m venv ~/.camoufox-venv
~/.camoufox-venv/bin/pip install playwright camoufox
~/.camoufox-venv/bin/python -m playwright install chromium

# 2. Binary Rust
cargo build --release
```

## Pakai

```bash
export BRIDGE_PYTHON=~/.camoufox-venv/bin/python
export BRIDGE_SCRIPT=$PWD/bridge.py

./target/release/qoder-creator check      # cek JoyMail, browser, DB 9Router
./target/release/qoder-creator run 3      # buat 3 akun
./target/release/qoder-creator list       # lihat accounts.json
./target/release/qoder-creator mail       # domain aktif JoyMail
./target/release/qoder-creator            # menu interaktif
```

Rotasi proxy otomatis per akun:

```bash
# proxies/list.txt berisi satu proxy per baris: host:port:user:pass
export QODER_PROXY_FILE=$PWD/proxies/list.txt
./loop_pat.sh 40 1        # 40 percobaan, mulai dari baris 1
```

## Environment

| Variabel | Default | Fungsi |
|---|---|---|
| `TMAIL_URL` | `https://tmail.mhankbarbar.dev` | base URL JoyMail |
| `TMAIL_DOMAIN` | — | paksa satu domain, lewati cek MX |
| `BRIDGE_PYTHON` | `python3` | interpreter untuk bridge |
| `BRIDGE_SCRIPT` | cari otomatis | path `bridge.py` |
| `QODER_DATA_DIR` | CWD | lokasi `accounts.json` + `qoder_log.txt` |
| `QODER_PASSWORD` | `ChangeMe123!Xyz` | password akun baru |
| `HEADLESS` | `1` | `0` untuk melihat jendela browser |
| `QODER_ENGINE` | `chromium` | `chromium` atau `camoufox` |
| `CHROME_BIN` | cari otomatis | path binary Chromium |
| `QODER_PROXY` | — | `host:port:user:pass` |
| `QODER_PROXY_FILE` | — | file daftar proxy untuk rotasi |
| `QODER_PROXY_INDEX` | `1` | baris proxy yang dipakai |
| `CAPTCHA_ATTEMPTS` | `10` | percobaan maksimum per CAPTCHA |
| `OTP_TIMEOUT` | `420` | detik menunggu email verifikasi |
| `QODER_TZ` | — | timezone context browser |
| `QODER_COOKIE_PATH` | `/tmp/qoder_cookies.json` | tempat ekspor cookie |
| `QODERWAKE_PATH` | `~/.qoderwake/qoderwake` | binary qoderwake |

## Engine browser

Default **Chromium**, bukan Camoufox. Bukti empiris: pada proxy dan soal yang
sama, Chromium pernah lolos CAPTCHA dengan error 1.94px sementara Camoufox
selalu ditolak di posisi yang identik. Camoufox tetap tersedia lewat
`QODER_ENGINE=camoufox`.

UA harus cocok dengan binary (`Chrome/145`), karena client hints mengirim
versi binary. UA `Chrome/131` pada binary 145 membuat fingerprint tidak
konsisten.

## Tempmail: JoyMail

JoyMail tidak punya endpoint "buat inbox". Alamat dipilih sendiri pada domain
aktif, lalu dibaca lewat endpoint publik tanpa auth:

```
GET /api/domains          → daftar domain aktif
GET /api/inbox/{address}  → semua email + body lengkap
```

`text` dan `html` sudah ikut di respons inbox. Polling 4 detik.

## Status per komponen

| Komponen | Status | Bukti |
|---|---|---|
| Build release | ✅ | `cargo build --release` |
| Unit test (OTP, HTML, CAPTCHA decode, SQLite) | ✅ | 8/8 lulus |
| `check` — JoyMail + browser + DB | ✅ | dijalankan live |
| Panel CAPTCHA terbuka lewat proxy | ✅ | `#aliyunCaptcha-captcha-left` 34px |
| CAPTCHA solved | ✅ | error <2px, lolos verifikasi |
| Registrasi + OTP | ✅ | akun nyata terbuat |
| Verifikasi TMD → sesi terbit | ✅ | `GET /api/v1/me` = 200 |
| Pembuatan PAT | ✅ | token `pt-...` diterbitkan |
| Login qoderwake via PAT | ✅ | `[qoderwake] Login succeeded.` |
| Ekspor cookie sesi | ✅ | 18 cookie termasuk `qoder_session_cookie` |
| Injeksi 9Router | ⚠️ | butuh `~/.9router/db/data.sqlite` di host |
| Klaim Pro Trial | ❌ | lihat di bawah |

Verifikasi independen di luar browser (curl dengan cookie hasil ekspor):

```
GET /api/v1/me         → 200  {"name":"...","email":"..."}
GET /api/v1/me/userplan → 200  {"plan_tier":"PLAN_TIER_FREE", ...}
```

## Soal Pro Trial

Trial 14 hari + 300 credits **hanya** diberikan saat login pertama di Qoder
desktop client, dan **tidak tersedia di virtual machine**. Ini dinyatakan di
FAQ resmi Qoder, dan dikonfirmasi di mesin uji: `systemd-detect-virt` → `kvm`,
DMI `CVM` / `Tencent Cloud`.

Akun hasil registrasi web tetap `PLAN_TIER_FREE` di `/api/v1/me/userplan`,
walau PAT dan sesi valid. Tool ini **tidak** mengklaim trial; field
`trial_claimed` di `accounts.json` dibiarkan `false` dan dipisahkan dari
`qoderwake_login` supaya status tidak menyesatkan.

Binary qoderwake juga tidak punya perintah claim/coupon/redeem — hanya
`login`, `whoami`, `account profile|plan|usage|activity`.

## Catatan operasional

- `pkill -f <pola>` cocok dengan command line sendiri dan membunuh shell yang
  menjalankannya. Pakai `pkill -x <nama>` atau iterasi `pgrep` + `kill -9`.
- Proxy ini HTTP, bukan SOCKS (`socks5h://` membalas 000).
- Kredensial proxy harus dikirim sebagai `username`/`password` terpisah di
  dict proxy Playwright. URL ber-kredensial di field `server` ditolak Firefox
  dengan `NS_ERROR_PROXY_CONNECTION_REFUSED`.

## Lisensi

MIT, mengikuti proyek aslinya.
