#!/usr/bin/env python3
"""
Browser bridge untuk qoder-creator (Rust).

Rust memegang logika bisnis: tempmail, ekstraksi OTP, matematika CAPTCHA,
SQLite 9Router, CLI. Proses ini hanya mengeksekusi aksi browser.

Protokol: satu proses persisten, satu JSON per baris di stdin, satu JSON
per baris di stdout. Sesi browser hidup selama proses berjalan sehingga
beberapa command berurutan memakai halaman yang sama.

    {"action":"navigate","url":"..."}  ->  {"ok":true,"url":"..."}
"""

import json
import os
import random
import re
import sys

import traceback

from camoufox.sync_api import Camoufox

QODER_PAT_API = "https://qoder.com/api/v1/me/personal-access-tokens"
QODER_USERS_API = "https://qoder.com/api/v1/users"

FIRST = ["James", "Mary", "John", "Emma", "Robert", "Olivia", "Michael", "Sophia",
         "William", "Ava", "David", "Isabella", "Richard", "Mia", "Joseph", "Charlotte",
         "Thomas", "Amelia", "Daniel", "Evelyn", "Matthew", "Abigail", "Anthony", "Emily"]
LAST = ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
        "Rodriguez", "Martinez", "Wilson", "Anderson", "Taylor", "Moore", "Jackson",
        "Martin", "Lee", "Thompson", "White", "Harris", "Clark", "Lewis", "Walker"]

_ctx = {}


