//! `history_then_live` seam utility.
//!
//! Composes historical bars (one-shot) with a live fan-out stream into
//! a single chained `Stream<Item = Bar>` that has no gap and no
//! duplicate at the seam boundary (M-35: filter live by
//! `ts_open > t_server`).
//!
//! S5 implementation uses the router's realtime-bar fan-out
//! (`subscribe_rt_bars`) as the live source. S6 will layer the
//! aggregator on top via a separate `history_then_aggregated_live`
//! helper without changing this one.
//!
//! Ownership: we deliberately use [`SubscriptionHandle::into_stream`]
//! so rx AND guard live inside the returned stream. Dropping the
//! returned stream drops the guard → DecRef upstream (NB-2).

use chrono::Utc;
use futures::{stream, Stream, StreamExt};
use midas_broker_core::market_data::{
    Bar, IbDuration, MarketDataError, SymbolKey, Timeframe, WhatToShow,
};

use super::MarketDataRouter;

/// History + live seam (BR-7 rewrite, NB-2).
///
/// 1. Subscribe to the live RT-bar fan-out first (so nothing is lost
///    while history is being fetched).
/// 2. Fetch history one-shot up to "now".
/// 3. Return `stream::iter(history).chain(live.filter(ts_open > t_server))`.
///
/// The historical call uses `WhatToShow::Trades`; callers that need
/// other `WhatToShow` values should build their own seam on top of
/// `historical_bars` + `subscribe_rt_bars`.
pub(crate) async fn history_then_live_impl(
    router: &MarketDataRouter,
    symbol: SymbolKey,
    tf: Timeframe,
    duration: IbDuration,
) -> Result<impl Stream<Item = Bar> + Send + 'static, MarketDataError> {
    // 1. Live subscription FIRST so we don't miss any bars that fall
    //    between the history cutoff and our own subscribe call.
    let live_handle = router.subscribe_rt_bars(symbol.clone()).await?;
    let live_stream = live_handle.into_stream().map(|arc_bar| (*arc_bar).clone());

    // 2. One-shot history. Resolve contract if we haven't already.
    let con_id = router.resolve_or_cached(&symbol).await?.contract_id;
    let end = Utc::now();
    let hist = router
        .source()
        .historical_bars(&symbol, con_id, end, duration, tf, WhatToShow::Trades, true)
        .await?;
    let t_server = hist.last_ts;

    tracing::debug!(
        symbol = %symbol,
        t_server = %t_server,
        hist_len = hist.bars.len(),
        "history_then_live seam boundary"
    );

    // 3. Filter live tail by ts_open > t_server (M-35). Using
    //    ts_open rather than ts_close avoids the boundary-duplicate
    //    case where a bar whose window opens exactly at t_server is
    //    already in history.
    let filtered_tail = live_stream.filter(move |bar| {
        let keep = bar.ts_open > t_server;
        async move { keep }
    });

    Ok(stream::iter(hist.bars).chain(filtered_tail))
}
