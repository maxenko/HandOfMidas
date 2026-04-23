# Product Decisions Baked Into This Plan

Decisions I made autonomously per auto-mode, synthesising research from 4 agents (pro-platform UX, Rust calendar crates, 24h instrument handling, deep-map of midas-chart). If you want any changed, say which.

## A. Bar model for extended hours

**Choice: One contiguous intraday stream.** A 5-minute bar at 09:25 ET carries `SessionKind::PreMarket`; at 09:30 ET next bar is `Regular`. No per-session sub-streams.

**Why**: TradingView, ToS, IBKR TWS, Sierra Chart all converge on this. Splitting per session breaks indicator continuity and zoom interaction.

**Alternative rejected**: Three separate bars per day (PRE/RTH/POST). Only NT8 + MultiCharts expose this via "multi-series" and users generally find it confusing.

## B. Visual delineation

**Choice: Background shading + RTH-close vertical separator.** Candles get per-session RGB tinting (pre 65% brightness, post 55% brightness).

**Why**: TradingView model. Low cognitive load. Reuses existing rendering infrastructure (GridLineInstance pipeline, CandleInstance.color attribute).

**Alternative rejected**: Different bar styles (border thickness, stroke color). No platform does this; feedback would be noisy.

## C. D1 bar aggregation rule

**Choice: Session-aligned (09:30–16:00 ET for XNYS, `use_rth=true` default).** Daily bar aggregates RTH ticks only by default.

**Why**: TWS default, Bloomberg convention. Matches feed.

**Alternative**: Include extended hours in daily OHLC via a toggle. Deferred — easy to add later.

## D. Daily bar on early-close days

**Choice: Honour the exchange's declared calendar verbatim.** Black Friday D1 bar is 09:30–13:00 ET.

**Why**: Any merge-into-adjacent-day rule confuses more users than it helps. TV tries the merge; community backlash ample.

## E. Session toggle location

**Choice: Bottom-right chip ("EH" button) on the chart, plus settings checkbox.** Per-chart state, persisted via ChartViewStore.

**Why**: Matches TV + IBKR. Chip allows one-click toggle without opening a menu.

## F. 24h instruments

**Choice: `CryptoSpotCalendar` with `TimeAxisPolicy::Continuous`, `Mic("CRYPTO")`.** UTC-aligned D1 close. No session chrome. Chip hidden.

**Why**: Binance/Coinbase/Kraken + TV all converge on UTC midnight for spot crypto. Regional-session overlays are future work.

**Alternative rejected**: NY-close 17:00 ET roll (forex convention). Crypto ≠ forex. Users would not expect this.

## G. Futures (CME Globex)

**Choice: Deferred.** Calendar surface admits `XcmeCalendar` later; MVP doesn't ship it.

**Why**: Non-trivial — ETH vs RTH templates, 60-minute maintenance break, daily roll at 17:00 ET with 18:00 ET re-open. Independent feature worth its own plan.

## H. Forex

**Choice: Deferred.** 24/5 with three overlapping regional sessions (Sydney/Tokyo/London/NY). Labels on `Session.label` field; rendering is future.

**Why**: Same — separate feature.

## I. Rollover markers on continuous futures contracts

**Choice: Deferred.** Not part of this plan.

## J. Calendar-data maintenance strategy

**Choice: Hard-coded tables in-repo, cross-checked against `nyse-holiday-cal` in tests.** Coverage 2000–2031.

**Why**: Determinism for backtests; offline-capable; annual patch cadence is manageable. `nyse-holiday-cal` stays a dev-dep only.

## K. Session wire format

**Choice: Hard-coded Rust `TradingCalendar` structs for MVP.**

**Why**: TV-style session DSL (`0930-1600:23456`) is a nice future-extension but premature.

## L. Holiday early-close policy

**Choice: Shortened regular session 09:30–13:00 ET, post-market shortens to 13:00–17:00 ET.**

**Why**: Matches NYSE's own published schedule and TWS behavior.

## M. Cross-check data source

**Choice: `nyse-holiday-cal` crate as a dev-dep cross-check.** Not a runtime dep.

**Why**: Independent verification for free. Runtime calendar is our own.

## Open questions (defer to user)

None that block. But if you have strong opinions on any of A–M, now is the time.
