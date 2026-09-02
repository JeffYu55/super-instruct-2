#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME="${SUPER_INSTRUCT_RUNTIME_DIR:-$HOME/.codex/super-instruct-runtime}"
SRC_BIN="$ROOT/src-tauri/target/release/super-instruct"
DST_BIN="$RUNTIME/src-tauri/target/release/super-instruct"

cargo test --manifest-path "$ROOT/src-tauri/Cargo.toml"
cargo build --release --manifest-path "$ROOT/src-tauri/Cargo.toml"
mkdir -p "$RUNTIME/src-tauri/target/release" "$RUNTIME/scripts" "$RUNTIME/evolution"

if [[ -f "$DST_BIN" ]]; then
    cp -f "$DST_BIN" "$DST_BIN.prev"
fi
cp -f "$SRC_BIN" "$DST_BIN"
cp -f "$ROOT/src-tauri/start-daemon.sh" "$RUNTIME/src-tauri/start-daemon.sh"
cp -f "$ROOT/scripts/evolution_engine.py" "$RUNTIME/scripts/evolution_engine.py"
cp -f "$ROOT/scripts/evolution_daemon.sh" "$RUNTIME/scripts/evolution_daemon.sh"
cp -f "$ROOT/evolution/config.json" "$RUNTIME/evolution/config.json"
cp -f "$ROOT/bridge.md" "$RUNTIME/bridge.md"
mkdir -p "$RUNTIME/codex-skills"
rsync -a "$ROOT/codex-skills/" "$RUNTIME/codex-skills/"
if [[ ! -f "$RUNTIME/evolution/policy.json" ]]; then
    cp -f "$ROOT/evolution/policy.json" "$RUNTIME/evolution/policy.json"
fi
chmod +x "$DST_BIN" "$RUNTIME/src-tauri/start-daemon.sh" "$RUNTIME/scripts/evolution_"*

label="gui/$(id -u)/com.jeff.super-instruct"
if launchctl print "$label" >/dev/null 2>&1; then
    launchctl kickstart -k "$label"
else
    listener="$(lsof -nP -iTCP:8080 -sTCP:LISTEN -t 2>/dev/null | head -1 || true)"
    if [[ -n "$listener" ]]; then
        kill "$listener" 2>/dev/null || true
    fi
    nohup "$RUNTIME/src-tauri/start-daemon.sh" \
        >>"$RUNTIME/logs/launchd.out.log" \
        2>>"$RUNTIME/logs/launchd.err.log" &
fi

for _ in $(seq 1 45); do
    if curl --noproxy '*' -fsS --max-time 2 http://127.0.0.1:8080/ >/dev/null 2>&1; then
        echo "runtime deployed and healthy"
        exit 0
    fi
    sleep 1
done

echo "health check failed; rolling back binary" >&2
if [[ -f "$DST_BIN.prev" ]]; then
    cp -f "$DST_BIN.prev" "$DST_BIN"
fi
exit 1
