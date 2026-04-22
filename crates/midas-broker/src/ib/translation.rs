//! Translation between `rust-ibapi` 2.10 types and our router-era
//! `midas-broker-core::market_data` vocabulary.
//!
//! Every type that rust-ibapi exposes on the `market_data` / historical
//! surface is routed through a helper here so the adapter bodies stay
//! declarative. The helpers are pure, which also makes them easy to
//! unit-test (see `tests/ib_translation.rs`).

use chrono::{DateTime, TimeZone, Utc};
use midas_broker_core::market_data::{
    Bar, BarCompleteness, ContractDetails, ErrorCode, FarmCode, GenericTicks, IbDuration, ReqId,
    SecurityType, SymbolKey, Tick, TickAttributes, TickKind, TickType, TickValue, Timeframe,
    WhatToShow,
};

use ibapi::contracts::tick_types::TickType as IbTickType;
use ibapi::contracts::{Contract as IbContract, SecurityType as IbSecurityType};
use ibapi::market_data::historical::{
    Bar as IbHistoricalBar, BarSize as IbHistoricalBarSize, Duration as IbHistoricalDuration,
    HistoricalData as IbHistoricalData, WhatToShow as IbHistoricalWhatToShow,
};
use ibapi::market_data::realtime::{
    Bar as IbRealtimeBar, BarSize as IbRealtimeBarSize, TickTypes,
    WhatToShow as IbRealtimeWhatToShow,
};

/// Farm-code set: (IB numeric code, connected flag, FarmCode variant).
///
/// Used by both translation (M-13) and the error watcher to decide
/// whether an incoming error is a farm-status message.
#[allow(dead_code)] // consumed by error watcher in a follow-up commit
pub(crate) const FARM_CODES: &[(i32, bool, FarmCode)] = &[
    (2103, false, FarmCode::MarketDataFarmBroken),
    (2104, true, FarmCode::MarketDataFarmOk),
    (2105, false, FarmCode::HistoricalDataFarmBroken),
    (2106, true, FarmCode::HistoricalDataFarmOk),
    (2108, false, FarmCode::MarketDataFarmInactive),
    (2158, true, FarmCode::SecDefFarmOk),
    (1100, false, FarmCode::ConnectionLost),
    (1101, true, FarmCode::ConnectionRestoredDataLost),
    (1102, true, FarmCode::ConnectionRestoredDataKept),
];

/// Classify a raw IB numeric error code into our [`ErrorCode`] enum.
pub fn translate_error_code(code: i32) -> ErrorCode {
    match code {
        10089 => ErrorCode::NoMarketDataPermission,
        354 => ErrorCode::DelayedMarketDataSubscribed,
        10167 => ErrorCode::RequiresAdditionalSubscription,
        300 => ErrorCode::InvalidReqId,
        200 => ErrorCode::NoSecurityDefinition,
        201 => ErrorCode::OrderSizeInvalid,
        202 => ErrorCode::OrderRejected,
        162 => ErrorCode::HistoricalDataServiceError,
        322 => ErrorCode::DuplicateTickerId,
        10147 => ErrorCode::OrderCancelNotFound,
        321 => ErrorCode::Validation,
        100..=102 => ErrorCode::PacingViolation,
        other => ErrorCode::Other(other),
    }
}

/// Look up the `FarmCode` for a numeric IB code, if it is a farm-code.
///
/// Returns `Some((connected, farm_code))` for 2103–2158 and 1100–1102,
/// `None` otherwise.
#[allow(dead_code)] // consumed by error watcher in a follow-up commit
pub(crate) fn translate_farm_code(code: i32) -> Option<(bool, FarmCode)> {
    FARM_CODES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, conn, fc)| (*conn, *fc))
}

