# Volume Profile dev harness — preserved scripts

This doc preserves the four `devloop-vp-*.sh` harness scripts and the
screenshots reference index that lived on the `feat/vp-anchored`
branch but were not merged to `main` (the production code shipped in
commit `aa415db`). They drive the running app over the dev_harness
TCP socket (`127.0.0.1:9898`) to sweep VP anchor modes and capture
screenshots.

If you revive any of these, drop the script body into
`desktop/win/tools/<name>.sh`, `chmod +x`, and run from the
`desktop/win/` directory with the app booted under
`--features dev_harness` (and `session_chart` for the new-stack
script).

Reference PNGs were never committed (the manual GPU pass was deferred
when the branch was merged); without them, the harnesses still
capture screenshots but skip the SSIM compare step.

---

## Boot

```bash
# Terminal 1 — boot the app with the harness socket open.
cd desktop/win
cargo run -p midas-app --features dev_harness                  # legacy + fallback
cargo run -p midas-app --features "dev_harness session_chart"  # new + parity
```

Required fixture (under `tests/data/fixtures/`):
`vp_daily_aapl_5m_3days`. The fallback script also needs
`vp_collapse_gaps_aapl_5m_3days` and `vp_anchor_too_fine_spy_d1`.

---

## Per-stack legacy sweep — `devloop-vp-anchored-legacy.sh`

Sweeps every anchor mode (Viewport, Daily, Weekly, Monthly, Yearly)
on the legacy `midas-chart` stack, writes one PNG per mode under
`tests/data/screenshots/`, and SSIM-diffs each against its committed
reference (per-stack threshold ≥ 0.98).

```bash
#!/usr/bin/env bash
# Per-stack legacy Volume Profile screenshot harness.
#
# Sweeps every anchor mode (Viewport, Daily, Weekly, Monthly, Yearly) on
# the legacy `midas-chart` stack and writes one PNG per mode to
# `desktop/win/tests/data/screenshots/`. Each PNG is then diffed against
# the committed reference using the dev-harness `compare_images`
# command (SSIM ≥ 0.98 per per-stack threshold).
#
# Usage:
#   Terminal 1: cd desktop/win && cargo run -p midas-app --features dev_harness
#   Terminal 2: ./tools/devloop-vp-anchored-legacy.sh
#
# Reference-PNG regeneration (after a deliberate visual change):
#   MIDAS_DEVLOOP_REGENERATE=1 ./tools/devloop-vp-anchored-legacy.sh
# This overwrites the reference PNGs without running diffs. Commit the
# new PNGs and document why in the PR.
#
# Determinism notes:
#   - Window size is set by the fixture (1280x720). Override with
#     MIDAS_DEVLOOP_WINDOW_SIZE=WIDTHxHEIGHT before launching the app.
#   - WaitForIdle (timeout 500ms) drains the camera lerp before each
#     screenshot; the lerp on ticker switch is ~300ms.
#
# Requires: bash, nc (Git Bash netcat or WSL ncat), the
# `vp_daily_aapl_5m_3days` fixture under `tests/data/fixtures/`.

set -euo pipefail

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"
FIXTURE="${1:-vp_daily_aapl_5m_3days}"
SHOTS_DIR="${SHOTS_DIR:-tests/data/screenshots}"
REGEN="${MIDAS_DEVLOOP_REGENERATE:-0}"
mkdir -p "$SHOTS_DIR"

send() { printf '%s\n' "$1" | nc -w 5 "$HOST" "$PORT"; }
hr()   { printf -- '---- %s ----\n' "$1"; }

# Anchor sweep — one PNG per mode.
ANCHORS=(viewport daily weekly monthly yearly)

hr "sanity"
send '{"cmd":"ping"}'
echo
send "$(printf '{"cmd":"load_fixture","name":"%s"}' "$FIXTURE")"
echo

for anchor in "${ANCHORS[@]}"; do
  hr "set anchor=$anchor"
  case "$anchor" in
    viewport) anchor_name="Viewport" ;;
    daily)    anchor_name="Daily" ;;
    weekly)   anchor_name="Weekly" ;;
    monthly)  anchor_name="Monthly" ;;
    yearly)   anchor_name="Yearly" ;;
  esac
  send "$(printf '{"cmd":"set_vp_settings","chart_id":0,"settings_json":{"anchor":"%s","width_fraction":0.7}}' "$anchor_name")"
  echo
  send '{"cmd":"wait_for_idle","timeout_ms":500}'
  echo

  shot="$SHOTS_DIR/vp-legacy-$anchor.png"
  hr "screenshot $shot"
  send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$shot")"
  echo

  if [[ "$REGEN" != "1" ]]; then
    ref="$SHOTS_DIR/vp-legacy-$anchor.png"
    if [[ -f "$ref" ]]; then
      hr "compare $anchor against ref"
      send "$(printf '{"cmd":"compare_images","path_a":"%s","path_b":"%s","diff_out":null}' "$shot" "$ref")"
      echo
    else
      echo "WARN: reference $ref missing — first run, regenerate with MIDAS_DEVLOOP_REGENERATE=1"
    fi
  fi
done

echo
echo "Legacy VP sweep complete. Per-stack threshold: SSIM >= 0.98."
```

