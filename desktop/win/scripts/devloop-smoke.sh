#!/usr/bin/env bash
# Devloop v1 smoke test — manual end-to-end validation.
#
# Usage:
#   Terminal 1:  cd desktop/win && cargo run -p midas-app --features dev_harness
#   Terminal 2:  cd desktop/win && ./scripts/devloop-smoke.sh
#
# Or, to boot from a fixture:
#   Terminal 1:  cargo run -p midas-app --features dev_harness -- --fixture <name>
#
# Requires: bash, nc (ncat/netcat on WSL or Git Bash).

set -e

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"

send() {
  printf '%s\n' "$1" | nc -w 2 "$HOST" "$PORT"
}

hr() {
  printf -- '---- %s ----\n' "$1"
}

hr "ping"
send '{"cmd":"ping"}'
echo

hr "dump tickers (top-level)"
send '{"cmd":"dump_state","path":"tickers"}'
echo

hr "snapshot current state as fixture 'smoke-1'"
send '{"cmd":"snapshot_fixture","name":"smoke-1","note":"devloop smoke test"}'
echo

hr "inject SetBracketMode=Buy for AAPL"
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetBracketMode","side":"Buy"}}'
echo

hr "wait for SetBracketMode event"
send '{"cmd":"wait_for_event","event_type":"SetBracketMode","timeout_ms":1000}'
echo

hr "inject EnsureDraftBracket Buy / Limit"
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"EnsureDraftBracket","side":"Buy","entry_type":"Limit"}}'
echo

hr "set entry leg price"
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetLegPrice","role":"Entry","price":184.50}}'
echo

hr "dump AAPL live bracket"
send '{"cmd":"dump_state","path":"tickers.AAPL.live_bracket"}'
echo

hr "screenshot (.devloop/shots/smoke-1.png)"
send '{"cmd":"screenshot","out_path":".devloop/shots/smoke-1.png"}'
echo

hr "wait for idle"
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

hr "reload fixture smoke-1 — state should match snapshot"
send '{"cmd":"load_fixture","name":"smoke-1"}'
echo

echo
echo "Smoke flow complete. Run  '{\"cmd\":\"shutdown\"}'  to exit the app."
