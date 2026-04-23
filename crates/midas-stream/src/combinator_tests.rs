//! Unit tests for the stream combinators. Lives next to the modules in
//! `src/` so tests can reach `pub(crate)` helpers if needed.

use chrono::TimeZone;
use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, BarPeriod, SessionKind, Timestamp};
use tokio::sync::mpsc;

use crate::{
    BarStream, BarStreamMeta, ChannelBarStream, EhFilter, Filtered, FixtureBarStream,
    HistoryThenLive, SeekableBarStream, SessionKindFilter, StreamError, TimeRange,
};

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

/// Build a single XNYS M1 candle at UTC `ts`. Price 100/100.5/99.5/100.
fn xnys_m1_candle(ts: Timestamp, symbol: Symbol) -> Candle {
    let cal = xnys();
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(100.0, 100.5, 99.5, 100.0, 1_000, 10, None).unwrap();
    Candle::new(
        symbol,
        cal,
        BarPeriod::m1(),
        session,
        window,
        ohlcv,
        Completeness::Completed,
    )
    .unwrap()
}

/// Generate a contiguous M1 sequence, `n` bars starting at `start_ts`
/// stepping by 60s. Gaps through closed time are skipped by realigning
/// to the calendar's bar_window, so the test fixture only contains bars
/// that actually exist on the calendar.
fn xnys_m1_range(start_ts: Timestamp, n: usize, symbol: Symbol) -> Vec<Candle> {
    let mut out = Vec::with_capacity(n);
    let mut ts = start_ts;
    let step = chrono::Duration::minutes(1);
    while out.len() < n {
        // Use the window-open rather than raw `ts` to avoid alignment drift.
        let window = xnys().bar_window(ts, BarPeriod::m1()).unwrap();
        out.push(xnys_m1_candle(window.open, symbol));
        ts = window.open + step;
    }
    out
}

fn xnys_m1_meta(symbol: Symbol) -> BarStreamMeta {
    BarStreamMeta::new(symbol, xnys(), BarPeriod::m1())
}

// ---------------------------------------------------------------------------
// FixtureBarStream — `next()`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_next_yields_in_order() {
    let sym = Symbol::new("SPY", xnys().id());
    // 2024-01-17 14:30 UTC == 09:30 ET (Regular open).
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 5, sym);
    let expected: Vec<_> = bars.iter().map(|c| c.window.open).collect();

    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    let mut got = Vec::new();
    while let Some(c) = s.next().await {
        got.push(c.window.open);
    }
    assert_eq!(got, expected);
}

#[tokio::test]
async fn fixture_next_returns_none_after_last() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 3, sym);
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    assert!(s.next().await.is_some());
    assert!(s.next().await.is_some());
    assert!(s.next().await.is_some());
    assert!(s.next().await.is_none());
    assert!(s.next().await.is_none(), "repeated poll stays None");
}

// ---------------------------------------------------------------------------
// FixtureBarStream — `seek()`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_seek_jumps_cursor() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 10, sym);
    let target = bars[4].window.open;
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars.clone()).unwrap();
    s.seek(target).await.unwrap();
    let n = s.next().await.unwrap();
    assert_eq!(n.window.open, target);
}

#[tokio::test]
async fn fixture_seek_before_start_yields_from_start() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 5, sym);
    let first = bars[0].window.open;
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    // Seek to 1 hour before first bar.
    s.seek(first - chrono::Duration::hours(1)).await.unwrap();
    let n = s.next().await.unwrap();
    assert_eq!(n.window.open, first);
}

#[tokio::test]
async fn fixture_seek_after_end_yields_none() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 5, sym);
    let last = bars.last().unwrap().window.open;
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    s.seek(last + chrono::Duration::hours(1)).await.unwrap();
    assert!(s.next().await.is_none());
}

// ---------------------------------------------------------------------------
// FixtureBarStream — `snapshot`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_snapshot_respects_half_open() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 5, sym);
    let first = bars[0].window.open;
    let third = bars[2].window.open;
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars.clone()).unwrap();
    let snap = s
        .snapshot(TimeRange::new(first, third).unwrap())
        .await
        .unwrap();
    // Half-open: should contain indices 0 and 1 (not 2).
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].window.open, first);
    assert_eq!(snap[1].window.open, bars[1].window.open);
}

