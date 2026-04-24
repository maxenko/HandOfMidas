#!/usr/bin/env bash
# Chart-transition parity harness driver.
#
# Drives two sequential renders of the same fixture through the legacy
# and new chart backends (slice 9a's `ChartBackend` toggle is a
# prerequisite), writes both PNGs to disk, then asks the dev-harness to
# compare them via the slice-0 `CompareImages` command.
#
# Usage:
#   Terminal 1: cd desktop/win && cargo run -p midas-app --features dev_harness
#   Terminal 2: ./tools/devloop-chart-parity.sh [fixture-name]
#
# Until slice 9a lands, this script prints the full sequence it WOULD
# run and exits with a non-zero status. Keeping it checked in early
# means the TCP schema is pinned and CI can start wiring the call
# even before the backend toggle exists.
#
# Requires: bash, nc (ncat / Git Bash netcat). Windows users can run
# this from WSL against a native-Windows midas-app listening on the
# default port — localhost sockets cross the WSL ↔ Windows boundary.

set -euo pipefail

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"
FIXTURE="${1:-aapl_m1_rth}"
OUT_DIR="${OUT_DIR:-.devloop/parity}"
mkdir -p "$OUT_DIR"

LEGACY_PNG="$OUT_DIR/${FIXTURE}.legacy.png"
NEW_PNG="$OUT_DIR/${FIXTURE}.new.png"
DIFF_PNG="$OUT_DIR/${FIXTURE}.diff.png"

send() {
  printf '%s\n' "$1" | nc -w 5 "$HOST" "$PORT"
}

hr() {
  printf -- '---- %s ----\n' "$1"
}

# Slice 9a gate: if the `ChartBackend` config key is not a recognised
# command, bail with a clear message pointing at the blocked slice.
# Probe by attempting to dump the chart panels' backend field — a
# response containing `"backend":` proves slice 9a shipped.
probe_backend_field() {
  local dump
  dump=$(send '{"cmd":"dump_state","path":"charts"}' | head -c 4096 || true)
  if printf '%s' "$dump" | grep -q '"backend"'; then
    return 0
  fi
  return 1
}

hr "sanity"
send '{"cmd":"ping"}'
echo

if ! probe_backend_field; then
  cat <<EOF
parity harness is blocked on slice 9a — the runtime ChartBackend toggle
is not yet present. Once 9a lands, this script will:
  1. LoadFixture $FIXTURE
  2. Toggle every visible chart to backend=Legacy
  3. Screenshot to $LEGACY_PNG
  4. Toggle every visible chart to backend=New
  5. Screenshot to $NEW_PNG
  6. CompareImages path_a=$LEGACY_PNG path_b=$NEW_PNG diff_out=$DIFF_PNG
  7. Exit 0 iff ssim >= 0.995 and diff_fraction <= 0.002 (slice-0 gate)

Today, the CompareImages command itself IS wired — you can test it
against any two pre-rendered PNGs:
  echo '{"cmd":"compare_images","path_a":"a.png","path_b":"b.png","diff_out":null}' \
    | nc -w 5 $HOST $PORT
EOF
  exit 2
fi

# Full parity run — activated once slice 9a provides the backend toggle.

hr "load fixture $FIXTURE"
send "$(printf '{"cmd":"load_fixture","name":"%s"}' "$FIXTURE")"
echo

hr "screenshot legacy"
# TODO slice 9a: toggle panels to backend=Legacy first.
send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$LEGACY_PNG")"
echo

hr "screenshot new"
# TODO slice 9a: toggle panels to backend=New first.
send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$NEW_PNG")"
echo

hr "compare"
send "$(printf '{"cmd":"compare_images","path_a":"%s","path_b":"%s","diff_out":"%s"}' \
  "$LEGACY_PNG" "$NEW_PNG" "$DIFF_PNG")"
echo