/// Translate our [`WhatToShow`] into rust-ibapi's realtime enum
/// (`Trades` / `MidPoint` / `Bid` / `Ask`).
///
/// rust-ibapi's realtime `WhatToShow` is narrower than ours — values we
/// cannot represent there (e.g. `BidAsk`, yield variants) fall back to
/// `Trades` and the caller is expected to have validated ahead of time.
pub(crate) fn to_ib_realtime_what_to_show(w: WhatToShow) -> IbRealtimeWhatToShow {
    match w {
        WhatToShow::Trades => IbRealtimeWhatToShow::Trades,
        WhatToShow::Midpoint => IbRealtimeWhatToShow::MidPoint,
        WhatToShow::Bid => IbRealtimeWhatToShow::Bid,
        WhatToShow::Ask => IbRealtimeWhatToShow::Ask,
        // Anything else is only valid for historical bars — IB rejects it
        // on realtime_bars, so we coerce to Trades and log at the call
        // site (the adapter does not silently mis-route).
        _ => IbRealtimeWhatToShow::Trades,
    }
}

/// Translate our [`WhatToShow`] into rust-ibapi's historical enum.
pub(crate) fn to_ib_historical_what_to_show(w: WhatToShow) -> IbHistoricalWhatToShow {
    match w {
        WhatToShow::Trades => IbHistoricalWhatToShow::Trades,
        WhatToShow::Midpoint => IbHistoricalWhatToShow::MidPoint,
        WhatToShow::Bid => IbHistoricalWhatToShow::Bid,
        WhatToShow::Ask => IbHistoricalWhatToShow::Ask,
        WhatToShow::BidAsk => IbHistoricalWhatToShow::BidAsk,
        WhatToShow::HistoricalVolatility => IbHistoricalWhatToShow::HistoricalVolatility,
        WhatToShow::OptionImpliedVolatility => IbHistoricalWhatToShow::OptionImpliedVolatility,
        WhatToShow::Schedule => IbHistoricalWhatToShow::Schedule,
        WhatToShow::AdjustedLast => IbHistoricalWhatToShow::AdjustedLast,
        // Yield-family variants don't exist on rust-ibapi 2.10's
        // historical enum — fall back to Trades.
        _ => IbHistoricalWhatToShow::Trades,
    }
}

/// Map our [`Timeframe`] to rust-ibapi's realtime `BarSize`.
///
/// rust-ibapi only supports `Sec5` on realtime bars; anything else would
/// be rejected. Callers should only ever pass [`Timeframe::S5`] here.
pub(crate) fn to_ib_realtime_bar_size(tf: Timeframe) -> IbRealtimeBarSize {
    // rust-ibapi only exposes Sec5 for realtime bars.
    let _ = tf;
    IbRealtimeBarSize::Sec5
}

/// Map our [`Timeframe`] to rust-ibapi's historical `BarSize`.
///
/// Returns `None` when the timeframe has no one-to-one representation
/// (e.g. seconds granularities below `S1`).
pub(crate) fn to_ib_historical_bar_size(tf: Timeframe) -> IbHistoricalBarSize {
    match tf {
        Timeframe::S1 => IbHistoricalBarSize::Sec,
        Timeframe::S5 => IbHistoricalBarSize::Sec5,
        Timeframe::S15 => IbHistoricalBarSize::Sec15,
        Timeframe::S30 => IbHistoricalBarSize::Sec30,
        Timeframe::M1 => IbHistoricalBarSize::Min,
        Timeframe::M5 => IbHistoricalBarSize::Min5,
        Timeframe::M15 => IbHistoricalBarSize::Min15,
        Timeframe::M30 => IbHistoricalBarSize::Min30,
        Timeframe::H1 => IbHistoricalBarSize::Hour,
        Timeframe::H4 => IbHistoricalBarSize::Hour4,
        Timeframe::D1 => IbHistoricalBarSize::Day,
        Timeframe::W1 => IbHistoricalBarSize::Week,
        Timeframe::MN1 => IbHistoricalBarSize::Month,
    }
}

