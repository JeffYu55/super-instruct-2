#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME_DIR="${SUPER_INSTRUCT_RUNTIME_DIR:-$HOME/.codex/super-instruct-runtime}"
INTERACTIONS="${SUPER_INSTRUCT_INTERACTIONS:-$RUNTIME_DIR/logs/interactions.jsonl}"
POLICY="${SUPER_INSTRUCT_EVOLUTION_POLICY:-$RUNTIME_DIR/evolution/policy.json}"
HISTORY="${SUPER_INSTRUCT_EVOLUTION_HISTORY:-$RUNTIME_DIR/evolution/generations}"
INTERVAL="${SUPER_INSTRUCT_EVOLUTION_INTERVAL_SECS:-300}"
LOG="${SUPER_INSTRUCT_EVOLUTION_LOG:-$RUNTIME_DIR/logs/evolution.log}"

mkdir -p "$(dirname "$POLICY")" "$HISTORY" "$(dirname "$LOG")"
if [[ ! -f "$POLICY" ]]; then
  cp "$ROOT/evolution/policy.json" "$POLICY"
fi

while true; do
  python3 "$ROOT/scripts/evolution_engine.py" \
    --interactions "$INTERACTIONS" \
    --config "$ROOT/evolution/config.json" \
    --policy "$POLICY" \
    --history "$HISTORY" >>"$LOG" 2>&1 || true
  sleep "$INTERVAL"
done
