#!/bin/bash
# High-availability keepalive runner for super-instruct proxy
# Prevents bind thrashing by waiting for port 8080 to be free before launching

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

mkdir -p ../logs

while true; do
    # Ensure port 8080 is not currently occupied before starting
    while lsof -i :8080 >/dev/null 2>&1; do
        echo "[$(date)] Port 8080 is busy, waiting..." >> ../logs/keepalive.log
        sleep 1
    done

    echo "[$(date)] Starting super-instruct proxy..." >> ../logs/keepalive.log
    ./target/release/super-instruct >> ../logs/service-console.log 2>&1
    EXIT_CODE=$?
    echo "[$(date)] super-instruct proxy exited with code $EXIT_CODE." >> ../logs/keepalive.log
    sleep 0.5
done