#[tokio::test]
async fn fixture_snapshot_does_not_advance_cursor() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 5, sym);
    let first = bars[0].window.open;
    let last_plus_1 = bars.last().unwrap().window.open + chrono::Duration::minutes(1);
    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars.clone()).unwrap();
    let _ = s
        .snapshot(TimeRange::new(first, last_plus_1).unwrap())
        .await
        .unwrap();
    assert_eq!(s.cursor(), 0, "snapshot must not advance cursor");
    // next() still yields the first bar.
    let n = s.next().await.unwrap();
    assert_eq!(n.window.open, first);
}

// ---------------------------------------------------------------------------
// FixtureBarStream — validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_rejects_wrong_period() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let bars = xnys_m1_range(start, 3, sym);
    // Meta declares m5; fixture bars are m1.
    let bad_meta = BarStreamMeta::new(sym, xnys(), BarPeriod::m5());
    let err = FixtureBarStream::new(bad_meta, bars).unwrap_err();
    assert!(matches!(err, StreamError::Upstream(_)));
}

#[tokio::test]
async fn fixture_rejects_unsorted() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let mut bars = xnys_m1_range(start, 3, sym);
    bars.swap(0, 2);
    let err = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap_err();
    assert!(matches!(err, StreamError::Upstream(_)));
}

// ---------------------------------------------------------------------------
// ChannelBarStream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_next_awaits_and_returns_none_on_close() {
    let sym = Symbol::new("SPY", xnys().id());
    let (tx, rx) = mpsc::channel(4);
    let mut s = ChannelBarStream::new(xnys_m1_meta(sym), rx);

    let start = utc(2024, 1, 17, 14, 30);
    let c1 = xnys_m1_candle(start, sym);
    let c2 = xnys_m1_candle(start + chrono::Duration::minutes(1), sym);

    tx.send(c1.clone()).await.unwrap();
    tx.send(c2.clone()).await.unwrap();
    drop(tx);

    let got1 = s.next().await.unwrap();
    let got2 = s.next().await.unwrap();
    let got3 = s.next().await;

    assert_eq!(got1.window.open, c1.window.open);
    assert_eq!(got2.window.open, c2.window.open);
    assert!(got3.is_none());
}

#[tokio::test]
async fn channel_snapshot_is_not_seekable() {
    let sym = Symbol::new("SPY", xnys().id());
    let (_tx, rx) = mpsc::channel(1);
    let mut s = ChannelBarStream::new(xnys_m1_meta(sym), rx);
    let range = TimeRange::new(utc(2024, 1, 17, 0, 0), utc(2024, 1, 18, 0, 0)).unwrap();
    let err = s.snapshot(range).await.unwrap_err();
    assert_eq!(err, StreamError::NotSeekable);
}

// ---------------------------------------------------------------------------
// HistoryThenLive
// ---------------------------------------------------------------------------

#[tokio::test]
async fn history_then_live_drains_history_then_live() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let hist_bars = xnys_m1_range(start, 5, sym);
    let hist = FixtureBarStream::new(xnys_m1_meta(sym), hist_bars.clone()).unwrap();

    let (tx, rx) = mpsc::channel(8);
    let live = ChannelBarStream::new(xnys_m1_meta(sym), rx);

    let mut chained = HistoryThenLive::new(xnys_m1_meta(sym), hist, live);

    // Pre-arm the live stream with two more bars, strictly after history.
    let live1_ts = hist_bars.last().unwrap().window.open + chrono::Duration::minutes(1);
    let live2_ts = live1_ts + chrono::Duration::minutes(1);
    tx.send(xnys_m1_candle(live1_ts, sym)).await.unwrap();
    tx.send(xnys_m1_candle(live2_ts, sym)).await.unwrap();
    drop(tx);

    let mut got = Vec::new();
    while let Some(c) = chained.next().await {
        got.push(c.window.open);
    }
    assert_eq!(got.len(), 7);
    // History first, in order.
    assert_eq!(got[0], hist_bars[0].window.open);
    assert_eq!(got[4], hist_bars[4].window.open);
    // Then live.
    assert_eq!(got[5], live1_ts);
    assert_eq!(got[6], live2_ts);
}

