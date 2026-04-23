# Slice 12 — Docs + plan archive

**Goal.** Update project docs to reflect session-aware charts. Archive this plan.

## Scope

- `CLAUDE.md` (repo root): add Section "Session-aware charts" describing:
  - `midas-calendar` crate + trait.
  - `ExchangeCalendar` injection via router.subscribe_bars.
  - `SessionKind` on Bar + CandleData.
  - Rendering: tint + bands + separator.
  - Per-chart `show_extended_hours` toggle.
- `desktop/win/CLAUDE.md`: same refresh at the desktop-workspace layer.
- `README.md`: update the "Features" or "Architecture" section with a 2-line mention.
- Archive: move `plan/session-aware-charts/` → `plan/archive/session-aware-charts/`. Leave an index pointer.

## Files touched

- `CLAUDE.md`
- `desktop/win/CLAUDE.md`
- `README.md`
- `plan/session-aware-charts/*.md` → `plan/archive/session-aware-charts/`

## Commit

Single commit: `docs: session-aware charts + archive implementation plan`.
