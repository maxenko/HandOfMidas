#!/usr/bin/env bash
# Slice G smoke — multi-window devloop journey.
#
# Drives the harness commands added in slice G:
#   - OpenWindow (with and without an explicit name)
#   - ListWindows
#   - SetWindowFocus + window-targeted Key (Ctrl+N adds a chart in
#     the focused window)
#   - RenameWindow
#   - Screenshot { window: Some(...) } — captures the named window
#   - DumpState path "windows" — verifies the slice-G projection
#   - CloseWindow
#
# Usage:
#   Terminal 1:  cd desktop/win && cargo run -p midas-app --features dev_harness
#   Terminal 2:  cd desktop/win && ./tools/multi-window-journey.sh
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

mkdir -p .devloop/shots

hr "ping"
send '{"cmd":"ping"}'
echo

hr "list windows (expect just Main)"
send '{"cmd":"list_windows"}'
echo

hr "open named window 'Scanner'"
send '{"cmd":"open_window","name":"Scanner"}'
echo

hr "open auto-named window (slice C mints 'Window 3' or similar)"
send '{"cmd":"open_window"}'
echo

hr "wait for both attaches to settle"
send '{"cmd":"wait_for_idle","timeout_ms":2000}'
echo

hr "list windows (expect Main + Scanner + auto)"
send '{"cmd":"list_windows"}'
echo

hr "focus Scanner, then Ctrl+N → chart should land in Scanner, not Main"
send '{"cmd":"set_window_focus","name":"Scanner"}'
send '{"cmd":"key","combo":"Ctrl+N","window":"Scanner"}'
send '{"cmd":"wait_for_idle","timeout_ms":1000}'
echo

hr "dump windows projection — Scanner panel_count should be >= 1"
send '{"cmd":"dump_state","path":"windows"}'
echo

hr "screenshot Scanner"
send '{"cmd":"screenshot","out_path":".devloop/shots/multi-window-scanner.png","window":"Scanner"}'
echo

hr "rename Scanner → 'Day Trading'"
send '{"cmd":"rename_window","from":"Scanner","to":"Day Trading"}'
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

hr "screenshot Day Trading (titlebar should reflect rename)"
send '{"cmd":"screenshot","out_path":".devloop/shots/multi-window-renamed.png","window":"Day Trading"}'
echo

hr "close Day Trading"
send '{"cmd":"close_window","name":"Day Trading"}'
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

hr "list windows again — Day Trading should be gone"
send '{"cmd":"list_windows"}'
echo

hr "screenshot main (.devloop/shots/multi-window-main.png)"
send '{"cmd":"screenshot","out_path":".devloop/shots/multi-window-main.png"}'
echo

echo
echo "Multi-window journey complete. Run  '{\"cmd\":\"shutdown\"}'  to exit the app."