#[tokio::test]
async fn history_then_live_forwards_seam_refresh_but_drops_stale() {
    // Bug-hunt H3 regression. Previously the seam dedup was `<=`,
    // which silently dropped a live bar whose window.open equalled
    // the last history bar's open. That erased the Partial refresh
    // delivered by the live aggregator once the seam bar reopened.
    // The fix uses strict `<`: only bars STRICTLY before the seam are
    // dropped; same-open live bars are forwarded (downstream
    // `CandleSeries::apply` overwrites the row by open-ts, so no
    // duplicate ever reaches storage). A strictly-older bar is still
    // suppressed.
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let hist_bars = xnys_m1_range(start, 3, sym);
    let last_hist_ts = hist_bars.last().unwrap().window.open;
    let pre_seam_ts = hist_bars.first().unwrap().window.open; // strictly older than seam
    let hist = FixtureBarStream::new(xnys_m1_meta(sym), hist_bars.clone()).unwrap();

    let (tx, rx) = mpsc::channel(8);
    let live = ChannelBarStream::new(xnys_m1_meta(sym), rx);
    let mut chained = HistoryThenLive::new(xnys_m1_meta(sym), hist, live);

    // Live side sends a stale pre-seam bar (dropped), a same-open
    // seam refresh (forwarded), and a fresh forward bar.
    tx.send(xnys_m1_candle(pre_seam_ts, sym)).await.unwrap();
    tx.send(xnys_m1_candle(last_hist_ts, sym)).await.unwrap();
    let fwd_ts = last_hist_ts + chrono::Duration::minutes(1);
    tx.send(xnys_m1_candle(fwd_ts, sym)).await.unwrap();
    drop(tx);

    let mut got = Vec::new();
    while let Some(c) = chained.next().await {
        got.push(c.window.open);
    }
    // History (3) + seam refresh (1, same-open live bar) + forward (1).
    // pre_seam_ts is strictly older than the seam → dropped.
    assert_eq!(got.len(), hist_bars.len() + 2);
    assert_eq!(got.last().copied(), Some(fwd_ts));
    // The seam-refresh bar is forwarded, so `last_hist_ts` appears
    // TWICE in the emit order (once from history, once from live).
    // Downstream CandleSeries::apply overwrites the row on open-ts,
    // so no duplicate reaches storage — only the transport sees it.
    assert_eq!(got.iter().filter(|&&ts| ts == last_hist_ts).count(), 2);
    // No stale bar ever forwarded.
    assert!(
        !got.contains(&pre_seam_ts) || got.iter().filter(|&&ts| ts == pre_seam_ts).count() == 1,
        "pre_seam_ts should only appear from history, never from live"
    );
}

#[tokio::test]
async fn history_then_live_try_seek_active_then_fails_after_drain() {
    let sym = Symbol::new("SPY", xnys().id());
    let start = utc(2024, 1, 17, 14, 30);
    let hist_bars = xnys_m1_range(start, 5, sym);
    let mid_ts = hist_bars[3].window.open;
    let hist = FixtureBarStream::new(xnys_m1_meta(sym), hist_bars.clone()).unwrap();

    // Close the live sender up front so that once history is drained
    // the live leg immediately returns `None` and `next()` terminates
    // rather than blocking forever on an open-but-empty channel.
    let (tx, rx) = mpsc::channel::<Candle>(4);
    drop(tx);
    let live = ChannelBarStream::new(xnys_m1_meta(sym), rx);
    let mut chained = HistoryThenLive::new(xnys_m1_meta(sym), hist, live);

    // Seek works while history is present.
    chained.try_seek(mid_ts).await.unwrap();
    let first = chained.next().await.unwrap();
    assert_eq!(first.window.open, mid_ts);
    assert!(
        chained.history_active(),
        "history still active after one bar"
    );

    // Drain the rest of history. The first `None` from `next()` signals
    // history has flipped off AND the closed live side returned None.
    let mut drained: usize = 1;
    while chained.next().await.is_some() {
        drained += 1;
    }
    assert_eq!(drained, hist_bars.len() - 3);
    assert!(
        !chained.history_active(),
        "history should be drained once next() returns None"
    );

    // Now seek must fail.
    let err = chained.try_seek(mid_ts).await.unwrap_err();
    assert_eq!(err, StreamError::NotSeekable);
}

