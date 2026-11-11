#!/usr/bin/env bash
# Jalankan qoder-creator berulang dengan proxy berbeda sampai satu akun lolos
# verifikasi TMD dan PAT berhasil dibuat.
#
# TMD Alibaba menolak sebagian besar IP proxy premium (submit diarahkan ke
# /_____tmd_____/punish), jadi keberhasilan bersifat probabilistik dan rotasi
# proxy adalah bagian dari alur, bukan retry error.
set -u

cd "$(dirname "$0")"

export BRIDGE_PYTHON="$HOME/.camoufox-venv/bin/python"
export BRIDGE_SCRIPT="$PWD/bridge.py"
export HEADLESS=1
export QODER_DATA_DIR="$PWD/data"
export BRIDGE_TIMEOUT=300
export CHROME_BIN="$HOME/.hermes/tools/chromium-1208/chrome-linux64/chrome"
export QODER_TZ="Europe/Vienna"
export CAPTCHA_ATTEMPTS=6
export QODER_PROXY_FILE="$PWD/proxies/premium_pool.txt"

MAX_TRY="${1:-40}"
START_IDX="${2:-1}"
mkdir -p "$HOME/qoder-loop"

ok=0
try=0
idx=$START_IDX

while [ "$try" -lt "$MAX_TRY" ]; do
    try=$((try + 1))
    total=$(wc -l < "$QODER_PROXY_FILE")
    if [ "$idx" -gt "$total" ]; then idx=1; fi

    log="$HOME/qoder-loop/run-$(printf '%02d' "$try")-p$idx.log"
    echo "=== percobaan $try/$MAX_TRY (proxy #$idx) ==="

    QODER_PROXY_INDEX=$idx timeout 800 ./target/release/qoder-creator run 1 \
        > "$log" 2>&1

    if grep -qa "PAT: pt-" "$log"; then
        ok=$((ok + 1))
        echo "  ✅ PAT didapat — $(grep -a 'Berhasil:' "$log" | tail -1)"
        grep -a "PAT: pt-" "$log" | tail -1
        break
    fi

    if grep -qa "tmd_punish" "$log"; then
        echo "  ✗ TMD punish (proxy #$idx)"
    elif grep -qa "CAPTCHA gagal" "$log"; then
        echo "  ✗ CAPTCHA gagal"
    elif grep -qa "OTP timeout\|OTP tidak" "$log"; then
        echo "  ✗ OTP tidak masuk"
    else
        echo "  ✗ $(grep -a '❌' "$log" | tail -1)"
    fi

    idx=$((idx + 1))
done

echo "══════════════════════════════════════"
echo "  selesai: $ok akun dapat PAT dari $try percobaan"
