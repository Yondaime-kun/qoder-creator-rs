#!/usr/bin/env bash
# Loop pembuatan akun sampai satu akun menembus seluruh alur dan klaim
# Pro Trial. CAPTCHA Aliyun menilai environment secara probabilistik: proxy
# dan sesi yang sama bisa lolos di satu percobaan dan ditolak di percobaan
# lain, jadi satu kali jalan tidak cukup untuk menyimpulkan hasil.
#
# Setiap iterasi memakai proxy berikutnya dari pool (rotasi), sehingga
# kegagalan satu IP tidak menghentikan seluruh proses.
set -u

cd "$(dirname "$0")" || exit 1

POOL="${POOL:-proxies/pool_ok.txt}"
MAX_TRIES="${MAX_TRIES:-40}"
LOG_DIR="${LOG_DIR:-$HOME/qoder-loop}"
mkdir -p "$LOG_DIR"

mapfile -t PROXIES < <(grep -v '^\s*$' "$POOL")
if [ ${#PROXIES[@]} -eq 0 ]; then
  echo "pool kosong: $POOL"
  exit 1
fi
echo "pool: ${#PROXIES[@]} proxy, maks ${MAX_TRIES} percobaan"

for i in $(seq 1 "$MAX_TRIES"); do
  # Geser titik awal tiap kali loop dijalankan supaya tidak selalu menabrak
  # proxy yang sama (IP yang sudah jenuh akan gagal terus).
  start="${START_IDX:-0}"
  px="${PROXIES[$(( (start + i - 1) % ${#PROXIES[@]} ))]}"
  host="${px%%:*}"
  log="$LOG_DIR/try-$(printf '%02d' "$i").log"

  echo "=== percobaan $i/${MAX_TRIES} via $host ==="
  BRIDGE_PYTHON="$HOME/.camoufox-venv/bin/python" \
  BRIDGE_SCRIPT="$PWD/bridge.py" \
  HEADLESS=1 \
  QODER_DATA_DIR="$PWD/data" \
  BRIDGE_TIMEOUT=280 \
  CHROME_BIN="${CHROME_BIN:-$HOME/.hermes/tools/chromium-1208/chrome-linux64/chrome}" \
  QODER_PROXY="$px" \
  QODER_TZ="Europe/Vienna" \
  CAPTCHA_ATTEMPTS="${CAPTCHA_ATTEMPTS:-6}" \
  OTP_TIMEOUT="${OTP_TIMEOUT:-300}" \
  timeout 1400 ./target/release/qoder-creator run 1 > "$log" 2>&1

  # Keberhasilan dinilai dari hasil akhir di log, bukan exit code: proses
  # keluar 0 meski akun gagal, karena kegagalan akun bukan kegagalan program.
  # Bukti harus plan Pro/trial aktif, bukan sekadar akun terbuat.
  if grep -qa "Plan Pro/trial aktif" "$log"; then
    echo "=== BERHASIL pada percobaan $i ($host) ==="
    grep -aE "✅|Pro Trial|trial|PAT|1/1" "$log" | tail -20
    exit 0
  fi
  echo "  gagal, lanjut"
done

echo "=== ${MAX_TRIES} percobaan habis tanpa hasil ==="
exit 1
