//! Integration: 100 historical fixture bars chained with a 100-bar
//! `ChannelBarStream`. Drain the composite via `next()` and verify
//! total count and strictly increasing timestamps with no duplicates.
//!
//! Note on seam policy: per bug-hunt H3 (see
//! `plan/session-aware-charts/99-diagnostic-findings-r2.md`), the
//! seam dedup only drops bars whose `window.open` is *strictly*
//! before the seam. Live bars AT the seam are forwarded (they carry
//! Partial refreshes). This test's live tail starts strictly AFTER
//! the last history bar, so both the old `<=` semantic and the new
//! `<` semantic produce the same output. A dedicated unit test in
//! `combinator_tests.rs::history_then_live_forwards_seam_refresh_but_drops_stale`
//! covers the seam-refresh behaviour change explicitly.

use chrono::TimeZone;
use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, BarPeriod, Timestamp};
use tokio::sync::mpsc;

use midas_stream::{BarStream, BarStreamMeta, ChannelBarStream, FixtureBarStream, HistoryThenLive};

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

fn m1_candle(ts: Timestamp, sym: Symbol) -> Candle {
    let cal = xnys();
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(100.0, 100.5, 99.5, 100.0, 1_000, 10, None).unwrap();
    Candle::new(
        sym,
        cal,
        BarPeriod::m1(),
        session,
        window,
        ohlcv,
        Completeness::Completed,
    )
    .unwrap()
}

/// Produce `n` contiguous M1 bars starting at `start_ts`, aligned to
/// the calendar's bar_window (skips any closed-time gaps).
fn m1_range(start_ts: Timestamp, n: usize, sym: Symbol) -> Vec<Candle> {
    let mut out = Vec::with_capacity(n);
    let mut ts = start_ts;
    while out.len() < n {
        let window = xnys().bar_window(ts, BarPeriod::m1()).unwrap();
        out.push(m1_candle(window.open, sym));
        ts = window.open + chrono::Duration::minutes(1);
    }
    out
}

#[tokio::test]
async fn history_then_live_200_bars_no_dupe() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30); // 09:30 ET, Regular open.

    // 100 historical bars.
    let hist_bars = m1_range(start, 100, sym);
    assert_eq!(hist_bars.len(), 100);
    let last_hist_ts = hist_bars.last().unwrap().window.open;

    // 100 live bars, starting strictly after the last history bar.
    let live_start = last_hist_ts + chrono::Duration::minutes(1);
    let live_bars = m1_range(live_start, 100, sym);
    assert_eq!(live_bars.len(), 100);

    let meta = BarStreamMeta::new(sym, xnys(), BarPeriod::m1());
    let hist = FixtureBarStream::new(meta.clone(), hist_bars.clone()).unwrap();

    let (tx, rx) = mpsc::channel(256);
    for c in &live_bars {
        tx.send(c.clone()).await.unwrap();
    }
    drop(tx);
    let live = ChannelBarStream::new(meta.clone(), rx);

    let mut chained = HistoryThenLive::new(meta, hist, live);

    let mut got = Vec::with_capacity(200);
    while let Some(c) = chained.next().await {
        got.push(c.window.open);
    }

    // Count.
    assert_eq!(got.len(), 200, "total bars should be 200");

    // Strictly increasing — no dupes.
    for w in got.windows(2) {
        assert!(
            w[0] < w[1],
            "timestamps must be strictly increasing; got {} then {}",
            w[0],
            w[1],
        );
    }

    // Explicit dedup check.
    let mut sorted = got.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        got.len(),
        "no duplicate timestamps at the seam"
    );
}