/// Translate our [`IbDuration`] into rust-ibapi's `Duration`.
pub(crate) fn to_ib_historical_duration(d: IbDuration) -> IbHistoricalDuration {
    match d {
        IbDuration::Seconds(n) => IbHistoricalDuration::seconds(n as i32),
        IbDuration::Days(n) => IbHistoricalDuration::days(n as i32),
        IbDuration::Weeks(n) => IbHistoricalDuration::weeks(n as i32),
        IbDuration::Months(n) => IbHistoricalDuration::months(n as i32),
        IbDuration::Years(n) => IbHistoricalDuration::years(n as i32),
    }
}

/// Translate our [`SecurityType`] to rust-ibapi's.
pub(crate) fn to_ib_security_type(s: SecurityType) -> IbSecurityType {
    // Our root `SecurityType` uses IB wire strings — round-trip through
    // the rust-ibapi `from(&str)` constructor.
    IbSecurityType::from(s.as_ib_str())
}

/// Translate rust-ibapi's `SecurityType` back to ours.
///
/// Our 4-variant enum is narrower than rust-ibapi's; unknown variants
/// collapse to [`SecurityType::Stock`] (IB's default) and the caller
/// relies on the raw `ContractDetails` text for disambiguation.
pub(crate) fn from_ib_security_type(s: &IbSecurityType) -> SecurityType {
    SecurityType::from_ib_str(&s.to_string()).unwrap_or(SecurityType::Stock)
}

/// Translate our [`SymbolKey`] + extras to an rust-ibapi [`Contract`].
///
/// Callers typically want this for stock subscriptions; derivatives need
/// `ContractDetails` resolution first (see `resolve_contract`).
pub(crate) fn build_ib_stock_contract(symbol: &SymbolKey, exchange: &str) -> IbContract {
    IbContract {
        contract_id: symbol.contract_id,
        symbol: symbol.symbol.clone().into(),
        security_type: IbSecurityType::Stock,
        exchange: exchange.into(),
        currency: "USD".into(),
        ..IbContract::default()
    }
}

/// Translate rust-ibapi's [`TickType`](IbTickType) to our narrower
/// [`TickType`].
///
/// Unknown or unmapped variants fall back to `None` — the adapter then
/// drops the tick rather than emitting a lossy mapping.
pub(crate) fn translate_tick_type(ib: IbTickType) -> Option<TickType> {
    Some(match ib {
        IbTickType::Bid => TickType::Bid,
        IbTickType::Ask => TickType::Ask,
        IbTickType::Last => TickType::Last,
        IbTickType::BidSize => TickType::BidSize,
        IbTickType::AskSize => TickType::AskSize,
        IbTickType::LastSize => TickType::LastSize,
        IbTickType::Volume => TickType::Volume,
        IbTickType::High => TickType::High,
        IbTickType::Low => TickType::Low,
        IbTickType::Close => TickType::Close,
        IbTickType::Open => TickType::Open,
        // IB 49 is "halted": mapped to our HaltedState marker.
        IbTickType::Halted => TickType::HaltedState,
        _ => return None,
    })
}

/// Translate rust-ibapi's price-tick attribute bitflags to ours.
pub(crate) fn translate_price_attributes(
    a: &ibapi::market_data::realtime::TickAttribute,
) -> TickAttributes {
    TickAttributes {
        can_auto_execute: a.can_auto_execute,
        past_limit: a.past_limit,
        pre_open: a.pre_open,
        ..TickAttributes::default()
    }
}

