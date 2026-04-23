//! Integration: build ~two days of XNYS M1 bars spanning
//! pre-market / RTH / post-market, wrap the stream with
//! `Filtered<_, EhFilter::RTH_ONLY>`, snapshot the full range, and
//! verify the result contains only `Regular` bars.

use chrono::TimeZone;
use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, BarPeriod, SessionKind, Timestamp};

use midas_stream::{BarStream, BarStreamMeta, EhFilter, Filtered, FixtureBarStream, TimeRange};

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

/// Generate every M1 bar the XNYS calendar reports across two trading
/// days (2024-01-17 and 2024-01-18), covering pre / RTH / post.
fn two_days_of_m1_bars(sym: Symbol) -> Vec<Candle> {
    let cal = xnys();
    // XNYS pre-market opens at 04:00 ET = 09:00 UTC (winter).
    // Iterate minute-by-minute from 2024-01-17 09:00 UTC through
    // 2024-01-19 01:30 UTC (past post-market close for 1/18).
    let start = utc(2024, 1, 17, 9, 0);
    let end = utc(2024, 1, 19, 1, 30);

    let mut out = Vec::new();
    let mut ts = start;
    let step = chrono::Duration::minutes(1);

    while ts < end {
        let session = cal.classify(ts);
        if session.kind() != SessionKind::Closed {
            // Align to the bar_window.open so we don't double-enter
            // inside a single bar (M1 is trivially aligned to the
            // minute, but be explicit).
            let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
            if out
                .last()
                .map(|c: &Candle| c.window.open != window.open)
                .unwrap_or(true)
            {
                out.push(m1_candle(window.open, sym));
            }
        }
        ts += step;
    }
    out
}

#[tokio::test]
async fn filtered_snapshot_returns_only_rth_bars() {
    let sym = Symbol::new("SPY", xnys().id());
    let all_bars = two_days_of_m1_bars(sym);
    assert!(!all_bars.is_empty(), "fixture must have bars");
    // Sanity: the raw fixture must contain at least one bar of each of
    // PreMarket, Regular, PostMarket so the filter is actually doing work.
    let mut seen_pre = false;
    let mut seen_reg = false;
    let mut seen_post = false;
    for c in &all_bars {
        match c.session.kind() {
            SessionKind::PreMarket => seen_pre = true,
            SessionKind::Regular => seen_reg = true,
            SessionKind::PostMarket => seen_post = true,
            _ => {}
        }
    }
    assert!(
        seen_pre && seen_reg && seen_post,
        "fixture missing a session tag: pre={seen_pre} reg={seen_reg} post={seen_post}",
    );

    let expected_regular = all_bars
        .iter()
        .filter(|c| c.session.kind() == SessionKind::Regular)
        .count();

    let meta = BarStreamMeta::new(sym, xnys(), BarPeriod::m1());
    let inner = FixtureBarStream::new(meta, all_bars.clone()).unwrap();
    let mut s = Filtered::new(inner, EhFilter::RTH_ONLY);

    let first = all_bars.first().unwrap().window.open;
    let last = all_bars.last().unwrap().window.open + chrono::Duration::minutes(1);
    let snap = s
        .snapshot(TimeRange::new(first, last).unwrap())
        .await
        .unwrap();

    assert_eq!(
        snap.len(),
        expected_regular,
        "Filtered::snapshot must equal raw Regular-count"
    );
    for c in &snap {
        assert_eq!(c.session.kind(), SessionKind::Regular);
    }
}