---

## Per-stack new-stack sweep — `devloop-vp-anchored-new.sh`

Same layout as the legacy sweep, but flips the panel to `backend=New`
via `ToggleChartBackend` first. Requires `--features session_chart` on
the running app.

```bash
#!/usr/bin/env bash
# Per-stack new (session-aware) Volume Profile screenshot harness.
#
# Same layout as `devloop-vp-anchored-legacy.sh`, but with the panel
# flipped to backend=New via `ToggleChartBackend` first. Requires
# `--features session_chart` on the running app:
#
#   Terminal 1: cd desktop/win && cargo run -p midas-app --features "dev_harness session_chart"
#   Terminal 2: ./tools/devloop-vp-anchored-new.sh
#
# Reference-PNG regeneration:
#   MIDAS_DEVLOOP_REGENERATE=1 ./tools/devloop-vp-anchored-new.sh
#
# Per-stack threshold: SSIM >= 0.98.

set -euo pipefail

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"
FIXTURE="${1:-vp_daily_aapl_5m_3days}"
SHOTS_DIR="${SHOTS_DIR:-tests/data/screenshots}"
REGEN="${MIDAS_DEVLOOP_REGENERATE:-0}"
mkdir -p "$SHOTS_DIR"

send() { printf '%s\n' "$1" | nc -w 5 "$HOST" "$PORT"; }
hr()   { printf -- '---- %s ----\n' "$1"; }

ANCHORS=(viewport daily weekly monthly yearly)

hr "sanity"
send '{"cmd":"ping"}'
echo
send "$(printf '{"cmd":"load_fixture","name":"%s"}' "$FIXTURE")"
echo

# Flip backend to New for chart 0.
hr "toggle backend=New on chart 0"
send '{"cmd":"toggle_chart_backend","chart_id":0}'
echo
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

# Probe `"backend":"New"` to confirm slice 9a + session_chart feature.
dump=$(send '{"cmd":"dump_state","path":"charts.0.backend"}' | head -c 256 || true)
if ! printf '%s' "$dump" | grep -qi 'new'; then
  echo "ERROR: backend did not flip to New. Confirm --features session_chart enabled."
  exit 2
fi

for anchor in "${ANCHORS[@]}"; do
  hr "set anchor=$anchor"
  case "$anchor" in
    viewport) anchor_name="Viewport" ;;
    daily)    anchor_name="Daily" ;;
    weekly)   anchor_name="Weekly" ;;
    monthly)  anchor_name="Monthly" ;;
    yearly)   anchor_name="Yearly" ;;
  esac
  send "$(printf '{"cmd":"set_vp_settings","chart_id":0,"settings_json":{"anchor":"%s","width_fraction":0.7}}' "$anchor_name")"
  echo
  send '{"cmd":"wait_for_idle","timeout_ms":500}'
  echo

  shot="$SHOTS_DIR/vp-new-$anchor.png"
  hr "screenshot $shot"
  send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$shot")"
  echo

  if [[ "$REGEN" != "1" ]]; then
    ref="$SHOTS_DIR/vp-new-$anchor.png"
    if [[ -f "$ref" ]]; then
      hr "compare $anchor against ref"
      send "$(printf '{"cmd":"compare_images","path_a":"%s","path_b":"%s","diff_out":null}' "$shot" "$ref")"
      echo
    else
      echo "WARN: reference $ref missing — first run, regenerate with MIDAS_DEVLOOP_REGENERATE=1"
    fi
  fi
done

echo
echo "New-stack VP sweep complete. Per-stack threshold: SSIM >= 0.98."
```