/// Translate an rust-ibapi [`TickTypes`] event into zero or one [`Tick`].
///
/// Returns `None` for `SnapshotEnd`, `Notice`, `EFP`, `OptionComputation`
/// and `RequestParameters` — those map to non-tick router events (or are
/// dropped) by the caller, never into a [`Tick`].
///
/// M-17: `PriceSize` comes across as a single atomic event (price + size
/// together) — we keep that atomicity and do NOT split.
pub(crate) fn translate_tick_event(
    symbol: &SymbolKey,
    req_id: ReqId,
    ts: DateTime<Utc>,
    ev: TickTypes,
) -> Option<Tick> {
    match ev {
        TickTypes::Price(p) => {
            let tick_type = translate_tick_type(p.tick_type)?;
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::Price,
                tick_type,
                value: TickValue::Price(p.price),
                attrs: translate_price_attributes(&p.attributes),
                ts,
            })
        }
        TickTypes::Size(s) => {
            let tick_type = translate_tick_type(s.tick_type)?;
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::Size,
                tick_type,
                value: TickValue::Size(s.size as i64),
                attrs: TickAttributes::default(),
                ts,
            })
        }
        TickTypes::PriceSize(ps) => {
            // M-17: atomic pair — carry the price tick type (the size
            // half is always derivable).
            let tick_type = translate_tick_type(ps.price_tick_type)?;
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::PriceSize,
                tick_type,
                value: TickValue::PriceSize {
                    price: ps.price,
                    size: ps.size as i64,
                },
                attrs: translate_price_attributes(&ps.attributes),
                ts,
            })
        }
        TickTypes::Generic(g) => {
            let tick_type = translate_tick_type(g.tick_type)?;
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::Generic,
                tick_type,
                value: TickValue::Generic(g.value),
                attrs: TickAttributes::default(),
                ts,
            })
        }
        TickTypes::String(s) => {
            let tick_type = translate_tick_type(s.tick_type)?;
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::String,
                tick_type,
                value: TickValue::Text(s.value),
                attrs: TickAttributes::default(),
                ts,
            })
        }
        TickTypes::RequestParameters(_) => {
            // Emit a minimal marker Tick using `Last` as a stand-in tick
            // sub-type — consumers keyed on `kind == Params` can still
            // observe the event.
            Some(Tick {
                symbol: symbol.clone(),
                req_id,
                kind: TickKind::Params,
                tick_type: TickType::Last,
                value: TickValue::Generic(0.0),
                attrs: TickAttributes::default(),
                ts,
            })
        }
        // EFP, OptionComputation, SnapshotEnd, Notice are handled
        // outside the tick fan-out (router Error/lifecycle events).
        TickTypes::EFP(_)
        | TickTypes::OptionComputation(_)
        | TickTypes::SnapshotEnd
        | TickTypes::Notice(_) => None,
    }
}

/// Translate rust-ibapi's [`Bar`](IbRealtimeBar) realtime bar into ours.
pub(crate) fn translate_realtime_bar(
    symbol: &SymbolKey,
    b: &IbRealtimeBar,
    what_to_show: WhatToShow,
) -> Bar {
    let _ = what_to_show;
    let ts_open = offsetdatetime_to_chrono(b.date);
    let ts_close = ts_open + chrono::Duration::seconds(5);
    Bar {
        symbol: symbol.clone(),
        timeframe: Timeframe::S5,
        ts_open,
        ts_close,
        o: b.open,
        h: b.high,
        l: b.low,
        c: b.close,
        volume: (b.volume.max(0.0)) as u64,
        trade_count: b.count.max(0) as u32,
        wap: Some(b.wap),
        completeness: BarCompleteness::Completed,
    }
}

/// Translate rust-ibapi's historical [`Bar`](IbHistoricalBar) into ours.
pub(crate) fn translate_historical_bar(
    symbol: &SymbolKey,
    tf: Timeframe,
    b: &IbHistoricalBar,
) -> Bar {
    let ts_open = offsetdatetime_to_chrono(b.date);
    let ts_close = ts_open + chrono::Duration::seconds(tf.as_secs() as i64);
    Bar {
        symbol: symbol.clone(),
        timeframe: tf,
        ts_open,
        ts_close,
        o: b.open,
        h: b.high,
        l: b.low,
        c: b.close,
        volume: (b.volume.max(0.0)) as u64,
        trade_count: b.count.max(0) as u32,
        wap: Some(b.wap),
        completeness: BarCompleteness::Completed,
    }
}

/// Translate a full rust-ibapi historical payload into a list of our
/// [`Bar`]s.
pub(crate) fn translate_historical_payload(
    symbol: &SymbolKey,
    tf: Timeframe,
    d: &IbHistoricalData,
) -> Vec<Bar> {
    d.bars
        .iter()
        .map(|b| translate_historical_bar(symbol, tf, b))
        .collect()
}