def emit(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def log(msg):
    sys.stderr.write(str(msg) + "\n")
    sys.stderr.flush()


def get_page():
    page = _ctx.get("page")
    if page is None:
        raise RuntimeError("browser belum di-start (kirim action 'start' dulu)")
    return page


def start_browser(headless=True):
    if _ctx.get("page") is not None:
        return _ctx["page"]

    engine = os.environ.get("QODER_ENGINE", "chromium").strip().lower()
    proxy = _proxy_config()

    if engine == "chromium":
        return _start_chromium(headless, proxy)

    return _start_camoufox(headless, proxy)


def _proxy_config():
    """Bentuk dict proxy Playwright dari QODER_PROXY (host:port:user:pass).

    Kredensial harus dikirim sebagai username/password terpisah, bukan
    digabung ke URL server — Firefox menolak URL berisi kredensial dengan
    NS_ERROR_PROXY_CONNECTION_REFUSED.
    """
    proxy_url = os.environ.get("QODER_PROXY", "").strip()
    if not proxy_url:
        return None
    parts = proxy_url.split(":")
    if len(parts) >= 4:
        host, port = parts[0], parts[1]
        user, pw = ":".join(parts[2:-1]), parts[-1]
        return {"server": f"http://{host}:{port}",
                "username": user, "password": pw}
    if len(parts) == 3:
        return {"server": f"http://{parts[0]}:{parts[1]}",
                "username": parts[2], "password": ""}
    return {"server": proxy_url}


def _start_chromium(headless, proxy):
    """Chromium + UA Windows + automation flag dimatikan.

    Aliyun menilai environment, bukan cuma posisi puzzle: Camoufox (Firefox
    build) ditolak konsisten walau drag aligned <2px, sementara Chromium
    dengan profil ini lolos. UA harus menyebut versi yang sama dengan binary
    yang dipakai, kalau tidak client hints (sec-ch-ua) jadi mismatch.
    """
    from playwright.sync_api import sync_playwright

    pw = sync_playwright().start()
    launch = {
        "headless": bool(headless),
        "args": ["--no-sandbox", "--disable-gpu",
                 "--disable-blink-features=AutomationControlled",
                 # Domain Aliyun (captcha + CDN) harus dilewatkan proxy.
                 # Proxy datacenter gagal menyalurkan banyak koneksi paralel ke
                 # domain itu, sehingga SDK captcha tidak bisa merender tombol
                 # dan panel tidak pernah terbuka. Domain tersebut dapat
                 # diakses langsung, jadi cukup di-bypass.
                 "--proxy-bypass-list=" + ";".join([
                     "*.aliyuncs.com", "*.alicdn.com", "*.alibaba.com",
                     "*.aliapp.org", "*.google-analytics.com",
                     "*.googletagmanager.com", "*.doubleclick.net",
                     "*.google.com", "*.clarity.ms", "*.bing.net",
                 ])],
    }
    exe = os.environ.get("CHROME_BIN", "").strip()
    if exe:
        launch["executable_path"] = exe
    if proxy:
        launch["proxy"] = proxy

    browser = pw.chromium.launch(**launch)
    ctx = browser.new_context(
        user_agent=os.environ.get(
            "QODER_UA",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36"),
        viewport={"width": 1280, "height": 720},
        locale="en-US",
        # Timezone tetap Vienna untuk semua proxy, sama seperti runner yang
        # terbukti lolos. Menyamakan timezone dengan lokasi IP justru gagal:
        # nilai ini bagian dari fingerprint yang dinilai, dan kombinasi
        # UA Windows + locale en-US + Vienna adalah profil yang diterima.
        timezone_id=os.environ.get("QODER_TZ", "Europe/Vienna"),
    )
    ctx.add_init_script(
        "Object.defineProperty(navigator,'webdriver',{get:()=>false})")

    page = ctx.new_page()
    page.set_default_timeout(25000)
    page.set_default_navigation_timeout(60000)

    _ctx["pw"] = pw
    _ctx["browser"] = browser
    _ctx["ctx"] = ctx
    _ctx["page"] = page
    return page


def _start_camoufox(headless, proxy):
    kwargs = {
        "headless": bool(headless),
        "humanize": True,
        "os": ["windows"],
        "locale": ["en-US", "en"],
        # Canvas readback acak bikin browser terbaca sebagai Firefox yang
        # di-hardening (LibreWolf/Tor), bukan Firefox biasa — itu sendiri
        # sebuah fingerprint tell. Matikan.
        "canvas_noise": False,
        # Fingerprint harus di-inject di main world. Kalau hanya di isolated
        # world, halaman bisa mendeteksi bahwa nilai navigator/screen tidak
        # berasal dari world yang sama dengan kode deteksinya.
        "main_world_eval": True,
        # GeoIP memaksa timezone/locale mengikuti IP. Kalau IP dan fingerprint
        # OS tidak cocok (mis. IP Asia + profil Windows), kombinasi itu sendiri
        # menjadi sinyal anomali. Biarkan konsisten dengan profil.
        "geoip": False,
        "i_know_what_im_doing": True,
    }

    if proxy:
        kwargs["proxy"] = proxy

    try:
        cm = Camoufox(**kwargs)
    except Exception as e:
        log(f"start dengan geoip gagal ({e}), ulang tanpa geoip")
        kwargs.pop("geoip", None)
        cm = Camoufox(**kwargs)

    browser = cm.__enter__()
    _ctx["cm"] = cm
    _ctx["browser"] = browser
    page = browser.new_page()
    # Batas waktu default untuk semua operasi Playwright. Tanpa ini,
    # page.evaluate / wait_for_function bisa menggantung tanpa akhir ketika
    # halaman memuat resource yang tidak pernah selesai.
    page.set_default_timeout(25000)
    page.set_default_navigation_timeout(60000)
    _ctx["page"] = page
    return page


def stop_browser():
    browser = _ctx.get("browser")
    pw = _ctx.get("pw")
    if browser is not None:
        try:
            browser.close()
        except Exception as e:
            log(f"close error: {e}")
    if pw is not None:
        try:
            pw.stop()
        except Exception as e:
            log(f"pw stop error: {e}")
    cm = _ctx.get("cm")
    if cm is not None:
        try:
            cm.__exit__(None, None, None)
        except Exception as e:
            log(f"close error: {e}")
    _ctx.clear()


# ------------------------------------------------------------------ actions

def act_start(args):
    page = start_browser(args.get("headless", True))
    ua = page.evaluate("() => navigator.userAgent")
    return {"userAgent": ua}


def act_navigate(args):
    page = get_page()
    page.goto(args["url"], wait_until="domcontentloaded",
              timeout=int(args.get("timeout", 60000)))
    return {"url": page.url}


def act_fill_registration(args):
    page = get_page()
    first = random.choice(FIRST)
    last = random.choice(LAST)

    page.locator("#basic_firstName").wait_for(state="visible", timeout=30000)
    page.locator("#basic_firstName").fill(first)
    page.locator("#basic_lastName").fill(last)
    page.locator("#basic_email").fill(args["email"])

    page.evaluate("""() => {
        const cb = document.querySelector('input[type="checkbox"]');
        if (cb && !cb.checked) {
            cb.click();
            if (!cb.checked) { const l = cb.closest('label'); if (l) l.click(); }
        }
        return cb ? cb.checked : false;
    }""")
    page.wait_for_timeout(500)

    page.get_by_role("button", name="Continue").first.click()
    page.wait_for_timeout(2000)

    page.locator("#basic_password").wait_for(state="visible", timeout=20000)
    page.locator("#basic_password").fill(args["password"])
    page.wait_for_timeout(500)
    page.get_by_role("button", name="Continue").first.click()
    page.wait_for_timeout(2000)

    return {"name": f"{first} {last}", "url": page.url}


def _data_url(page, sel):
    """Ambil src gambar CAPTCHA sebagai data URL.

    Gambar bisa berupa data URL (soal pertama) atau URL http ke CDN Aliyun
    (soal setelah refresh). Untuk yang http, pengambilan dilakukan lewat
    page.request supaya tidak kena batasan CORS halaman.
    """
    src = page.evaluate("""(sel) => {
        const el = document.querySelector(sel);
        return el ? (el.src || '') : '';
    }""", sel)

    if not src:
        return ""

    if src.startswith("data:"):
        return src

    # URL http(s): ambil byte-nya lewat konteks request Playwright.
    try:
        resp = page.request.get(src, timeout=20000)
        if not resp.ok:
            return ""
        body = resp.body()
        ctype = resp.headers.get("content-type", "image/png").split(";")[0]
        import base64 as _b64
        return f"data:{ctype};base64,{_b64.b64encode(body).decode()}"
    except Exception:
        return ""


def _reopen_captcha(page):
    """Buka ulang panel CAPTCHA dari state 'Click to verify'.

    Setelah percobaan drag yang gagal, widget Aliyun runtuh kembali ke
    tombol awal dan gambar tidak dimuat. Menekan tombol refresh saja tidak
    cukup; panel harus dibuka lagi lewat tombol verifikasi.
    """
    opened = page.evaluate("""() => {
        // 1. Panel sudah terbuka?
        const s = document.querySelector('#aliyunCaptcha-sliding-slider');
        if (s && s.getBoundingClientRect().width > 0) return 'already';

        // 2. Tombol awal "Click to verify".
        for (const id of ['captcha-button', 'aliyunCaptcha-captcha-left', 'aliyunCaptcha-start-icon']) {
            const el = document.getElementById(id);
            if (el) {
                const r = el.getBoundingClientRect();
                if (r.width > 0 && r.height > 0) { el.click(); return id; }
            }
        }

        // 3. Fallback: cari elemen yang teksnya "Click to verify".
        const all = Array.from(document.querySelectorAll('div, span, button'));
        for (const el of all) {
            const t = (el.innerText || '').trim();
            if (/^click to verify$/i.test(t)) {
                const r = el.getBoundingClientRect();
                if (r.width > 0) { el.click(); return 'text'; }
            }
        }
        return '';
    }""")
    return opened


def _open_panel(page, tries=5):
    """Buka panel CAPTCHA lewat klik mouse di koordinat tombol.

    Klik lewat el.click() di dalam page.evaluate bisa menggantung karena
    handler Aliyun memuat SDK secara sinkron di event yang sama. Klik mouse
    mengirim input asli ke browser sehingga evaluate langsung kembali.
    """
    for _ in range(tries):
        box = page.evaluate("""() => {
            const vis = el => !!(el && el.getBoundingClientRect().width > 0
                                 && el.getBoundingClientRect().height > 0);
            const sl = document.querySelector('#aliyunCaptcha-sliding-slider');
            if (vis(sl)) return {state: 'open'};
            // Tombol verifikasi dirender SDK ke dalam #captcha-element
            // (#captcha-button sendiri tetap kosong), jadi yang dicari adalah
            // elemen Aliyun yang benar-benar punya tinggi.
            for (const id of ['aliyunCaptcha-captcha-left',
                              'aliyunCaptcha-captcha-wrapper',
                              'aliyunCaptcha-captcha-body',
                              'aliyunCaptcha-start-icon',
                              'captcha-button']) {
                const el = document.getElementById(id);
                if (el && vis(el)) {
                    const r = el.getBoundingClientRect();
                    return {state: 'click', x: r.x + r.width / 2, y: r.y + r.height / 2, id};
                }
            }
            // #captcha-button sering berukuran height 0 (wadah kosong), sehingga
            // klik di koordinatnya tidak mengenai apa pun. Elemen pembungkus
            // berkelas loginVerify--* adalah kotak yang benar-benar terlihat
            // dan menerima klik.
            for (const el of document.querySelectorAll('div[class*="loginVerify"]')) {
                const r = el.getBoundingClientRect();
                if (r.width > 40 && r.height > 20) {
                    return {state: 'click', x: r.x + r.width / 2,
                            y: r.y + r.height / 2, id: 'loginVerify'};
                }
            }
            return {state: 'wait'};
        }""")

        if not box:
            return False
        if box.get("state") == "open":
            return True
        if box.get("state") == "click":
            try:
                page.mouse.click(box["x"], box["y"])
            except Exception:
                pass
            page.wait_for_timeout(2200)
        else:
            page.wait_for_timeout(1000)

    # Terakhir: cek apakah panel sudah terbuka.
    return bool(page.evaluate("""() => {
        const s = document.querySelector('#aliyunCaptcha-sliding-slider');
        return !!(s && s.getBoundingClientRect().width > 0);
    }"""))


def act_captcha_geometry(args):
    page = get_page()

    # Panel CAPTCHA bisa dalam dua keadaan: tombol "Click to verify" (belum
    # dibuka) atau slider yang sudah terbuka. Buka dulu kalau masih tertutup.
    # Kalau gambar belum muncul, coba buka ulang beberapa kali sebelum menyerah.
    ready = False
    for attempt in range(4):
        if not _open_panel(page):
            page.wait_for_timeout(1500)
            continue
        try:
            page.wait_for_function("""() => {
                const s = document.querySelector('#aliyunCaptcha-sliding-slider');
                return !!(s && s.getBoundingClientRect().width > 0);
            }""", timeout=10000)
            ready = True
            break
        except Exception:
            page.wait_for_timeout(1500)

    if not ready:
        return {"ready": False, "reason": "panel CAPTCHA tidak bisa dibuka"}

    # Gambar bisa dimuat belakangan; kalau src sudah ada tapi belum complete,
    # tetap lanjut karena pengambilan byte dilakukan lewat request context.
    for _ in range(10):
        st = page.evaluate("""() => {
            const img = document.querySelector('#aliyunCaptcha-img');
            return {src: !!(img && img.src), complete: !!(img && img.complete),
                    w: img ? img.naturalWidth : 0};
        }""")
        if st and st.get("src") and st.get("complete") and st.get("w", 0) > 0:
            break
        page.wait_for_timeout(600)

    page.wait_for_timeout(300)

    bg = _data_url(page, "#aliyunCaptcha-img")
    pz = _data_url(page, "#aliyunCaptcha-puzzle")
    if not bg or not pz:
        return {"ready": False, "reason": "gambar tidak terbaca"}

    geo = page.evaluate("""() => {
        const s = document.querySelector('#aliyunCaptcha-sliding-slider');
        const i = document.querySelector('#aliyunCaptcha-img');
        const p = document.querySelector('#aliyunCaptcha-puzzle');
        if (!s || !i) return null;
        const r = s.getBoundingClientRect();
        return {
            sliderX: r.x, sliderY: r.y, sliderW: r.width, sliderH: r.height,
            naturalW: i.naturalWidth, displayW: i.getBoundingClientRect().width,
            puzzleLeft: p ? (parseFloat(p.style.left) || 0) : 0,
        };
    }""")
    if not geo:
        return {"ready": False, "reason": "geometri tidak terbaca"}

    return {"ready": True, "bg": bg, "puzzle": pz, "geometry": geo}


def act_captcha_drag(args):
    page = get_page()
    target = float(args["targetDisplay"])
    geo = args["geometry"]
    max_steps = int(args.get("maxSteps", 60))

    sx = geo["sliderX"] + 10
    sy = geo["sliderY"] + geo["sliderH"] / 2
    track_w = geo["sliderW"]

    # Algoritma mengikuti implementasi Python asli yang sudah terbukti:
    # feedback diambil dari style.left elemen puzzle (posisi yang dilaporkan
    # widget sendiri), dan konversi jarak puzzle ke jarak mouse memakai rasio
    # track_w / puzzle_range dengan puzzle_range = lebar gambar - 52.
    img_display = float(geo.get("displayW") or 0.0)
    puzzle_range = max(img_display - 52.0, 1.0)
    ratio = track_w / puzzle_range

    def puzzle_left():
        """Posisi puzzle menurut widget (style.left), bukan geometri DOM."""
        return page.evaluate("""() => {
            const p = document.querySelector('#aliyunCaptcha-puzzle');
            return p ? (parseFloat(p.style.left) || 0) : null;
        }""")

    page.mouse.move(sx, sy)
    page.wait_for_timeout(random.randint(60, 120))
    page.mouse.down()
    page.wait_for_timeout(random.randint(30, 60))

    # Fase 1: lompat langsung ke perkiraan akhir. Mouse HARUS bisa jauh melewati
    # lebar track thumb (40px) — satu piksel puzzle butuh ~6 piksel gerakan
    # mouse (ratio ~0.161), jadi gap 105px butuh ~650px gerakan mouse. Clamp
    # 1270 di sini membatasi viewport, bukan track.
    frac = target / puzzle_range
    cur_x = max(10.0, min(1270.0, sx + frac * track_w))
    page.mouse.move(cur_x, sy + random.uniform(-1, 1), steps=10)
    page.wait_for_timeout(100)

    # Fase 2: koreksi memakai style.left seperti versi asli.
    aligned = False
    for _ in range(60):
        left = puzzle_left()
        if left is None:
            break
        remaining = target - float(left)
        if abs(remaining) <= 2.0:
            aligned = True
            break
        delta = remaining * ratio
        delta = max(-60.0, min(60.0, delta)) + random.uniform(-1, 1)
        cur_x = max(10.0, min(1270.0, cur_x + delta))
        page.mouse.move(cur_x, sy + random.uniform(-1, 1), steps=5)
        page.wait_for_timeout(random.randint(15, 30))

    page.wait_for_timeout(random.randint(60, 140))
    page.mouse.up()
    page.wait_for_timeout(2800)

    # Deteksi keberhasilan. Satu-satunya bukti tegas CAPTCHA lolos adalah
    # munculnya field verification code (tahap berikutnya). Widget yang runtuh
    # ke "Click to verify" atau masih menampilkan teks captcha berarti gagal,
    # meskipun slider sudah tidak terlihat.
    st = page.evaluate("""() => {
        const vis = el => !!(el && el.getBoundingClientRect().width > 0);
        const slider = document.querySelector('#aliyunCaptcha-sliding-slider');
        const code = document.querySelector('input[maxlength="6"]');
        const body = (document.body.innerText || '').toLowerCase();
        return {
            sliderVisible: vis(slider),
            codeVisible: !!(code && code.offsetParent !== null),
            failedMsg: body.includes('unable to verify'),
            needComplete: body.includes('please complete the captcha'),
            stillCaptcha: body.includes('drag slide') || body.includes('click to verify'),
            bodySnippet: body.slice(0, 120),
        };
    }""")

    # Hanya field kode yang dianggap bukti lolos.
    solved = bool(st and st.get("codeVisible"))

    return {"aligned": aligned, "solved": solved, "url": page.url, "state": st}


def act_wait_otp_form(args):
    page = get_page()
    for _ in range(int(args.get("rounds", 15))):
        page.wait_for_timeout(2000)
        body = (page.inner_text("body") or "").lower()
        if any(k in body for k in ("verif", "code", "otp", "6-digit", "6 digit")):
            return {"ready": True, "url": page.url}
        if "/sign-up" not in page.url:
            return {"ready": True, "url": page.url}
    return {"ready": False, "url": page.url}


def act_submit_otp(args):
    """Ketik kode OTP lalu laporkan apakah TMD meloloskan verifikasi.

    Halaman verifikasi tidak punya tombol submit; kode dikirim otomatis saat
    field terisi. Yang menentukan berhasil bukan cara mengisi (setter native,
    insertText, dan keyboard.type menghasilkan hal yang sama) melainkan
    keputusan TMD Alibaba: bila IP proxy sudah ditandai, submit diarahkan ke
    /api/v1/users/_____tmd_____/punish dan sesi tidak pernah terbit.

    Penanda lolos: halaman berpindah ke /download dan /api/v1/me menjawab 200.
    Bila yang muncul halaman punish, akun perlu dicoba ulang dengan proxy lain.

    Setelah field terisi halaman mulai navigasi, jadi semua pembacaan status
    memakai ctx.request dan listener event, bukan page.evaluate.
    """
    page = get_page()
    ctx = page.context
    otp = args["otp"]

    reqs, navs = [], []
    page.on("request", lambda r: reqs.append(r.url))
    page.on("framenavigated", lambda f: navs.append(f.url))

    try:
        page.evaluate(
            """() => { const el = document.querySelector('input[maxlength="6"]');
                       if (el) el.focus(); }""")
    except Exception:
        return {"filled": False, "reason": "input OTP tidak ditemukan"}

    cdp = ctx.new_cdp_session(page)
    for ch in otp:
        cdp.send("Input.insertText", {"text": ch})
        page.wait_for_timeout(random.randint(60, 130))

    # Beri waktu auto-submit dan navigasi hasilnya.
    page.wait_for_timeout(12000)

    punished = any("_____tmd_____" in u or "/punish" in u for u in (navs + reqs))

    # Baca sesi lewat konteks; 401 berarti belum terautentikasi.
    status, body = None, ""
    for _ in range(5):
        try:
            r = ctx.request.get(
                "https://qoder.com/api/v1/me",
                headers={"Accept": "application/json"}, timeout=20000)
            status = r.status
            body = r.text()[:200]
        except Exception as exc:
            status, body = None, str(exc)[:120]
        if status == 200:
            break
        page.wait_for_timeout(3000)

    verified = status == 200
    return {"filled": True, "status": status, "verified": verified,
            "punished": punished, "via": "cdp-insert-text", "body": body}


def act_signin_credentials(args):
    """Tahap 1 login: isi email + password, submit, tunggu CAPTCHA muncul.

    Alur sign-in Qoder bertahap: email -> Continue -> password -> Sign in ->
    CAPTCHA -> verification code. Tahap CAPTCHA dan kode ditangani aksi
    terpisah supaya solver CAPTCHA yang sama bisa dipakai ulang.
    """
    page = get_page()
    page.goto("https://qoder.com/users/sign-in",
              wait_until="domcontentloaded", timeout=60000)
    page.wait_for_timeout(2500)

    try:
        page.locator("#basic_email").fill(args["email"], timeout=20000)
        page.wait_for_timeout(400)
        page.get_by_role("button", name="Continue").first.click()
        page.wait_for_timeout(3000)
    except Exception as e:
        return {"ok": False, "stage": "email", "reason": str(e)[:120], "url": page.url}

    pw_ok = False
    for sel in ("#password_password", "#basic_password"):
        try:
            pw = page.locator(sel)
            if pw.count() > 0 and pw.first.is_visible():
                pw.first.fill(args["password"])
                pw_ok = True
                break
        except Exception:
            continue

    if not pw_ok:
        return {"ok": False, "stage": "password", "reason": "field password tidak ada", "url": page.url}

    page.wait_for_timeout(600)

    try:
        page.get_by_role("button", name="Sign in").first.click()
    except Exception:
        page.get_by_role("button", name="Continue").first.click()

    # Tunggu CAPTCHA (tombol "Click to verify") atau form kode muncul.
    captcha = False
    code_ready = False
    for _ in range(24):
        page.wait_for_timeout(700)
        state = page.evaluate("""() => {
            const vis = el => !!(el && el.getBoundingClientRect().width > 0);
            return {
                // Tombol awal "Click to verify" ATAU slider yang sudah terbuka.
                captcha: vis(document.querySelector('#captcha-button')) ||
                         vis(document.querySelector('#aliyunCaptcha-sliding-slider')),
                code: (() => {
                    const c = document.querySelector('input[maxlength="6"]');
                    return !!(c && c.offsetParent !== null);
                })(),
            };
        }""")
        if state.get("captcha"):
            captcha = True
            break
        if state.get("code"):
            code_ready = True
            break

    return {
        "ok": True,
        "captcha": captcha,
        "code_ready": code_ready,
        "url": page.url,
    }


def act_signin_code(args):
    """Tahap akhir login: isi verification code lalu submit.

    Menyelesaikan CAPTCHA membuat form kembali ke langkah kredensial dengan
    field password kosong; kalau langsung submit, server menjawab
    "Incorrect email or password". Karena itu password diisi ulang lebih dulu
    bila field-nya terlihat dan kosong.
    """
    page = get_page()
    otp = args["otp"]
    password = args.get("password", "")

    # Isi ulang password bila form kembali ke langkah kredensial.
    refilled = False
    for sel in ("#password_password", "#basic_password"):
        try:
            el = page.locator(sel)
            if el.count() > 0 and el.first.is_visible():
                cur = el.first.input_value()
                if not cur and password:
                    el.first.fill(password)
                    refilled = True
                break
        except Exception:
            continue

    if refilled:
        page.wait_for_timeout(500)
        for name in ("Sign in", "Continue"):
            try:
                btn = page.get_by_role("button", name=name)
                if btn.count() and btn.first.is_visible():
                    btn.first.click()
                    break
            except Exception:
                continue
        page.wait_for_timeout(6000)

    code_filled = False
    for _ in range(12):
        for sel in ('input[maxlength="6"]', '#basic_code', 'input[name*="code" i]'):
            try:
                el = page.locator(sel)
                if el.count() > 0 and el.first.is_visible():
                    el.first.fill(otp)
                    code_filled = True
                    break
            except Exception:
                continue
        if code_filled:
            break
        page.wait_for_timeout(700)

    if not code_filled:
        # Laporkan keadaan DOM supaya penyebabnya kelihatan dari sisi Rust.
        dump = page.evaluate("""() => {
            const inputs = Array.from(document.querySelectorAll('input')).map(e => ({
                id: e.id, type: e.type, ml: e.getAttribute('maxlength'),
                vis: e.offsetParent !== null,
            }));
            const btns = Array.from(document.querySelectorAll('button'))
                .filter(e => e.offsetParent !== null)
                .map(e => (e.innerText || '').trim().slice(0, 30));
            return {
                url: location.href,
                inputs,
                btns,
                body: (document.body.innerText || '').slice(0, 300).replace(/\\n/g, ' | '),
            };
        }""")
        return {"code_filled": False, "authenticated": False, "url": page.url, "dump": dump}

    page.wait_for_timeout(400)

    for name in ("Sign in", "Continue", "Verify", "Submit"):
        try:
            btn = page.get_by_role("button", name=name)
            if btn.count() and btn.first.is_visible():
                btn.first.click()
                break
        except Exception:
            continue

    # Tunggu sesi benar-benar aktif. Endpoint /api/v1/users dipakai karena
    # itulah yang menjawab 200 untuk sesi valid; /api/v1/me tidak ada di
    # backend Qoder dan selalu 401 walau sesi sudah aktif.
    authed = False
    me_status = None
    for _ in range(15):
        page.wait_for_timeout(1500)
        me = page.evaluate("""async () => {
            try {
                const r = await fetch('/api/v1/users', {credentials:'include',
                    headers: {'Accept': 'application/json'}});
                return {status: r.status};
            } catch (e) { return {status: 0}; }
        }""")
        me_status = me.get("status") if isinstance(me, dict) else None
        if me_status == 200:
            authed = True
            break
        # Form bisa kembali ke langkah kredensial setelah submit; isi ulang
        # password lalu submit lagi sebelum menyerah.
        if me_status == 401:
            try:
                el = page.locator("#password_password")
                if el.count() > 0 and el.first.is_visible() and not el.first.input_value():
                    el.first.fill(password)
                    page.wait_for_timeout(400)
                    for name in ("Sign in", "Continue"):
                        btn = page.get_by_role("button", name=name)
                        if btn.count() and btn.first.is_visible():
                            btn.first.click()
                            break
            except Exception:
                pass

    return {
        "code_filled": True,
        "authenticated": authed,
        "me_status": me_status,
        "url": page.url,
    }


def act_signin_full(args):
    """Login penuh setelah akun terverifikasi.

    Registrasi hanya membuat akun; sesi browser belum terautentikasi. Halaman
    sign-in Qoder meminta email, password, dan verification code sekaligus
    (signInStep=credentialStep), jadi kode yang sama dipakai lagi di sini untuk
    mendapatkan sesi cookie yang valid. Tanpa langkah ini, request ke endpoint
    ber-sesi (termasuk pembuatan PAT) dijawab 401 Unauthorized.
    """
    page = get_page()
    email = args["email"]
    password = args["password"]
    otp = args["otp"]

    page.goto("https://qoder.com/users/sign-in",
              wait_until="domcontentloaded", timeout=60000)
    page.wait_for_timeout(2500)

    # Step 1: email
    try:
        page.locator("#basic_email").fill(email, timeout=20000)
        page.wait_for_timeout(400)
        page.get_by_role("button", name="Continue").first.click()
        page.wait_for_timeout(2500)
    except Exception as e:
        return {"ok_step": False, "reason": f"email step: {str(e)[:100]}", "url": page.url}

    # Step 2: password. Halaman credentialStep memakai #password_password;
    # #basic_password hanya ada di form registrasi.
    pw_ok = False
    for sel in ("#password_password", "#basic_password"):
        try:
            pw = page.locator(sel)
            if pw.count() > 0 and pw.first.is_visible():
                pw.first.fill(password)
                pw_ok = True
                break
        except Exception:
            continue
    page.wait_for_timeout(800)

    # Step 3: verification code — field baru muncul setelah password terisi.
    code_filled = False
    for _ in range(10):
        for sel in ('input[maxlength="6"]', '#basic_code', 'input[name*="code" i]'):
            try:
                el = page.locator(sel)
                if el.count() > 0 and el.first.is_visible():
                    el.first.fill(otp)
                    code_filled = True
                    break
            except Exception:
                continue
        if code_filled:
            break
        page.wait_for_timeout(700)

    if not code_filled:
        # input per digit
        digits = page.locator('input[maxlength="1"]:visible')
        if digits.count() >= 4:
            n = digits.count()
            for i, ch in enumerate(otp[:n]):
                digits.nth(i).fill(ch)
                page.wait_for_timeout(100)
            code_filled = True

    page.wait_for_timeout(500)

    # Submit
    submitted = False
    for name in ("Sign in", "Continue", "Verify", "Submit"):
        try:
            btn = page.get_by_role("button", name=name)
            if btn.count() and btn.first.is_visible():
                btn.first.click()
                submitted = True
                break
        except Exception:
            continue

    page.wait_for_timeout(7000)

    # Verifikasi sesi lewat endpoint yang butuh auth
    me = page.evaluate("""async () => {
        try {
            const r = await fetch('/api/v1/users', {credentials:'include',
                headers: {'Accept': 'application/json'}});
            return {status: r.status, body: (await r.text()).substring(0, 200)};
        } catch (e) { return {status: 0, body: String(e)}; }
    }""")

    authed = isinstance(me, dict) and me.get("status") == 200

    return {
        "code_filled": code_filled,
        "submitted": submitted,
        "authenticated": authed,
        "me_status": me.get("status") if isinstance(me, dict) else None,
        "url": page.url,
    }


def _csrf_token(page):
    """Ambil token CSRF dari cookie atau meta tag halaman."""
    try:
        t = page.evaluate("""() => {
            const m = document.cookie.match(/(?:csrf|xsrf)[^=]*=([^;]+)/i);
            if (m) return decodeURIComponent(m[1]);
            for (const el of document.querySelectorAll(
                    'meta[name*="csrf" i], meta[name*="xsrf" i]')) {
                const v = el.getAttribute('content');
                if (v) return v;
            }
            return '';
        }""")
        if t:
            return t
    except Exception:
        pass
    return "_echo_csrf_using_sec_fetch_site_"


def act_create_pat(args):
    page = get_page()

    # Pastikan berada di origin qoder.com supaya cookie sesi ikut terkirim.
    if "qoder.com" not in page.url:
        page.goto("https://qoder.com/account/profile", timeout=30000)
        page.wait_for_timeout(1500)

    csrf = _csrf_token(page)

    payload = {"name": "Key", "expires_at": 1817654399999}

    # Panggil lewat fetch di dalam halaman, bukan page.request: page.request
    # adalah konteks HTTP terpisah yang tidak membawa cookie sesi, sehingga
    # endpoint selalu menjawab 401 walau browser sudah login.
    res = page.evaluate("""async ([url, payload, csrf]) => {
        const headers = {
            'Content-Type': 'application/json',
            'Accept': 'application/json',
            'Referer': 'https://qoder.com/account/profile',
        };
        if (csrf) headers['X-CSRF-Token'] = csrf;
        try {
            const r = await fetch(url, {
                method: 'POST',
                credentials: 'include',
                headers,
                body: JSON.stringify(payload),
            });
            const text = await r.text();
            return {status: r.status, body: text.slice(0, 800)};
        } catch (e) {
            return {status: -1, body: String(e)};
        }
    }""", [QODER_PAT_API, payload, csrf])

    status = res.get("status", -1)
    text = res.get("body", "")

    # Beberapa build Qoder memakai endpoint settings; coba kalau yang utama gagal.
    if status not in (200, 201):
        alt = "https://qoder.com/api/v1/settings/personal-access-tokens"
        res2 = page.evaluate("""async ([url, payload, csrf]) => {
            const headers = {'Content-Type': 'application/json',
                             'Accept': 'application/json',
                             'Referer': 'https://qoder.com/account/profile'};
            if (csrf) headers['X-CSRF-Token'] = csrf;
            try {
                const r = await fetch(url, {method: 'POST', credentials: 'include',
                    headers, body: JSON.stringify(payload)});
                return {status: r.status, body: (await r.text()).slice(0, 800)};
            } catch (e) { return {status: -1, body: String(e)}; }
        }""", [alt, payload, csrf])
        if res2.get("status") in (200, 201):
            status, text = res2["status"], res2["body"]

    token = ""
    try:
        token = (json.loads(text) or {}).get("token") or ""
    except Exception:
        pass
    if not token:
        m = re.search(r'"token"\s*:\s*"([^"]+)"', text)
        if m:
            token = m.group(1)

    return {
        "status": status,
        "token": token,
        "body": text[:400],
        "csrf_found": bool(csrf),
    }


def act_captcha_refresh(args):
    """Minta soal CAPTCHA baru setelah percobaan yang gagal.

    Setelah drag salah, Aliyun menandai sesi itu ("Unable to verify the user is
    human") dan panelnya sering runtuh. Membuka ulang panel tidak cukup karena
    sesinya sudah ter-flag — soal baru di sesi yang sama tetap ditolak. Jadi
    jalur utama di sini adalah muat ulang halaman penuh lalu isi ulang form,
    yang memberi sesi CAPTCHA baru.
    """
    page = get_page()

    # Muat ulang halaman: sesi CAPTCHA baru, bukan soal baru di sesi lama.
    email = args.get("email", "")
    password = args.get("password", "")
    if email and password:
        try:
            page.goto("https://qoder.com/users/sign-up",
                      wait_until="domcontentloaded", timeout=60000)
            page.wait_for_timeout(2500)
            act_fill_registration({"email": email, "password": password})
            page.wait_for_timeout(2500)
            if _open_panel(page, tries=3):
                page.wait_for_timeout(2500)
            ready = page.evaluate("""() => {
                const img = document.querySelector('#aliyunCaptcha-img');
                const s = document.querySelector('#aliyunCaptcha-sliding-slider');
                return !!(img && img.complete && img.naturalWidth > 0 &&
                          s && s.getBoundingClientRect().width > 0);
            }""")
            if ready:
                return {"clicked": "reload", "ready": True}
        except Exception as e:
            log(f"reload captcha gagal: {e}")

    # Selalu minta soal baru lewat tombol refresh bila panelnya masih ada.
    clicked = page.evaluate("""() => {
        const el = document.getElementById('aliyunCaptcha-btn-refresh');
        if (el) {
            const r = el.getBoundingClientRect();
            if (r.width > 0 && r.height > 0) {
                el.click();
                return 'refresh';
            }
        }
        return '';
    }""")

    page.wait_for_timeout(2500)

    ready = page.evaluate("""() => {
        const img = document.querySelector('#aliyunCaptcha-img');
        const s = document.querySelector('#aliyunCaptcha-sliding-slider');
        return !!(img && img.complete && img.naturalWidth > 0 &&
                  s && s.getBoundingClientRect().width > 0);
    }""")

    if not ready:
        # Panel runtuh: buka ulang lewat tombol verifikasi.
        for _ in range(4):
            if _open_panel(page, tries=2):
                page.wait_for_timeout(2500)
                ready = page.evaluate("""() => {
                    const img = document.querySelector('#aliyunCaptcha-img');
                    const s = document.querySelector('#aliyunCaptcha-sliding-slider');
                    return !!(img && img.complete && img.naturalWidth > 0 &&
                              s && s.getBoundingClientRect().width > 0);
                }""")
                if ready:
                    break
            page.wait_for_timeout(1500)

    return {"clicked": clicked, "ready": bool(ready)}


def act_status(args):
    page = get_page()
    return {"url": page.url, "title": page.title()}


def act_export_cookies(args):
    """Ekspor cookie sesi qoder.com ke file JSON.

    Cookie inilah yang membuat GUI bisa login tanpa OAuth: GUI dan browser
    sama-sama memakai cookie sesi qoder.com, jadi menyuntikkannya ke GUI
    melewati seluruh alur device flow.
    """
    page = get_page()
    ctx = _ctx.get("ctx") or _ctx.get("browser")
    if ctx is None:
        return {"ok": False, "error": "context tidak ada"}
    cookies = ctx.cookies()
    picked = [c for c in cookies if "qoder" in c.get("domain", "")]
    path = args.get("path") or "/tmp/qoder_cookies.json"
    with open(path, "w") as f:
        json.dump(picked, f)
    return {"ok": True, "count": len(picked), "path": path,
            "names": [c["name"] for c in picked]}


def act_check_session(args):
    """Cek apakah sesi browser sudah terautentikasi.

    Setelah verifikasi OTP, sesi biasanya langsung aktif — memaksa login ulang
    hanya menambah satu CAPTCHA yang tidak perlu, dan CAPTCHA login itulah yang
    paling sering ditolak.
    """
    page = get_page()
    # Panggil endpoint yang butuh autentikasi dari dalam halaman, supaya cookie
    # sesi ikut terkirim. Status 200 berarti sesi valid.
    status = page.evaluate("""async () => {
        try {
            const r = await fetch('/api/v1/users', {
                credentials: 'include',
                headers: {'Accept': 'application/json'}
            });
            return r.status;
        } catch (e) {
            return -1;
        }
    }""")
    return {"authenticated": status == 200, "status": status, "url": page.url}


def act_close(args):
    stop_browser()
    return {"closed": True}


ACTIONS = {
    "start": act_start,
    "navigate": act_navigate,
    "fill_registration": act_fill_registration,
    "captcha_geometry": act_captcha_geometry,
    "captcha_drag": act_captcha_drag,
    "captcha_refresh": act_captcha_refresh,
    "wait_otp_form": act_wait_otp_form,
    "submit_otp": act_submit_otp,
    "signin_credentials": act_signin_credentials,
    "signin_code": act_signin_code,
    "signin_full": act_signin_full,
    "create_pat": act_create_pat,
    "status": act_status,
    "check_session": act_check_session,
    "export_cookies": act_export_cookies,
    "close": act_close,
}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            cmd = json.loads(line)
        except Exception as e:
            emit({"ok": False, "error": f"JSON invalid: {e}"})
            continue

        action = cmd.get("action", "")
        if action == "shutdown":
            stop_browser()
            emit({"ok": True, "closed": True})
            return 0

        fn = ACTIONS.get(action)
        if fn is None:
            emit({"ok": False, "error": f"action tidak dikenal: {action}"})
            continue

        # Aksi dijalankan di thread utama. Playwright sync API terikat ke
        # thread pembuat page-nya (greenlet), jadi memindahkannya ke thread
        # lain memicu "cannot switch to a different thread". Batas waktu
        # ditangani di sisi Rust: kalau bridge tidak menjawab, prosesnya
        # dimatikan dan akun itu ditandai gagal.
        try:
            out = {"ok": True}
            out.update(fn(cmd) or {})
            emit(out)
        except Exception as e:
            emit({
                "ok": False,
                "error": str(e),
                "trace": traceback.format_exc()[-1200:],
            })

    stop_browser()
    return 0


if __name__ == "__main__":
    sys.exit(main())
