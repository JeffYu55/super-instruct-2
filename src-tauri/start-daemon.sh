#!/bin/bash
# High-availability keepalive runner for super-instruct proxy
# Prevents bind thrashing by waiting for port 8080 to be free before launching

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

mkdir -p ../logs

LOCK_DIR="${TMPDIR:-/tmp}/super-instruct-$(id -u).lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    if curl -fsS --max-time 2 http://127.0.0.1:8080/ >/dev/null 2>&1; then
        echo "[$(date)] Proxy already healthy; duplicate daemon exiting." >> ../logs/keepalive.log
        exit 0
    fi
    rmdir "$LOCK_DIR" 2>/dev/null || {
        echo "[$(date)] Another daemon owns $LOCK_DIR; exiting." >> ../logs/keepalive.log
        exit 1
    }
    mkdir "$LOCK_DIR" 2>/dev/null || exit 1
fi
cleanup() {
    rmdir "$LOCK_DIR" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 0' INT TERM

while true; do
    # Ensure port 8080 is not currently occupied before starting
    while lsof -nP -iTCP:8080 -sTCP:LISTEN >/dev/null 2>&1; do
        echo "[$(date)] Port 8080 is busy, waiting..." >> ../logs/keepalive.log
        sleep 1
    done

    echo "[$(date)] Starting super-instruct proxy..." >> ../logs/keepalive.log
    ./target/release/super-instruct >> ../logs/service-console.log 2>&1
    EXIT_CODE=$?
    echo "[$(date)] super-instruct proxy exited with code $EXIT_CODE." >> ../logs/keepalive.log
    sleep 3
done