/// Translate an rust-ibapi `ContractDetails` into ours.
pub(crate) fn translate_contract_details(d: &ibapi::contracts::ContractDetails) -> ContractDetails {
    ContractDetails {
        contract_id: d.contract.contract_id,
        symbol: d.contract.symbol.to_string(),
        sec_type: from_ib_security_type(&d.contract.security_type),
        exchange: d.contract.exchange.to_string(),
        primary_exchange: {
            let s = d.contract.primary_exchange.to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        },
        currency: d.contract.currency.to_string(),
        long_name: if d.long_name.is_empty() {
            None
        } else {
            Some(d.long_name.clone())
        },
        min_tick: d.min_tick,
        multiplier: if d.contract.multiplier.is_empty() {
            None
        } else {
            Some(d.contract.multiplier.clone())
        },
        trading_class: if d.contract.trading_class.is_empty() {
            None
        } else {
            Some(d.contract.trading_class.clone())
        },
    }
}

/// Convert `time::OffsetDateTime` → `chrono::DateTime<Utc>`.
///
/// rust-ibapi uses the `time` crate; every public type in
/// `midas-broker-core` uses `chrono`. The conversion is cheap
/// (nanos-through-unix-timestamp) and preserves precision.
pub(crate) fn offsetdatetime_to_chrono(t: time::OffsetDateTime) -> DateTime<Utc> {
    let secs = t.unix_timestamp();
    let nanos = t.nanosecond();
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Convert `chrono::DateTime<Utc>` → `time::OffsetDateTime`.
pub(crate) fn chrono_to_offsetdatetime(t: DateTime<Utc>) -> time::OffsetDateTime {
    let secs = t.timestamp();
    let nanos = t.timestamp_subsec_nanos();
    // Reconstitute — we only ever use UTC on the router boundary.
    time::OffsetDateTime::from_unix_timestamp(secs).unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        + time::Duration::nanoseconds(nanos as i64)
}

/// Generic-ticks → comma-joined IB wire string.
///
/// Mirrors [`GenericTicks::to_ib_string`] but returns a `Vec<&str>`
/// friendly form for the rust-ibapi builder.
pub(crate) fn generic_ticks_as_vec(g: &GenericTicks) -> Vec<String> {
    g.0.iter().map(|c| c.to_string()).collect()
}

// ───────────────────────────────────────────────────────────────────────────
// Helpers local to our SecurityType — `as_ib_str` + `from_ib_str`.
//
// Our `SecurityType` is a narrow 4-variant enum (`Stock`, `Option`,
// `Future`, `Forex`); rust-ibapi's enum is richer. The wire tokens match
// where both sides carry the same variant.
// ───────────────────────────────────────────────────────────────────────────

trait SecurityTypeIb {
    fn as_ib_str(&self) -> &'static str;
    fn from_ib_str(s: &str) -> Option<SecurityType>;
}

impl SecurityTypeIb for SecurityType {
    fn as_ib_str(&self) -> &'static str {
        match self {
            SecurityType::Stock => "STK",
            SecurityType::Option => "OPT",
            SecurityType::Future => "FUT",
            SecurityType::Forex => "CASH",
        }
    }

    fn from_ib_str(s: &str) -> Option<SecurityType> {
        Some(match s {
            "STK" => SecurityType::Stock,
            "OPT" => SecurityType::Option,
            "FUT" => SecurityType::Future,
            "CASH" => SecurityType::Forex,
            // Any IB variant we don't model cleanly collapses onto the
            // closest match (`Stock` for equity-like, `Future` for
            // derivatives). Callers that need the full IB taxonomy
            // should use `ContractDetails::long_name` and friends.
            "CONTFUT" | "IND" | "CFD" | "WAR" | "BOND" | "FUND" | "CMDTY" | "CRYPTO" => {
                SecurityType::Stock
            }
            "FOP" | "BAG" => SecurityType::Future,
            _ => return None,
        })
    }
}