// ---------------------------------------------------------------------------
// Filtered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filtered_eh_drops_pre_market_bars() {
    let sym = Symbol::new("SPY", xnys().id());
    // 08:00 ET = 13:00 UTC → PreMarket.
    let pre_ts = utc(2024, 1, 17, 13, 0);
    // 09:30 ET = 14:30 UTC → Regular.
    let reg_ts = utc(2024, 1, 17, 14, 30);
    let bars = vec![xnys_m1_candle(pre_ts, sym), xnys_m1_candle(reg_ts, sym)];
    let inner = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    let mut s = Filtered::new(inner, EhFilter::RTH_ONLY);

    let mut got = Vec::new();
    while let Some(c) = s.next().await {
        got.push(c.session.kind());
    }
    assert_eq!(got, vec![SessionKind::Regular]);
}

#[tokio::test]
async fn filtered_eh_passes_through_when_all_allowed() {
    let sym = Symbol::new("SPY", xnys().id());
    let pre_ts = utc(2024, 1, 17, 13, 0);
    let reg_ts = utc(2024, 1, 17, 14, 30);
    let bars = vec![xnys_m1_candle(pre_ts, sym), xnys_m1_candle(reg_ts, sym)];
    let inner = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    let mut s = Filtered::new(inner, EhFilter::ALL);
    let mut got = Vec::new();
    while let Some(c) = s.next().await {
        got.push(c.session.kind());
    }
    assert_eq!(got, vec![SessionKind::PreMarket, SessionKind::Regular]);
}

#[tokio::test]
async fn filtered_snapshot_applies_policy() {
    let sym = Symbol::new("SPY", xnys().id());
    let pre_ts = utc(2024, 1, 17, 13, 0);
    let reg_ts = utc(2024, 1, 17, 14, 30);
    let bars = vec![xnys_m1_candle(pre_ts, sym), xnys_m1_candle(reg_ts, sym)];
    let inner = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    let mut s = Filtered::new(inner, EhFilter::RTH_ONLY);
    let range = TimeRange::new(pre_ts, reg_ts + chrono::Duration::minutes(1)).unwrap();
    let snap = s.snapshot(range).await.unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].session.kind(), SessionKind::Regular);
}

#[tokio::test]
async fn filtered_session_kind_allow_list() {
    let sym = Symbol::new("SPY", xnys().id());
    let pre_ts = utc(2024, 1, 17, 13, 0);
    let reg_ts = utc(2024, 1, 17, 14, 30);
    let post_ts = utc(2024, 1, 17, 21, 30); // 16:30 ET = PostMarket
    let bars = vec![
        xnys_m1_candle(pre_ts, sym),
        xnys_m1_candle(reg_ts, sym),
        xnys_m1_candle(post_ts, sym),
    ];
    let inner = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    let policy = SessionKindFilter::new([SessionKind::PreMarket, SessionKind::PostMarket]);
    let mut s = Filtered::new(inner, policy);
    let mut got = Vec::new();
    while let Some(c) = s.next().await {
        got.push(c.session.kind());
    }
    assert_eq!(got, vec![SessionKind::PreMarket, SessionKind::PostMarket]);
}

// ---------------------------------------------------------------------------
// Round-trip: 500 M1 bars, seek to 09:30 RTH open
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_round_trip_500_m1_seek_to_rth_open() {
    let sym = Symbol::new("SPY", xnys().id());
    // Start at 04:00 ET = 09:00 UTC (PreMarket open).
    let start = utc(2024, 1, 17, 9, 0);
    let bars = xnys_m1_range(start, 500, sym);

    // Find the first Regular bar; its window.open is the 09:30 ET open.
    let rth_open = bars
        .iter()
        .find(|c| c.session.kind() == SessionKind::Regular)
        .unwrap()
        .window
        .open;
    assert_eq!(rth_open, utc(2024, 1, 17, 14, 30)); // 09:30 ET = 14:30 UTC

    let mut s = FixtureBarStream::new(xnys_m1_meta(sym), bars).unwrap();
    s.seek(rth_open).await.unwrap();
    let next = s.next().await.unwrap();
    assert_eq!(next.window.open, rth_open);
    assert_eq!(next.session.kind(), SessionKind::Regular);
}

// ---------------------------------------------------------------------------
// Send + Sync checks
// ---------------------------------------------------------------------------

#[test]
fn fixture_stream_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<FixtureBarStream>();
    assert_send::<ChannelBarStream>();
    assert_send::<HistoryThenLive<FixtureBarStream, ChannelBarStream>>();
    assert_send::<Filtered<FixtureBarStream, EhFilter>>();
}

#[test]
fn bar_stream_meta_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BarStreamMeta>();
}
