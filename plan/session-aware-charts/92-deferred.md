# Explicitly Deferred

Work the plan deliberately does NOT ship. Each is flagged so you see what was considered and cut.

## Futures (CME Globex) support

- `XcmeCalendar` with ETH/RTH templates.
- 60-minute daily maintenance break (17:00–18:00 ET Mon–Thu).
- Daily settlement at 15:14:30–15:15:00 CT for ES (not the Globex close).
- Contract roll markers on continuous charts.

**Defer reason**: independent, non-trivial feature. The calendar trait surface is designed to admit it. Expected in a follow-up plan.

## Forex regional overlays

- `FxOtcCalendar` with Sydney/Tokyo/London/NY overlapping `Session.label` strings.
- Background shading for regional overlaps (e.g., London-NY overlap = both tints).
- 17:00 ET Sunday open, 17:00 ET Friday close.
- Session toggle (show/hide regional overlays).

**Defer reason**: same — separate feature. Calendar trait carries `label: Option<&'static str>` for exactly this.

## User-configurable session DSL

- Per-symbol session spec like TradingView's `0930-1600:23456,1700F-0900` format.
- Runtime parsing into a `TradingCalendar`.
- UI to edit.

**Defer reason**: MVP ships two hard-coded calendars (XNYS + CryptoSpot). DSL is a power-user feature. Add when there's demand.

## Index-based time axis rewrite

- trading-vue-js pattern: replace time axis with an integer index + `time↔index` map.
- Zero-gap rendering.
- Continuous-clock escape hatch toggle.

**Defer reason**: current collapsed-mode rendering handles gaps; a full rewrite is not justified by UX need.

## Session indicators

- Opening Range, Session VWAP, Session High/Low markers.
- Session-anchored volume profile.

**Defer reason**: these are indicators, not chart chrome. Belong to their own feature.

## Half-day-plus-full-day volume merge

- TradingView's optional behaviour to merge an early-close day's volume with the next day's to keep statistics comparable.

**Defer reason**: subtle analytical convention that confuses more users than it helps. Not a default.

## Multi-session per D1 (PRE + RTH + POST as separate daily bars)

- NT8's "multi-series" style: one D1 bar per session per day.

**Defer reason**: contradicts our "one contiguous stream" choice (decision A in 90-product-decisions.md). Re-open if users ask.

## Session-name labels on the chart body

- Text overlay like "PRE" / "RTH" / "POST" at session boundaries.

**Defer reason**: no platform does this as default. Time axis + background shading is information-sufficient. User indicators can add labels.

## Timezone-configurable display

- Chart setting "Display timezone" — re-label time axis in user tz while keeping calendar alignment in exchange tz.

**Defer reason**: time axis already UTC by default; exchange-tz labels are calendar's job. True tz customisation is a settings item that fits any time, not a structural gap.

## Holiday data past 2031

- Forward-dated holiday rules / forecast.

**Defer reason**: NYSE publishes ~2–3 years ahead. We cover 12 years (2020–2031). Annual patch cadence expected.

## Overnight / crypto session break

- User-configurable "daily close" for crypto (e.g., 17:00 ET CME-aligned break).

**Defer reason**: crypto defaults to UTC midnight; user override is a power-user feature.

---

If any of these jump up in priority, say so and we'll re-plan.