---

## Cross-stack parity — `devloop-vp-cross-stack-parity.sh`

Renders the same fixture through both backends (legacy, then new) and
SSIM-diffs them. Threshold is deliberately looser than per-stack
(≥ 0.85) — different pipelines, fonts, palettes. Checks structural
similarity of per-period histograms and POC y-positions, not pixel
equality.

```bash
#!/usr/bin/env bash
# Cross-stack Volume Profile parity harness (S5).
#
# Captures the same fixture rendered through both backends (legacy,
# then new) and asserts visual structural similarity. Threshold is
# deliberately looser than per-stack tests (SSIM >= 0.85) — the two
# stacks render with different pipelines, fonts, and color palettes.
# We're checking that per-period histogram counts and POC y-positions
# match within tolerance, not pixel equality.
#
# Usage:
#   Terminal 1: cd desktop/win && cargo run -p midas-app --features "dev_harness session_chart"
#   Terminal 2: ./tools/devloop-vp-cross-stack-parity.sh
#
# Output: writes both PNGs and a diff PNG under `.devloop/parity/`,
# and exits 0 iff `compare_images` returns SSIM >= 0.85.

set -euo pipefail

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"
FIXTURE="${1:-vp_daily_aapl_5m_3days}"
OUT_DIR="${OUT_DIR:-.devloop/parity}"
mkdir -p "$OUT_DIR"

LEGACY_PNG="$OUT_DIR/vp-${FIXTURE}.legacy.png"
NEW_PNG="$OUT_DIR/vp-${FIXTURE}.new.png"
DIFF_PNG="$OUT_DIR/vp-${FIXTURE}.diff.png"

send() { printf '%s\n' "$1" | nc -w 5 "$HOST" "$PORT"; }
hr()   { printf -- '---- %s ----\n' "$1"; }

hr "sanity"
send '{"cmd":"ping"}'
echo
send "$(printf '{"cmd":"load_fixture","name":"%s"}' "$FIXTURE")"
echo

# Render under Legacy backend.
hr "set backend=Legacy and anchor=Daily"
# Probing for the current backend keeps the script idempotent — if the
# fixture happens to load with backend=New, we toggle once; otherwise
# we leave it alone.
dump=$(send '{"cmd":"dump_state","path":"charts.0.backend"}' | head -c 256 || true)
if printf '%s' "$dump" | grep -qi 'new'; then
  send '{"cmd":"toggle_chart_backend","chart_id":0}'
  echo
fi
send '{"cmd":"set_vp_settings","chart_id":0,"settings_json":{"anchor":"Daily","width_fraction":0.7}}'
echo
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

hr "screenshot legacy"
send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$LEGACY_PNG")"
echo

# Flip to New backend.
hr "toggle backend=New"
send '{"cmd":"toggle_chart_backend","chart_id":0}'
echo
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

hr "screenshot new"
send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$NEW_PNG")"
echo

# Cross-stack parity threshold is SSIM >= 0.85 (per S5 plan §SSIM
# threshold). The slice-0 `compare_images` command emits ssim +
# diff_fraction in its response; the calling shell parses and asserts.
hr "compare cross-stack"
send "$(printf '{"cmd":"compare_images","path_a":"%s","path_b":"%s","diff_out":"%s"}' \
  "$LEGACY_PNG" "$NEW_PNG" "$DIFF_PNG")"
echo

echo
echo "Cross-stack parity threshold: SSIM >= 0.85."
echo "Inspect $DIFF_PNG to spot per-period or POC drift."
```

---

## Fallback case — `devloop-vp-fallback-cases.sh`

After S6 P6 (collapse_gaps fix), only one fallback case remains:
**D12** — anchor=Daily on a D1 timeframe (anchor too fine for the
chart's bar period). Renders identically to plain Viewport mode.

```bash
#!/usr/bin/env bash
# Volume Profile fallback-case harness (S5).
#
# Verifies the silent-fallback path renders identically to plain
# Viewport mode:
#
#   - D12: anchor=Daily on a D1 timeframe — the anchor period is too
#     fine for the chart's bar period.
#
# Should render exactly like Viewport mode. The reference PNG
# (`vp-anchor-too-fine.png`) should match the corresponding stack's
# `vp-legacy-viewport.png` / `vp-new-viewport.png`.
#
# S6 P6: D11 (collapse_gaps fallback) is gone — anchored mode now
# renders correctly under collapse_gaps. The previously planned
# `vp-collapse-gaps-fallback.png` reference is retired.
#
# Usage:
#   Terminal 1: cd desktop/win && cargo run -p midas-app --features "dev_harness session_chart"
#   Terminal 2: ./tools/devloop-vp-fallback-cases.sh

set -euo pipefail

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"
SHOTS_DIR="${SHOTS_DIR:-tests/data/screenshots}"
REGEN="${MIDAS_DEVLOOP_REGENERATE:-0}"
mkdir -p "$SHOTS_DIR"

send() { printf '%s\n' "$1" | nc -w 5 "$HOST" "$PORT"; }
hr()   { printf -- '---- %s ----\n' "$1"; }

# D12 — anchor=Daily on a D1 timeframe.
hr "fixture vp_anchor_too_fine_spy_d1"
send '{"cmd":"load_fixture","name":"vp_anchor_too_fine_spy_d1"}'
echo
send '{"cmd":"set_vp_settings","chart_id":0,"settings_json":{"anchor":"Daily","width_fraction":0.7}}'
echo
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo
shot="$SHOTS_DIR/vp-anchor-too-fine.png"
hr "screenshot $shot"
send "$(printf '{"cmd":"screenshot","out_path":"%s"}' "$shot")"
echo

if [[ "$REGEN" != "1" ]]; then
  viewport="$SHOTS_DIR/vp-legacy-viewport.png"
  if [[ -f "$viewport" ]]; then
    hr "compare D12 fallback against Viewport reference"
    send "$(printf '{"cmd":"compare_images","path_a":"%s","path_b":"%s","diff_out":null}' \
      "$shot" "$viewport")"
    echo
  fi
fi

echo
echo "Fallback-case sweep complete. PNG should match Viewport reference (SSIM >= 0.99)."
```

---

## Reference PNG index

Per-stack sweep — six per stack:

| File | Anchor | Stack |
|------|--------|-------|
| `vp-legacy-viewport.png` | Viewport | Legacy |
| `vp-legacy-daily.png`    | Daily    | Legacy |
| `vp-legacy-weekly.png`   | Weekly   | Legacy |
| `vp-legacy-monthly.png`  | Monthly  | Legacy |
| `vp-legacy-yearly.png`   | Yearly   | Legacy |
| `vp-new-viewport.png`    | Viewport | New    |
| `vp-new-daily.png`       | Daily    | New    |
| `vp-new-weekly.png`      | Weekly   | New    |
| `vp-new-monthly.png`     | Monthly  | New    |
| `vp-new-yearly.png`      | Yearly   | New    |

Fallback cases:

| File | Anchor | Notes |
|------|--------|-------|
| `vp-anchor-too-fine.png` | Daily on D1 | D12 — should match `vp-legacy-viewport.png` |

After S6 P6, `vp-collapse-gaps-fallback.png` is no longer relevant
(anchored mode renders correctly under collapse_gaps).

### Validation thresholds

- Per-stack: SSIM ≥ 0.98 (near-pixel-exact).
- Cross-stack parity: SSIM ≥ 0.85 (different pipelines, fonts,
  palettes — checks structural similarity).

### Determinism notes

- Window 1280×720, primary monitor at 100% DPI.
- `WaitForIdle --timeout-ms 500` before every screenshot to drain the
  ~300 ms camera lerp.
- Cursor moved off-chart before capture (avoid hover highlights).

### CI

Headless CI cannot run these — they need a real GPU and window
manager. Manual pre-PR gate until a GPU CI runner exists.

### Reviving

To bring the scripts back as live tools:

1. Recreate the four files at `desktop/win/tools/devloop-vp-*.sh`
   from the code blocks above.
2. `chmod +x desktop/win/tools/devloop-vp-*.sh`.
3. If you also want committed reference PNGs, add this unignore to
   `.gitignore` (already present in `main` from S5):
   ```
   !**/tests/data/
   !**/tests/data/screenshots/
   !**/tests/data/screenshots/**
   ```
4. Run with `MIDAS_DEVLOOP_REGENERATE=1` to seed fresh PNGs, visually
   verify, then commit.
