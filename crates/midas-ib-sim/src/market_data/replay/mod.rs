//! Replay market-data engine (DBN file → engine emissions).
//!
//! `ReplayEngine` streams a `.dbn` file through the `MarketDataEngine`
//! trait. The mapping from DBN instrument IDs to `SymbolKey` is owned by
//! the caller and plugged in via [`ReplayEngine::register_instrument`].

pub mod dbn_reader;
pub mod recorder;

use std::collections::HashMap;
use std::path::Path;

use midas_broker_core::SymbolKey;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{
    Bar, HistoricalReq, MarketEmission, SubKey, SubMode, TickAttribs, TickByTickKind, TickType,
};
use crate::market_data::{MarketDataEngine, MarketDataError, Snapshot};

use self::dbn_reader::{DbnEmission, DbnReader};

/// Reads a Databento `.dbn` file and feeds ticks through the engine interface.
///
/// Instrument-ID → SymbolKey mapping is injected at construction time so the
/// replay file can be generated from any schema (Databento publisher,
/// NBBO, test fixture) and still resolve correctly.
pub struct ReplayEngine {
    reader: Option<DbnReader>,
    /// Map DBN `instrument_id` → `SymbolKey` the rest of the sim knows.
    id_to_symbol: HashMap<u32, SymbolKey>,
    subs: HashMap<SubKey, SubMode>,
    subs_order: Vec<SubKey>,
    subs_dirty: bool,
    /// Cumulative volume per symbol.
    cum_volume: HashMap<SymbolKey, i64>,
    /// Last known bid/ask/last per symbol.
    snapshot_cache: HashMap<SymbolKey, Snapshot>,
}

impl Default for ReplayEngine {
    fn default() -> Self {
        Self::empty()
    }
}

impl ReplayEngine {
    /// Create a replay engine backed by the given DBN file.
    pub fn open(path: &Path) -> Result<Self, MarketDataError> {
        Ok(Self {
            reader: Some(DbnReader::open(path)?),
            ..Self::empty()
        })
    }

    /// Construct with no backing file — used by the hybrid engine and tests.
    pub fn empty() -> Self {
        Self {
            reader: None,
            id_to_symbol: HashMap::new(),
            subs: HashMap::new(),
            subs_order: Vec::new(),
            subs_dirty: false,
            cum_volume: HashMap::new(),
            snapshot_cache: HashMap::new(),
        }
    }

    /// Register a DBN `instrument_id` → `SymbolKey` mapping.
    pub fn register_instrument(&mut self, instrument_id: u32, symbol: SymbolKey) {
        self.id_to_symbol.insert(instrument_id, symbol);
    }

    fn refresh_subs_order(&mut self) {
        if !self.subs_dirty {
            return;
        }
        self.subs_order = self.subs.keys().cloned().collect();
        self.subs_order
            .sort_by_key(|k| (k.session.0, k.req_id.0, k.symbol.contract_id));
        self.subs_dirty = false;
    }

    /// Convert a DBN record to MarketEmissions for matching subscriptions.
    fn emissions_for(&mut self, em: &DbnEmission) -> Vec<MarketEmission> {
        let Some(symbol) = self.id_to_symbol.get(&em.instrument_id()).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match em {
            DbnEmission::Trade { price, size, .. } => {
                let price = *price;
                let size = *size;
                let cum = self
                    .cum_volume
                    .entry(symbol.clone())
                    .and_modify(|v| *v = v.saturating_add(size))
                    .or_insert(size);
                let cum_snapshot = *cum;
                self.update_snapshot(&symbol, None, None, Some(price), Some(size), em.ts());
                for key in self.subs_order.iter().filter(|k| k.symbol == symbol) {
                    if let Some(mode) = self.subs.get(key) {
                        match mode {
                            SubMode::StreamingL1 { .. } => {
                                out.push(MarketEmission::TickPrice {
                                    key: key.clone(),
                                    tick: TickType::Last,
                                    price,
                                    size: Some(size),
                                    attribs: TickAttribs::default(),
                                });
                                out.push(MarketEmission::TickSize {
                                    key: key.clone(),
                                    tick: TickType::Volume,
                                    size: cum_snapshot,
                                });
                            }
                            SubMode::TickByTick { kind } => match kind {
                                TickByTickKind::Last | TickByTickKind::AllLast => {
                                    out.push(MarketEmission::TickPrice {
                                        key: key.clone(),
                                        tick: TickType::Last,
                                        price,
                                        size: Some(size),
                                        attribs: TickAttribs::default(),
                                    });
                                }
                                _ => {}
                            },
                            SubMode::RealtimeBars5s | SubMode::Historical(_) => {}
                        }
                    }
                }
            }
            DbnEmission::Quote { bid, ask, .. } => {
                let bid = *bid;
                let ask = *ask;
                self.update_snapshot(&symbol, Some(bid), Some(ask), None, None, em.ts());
                for key in self.subs_order.iter().filter(|k| k.symbol == symbol) {
                    if let Some(mode) = self.subs.get(key) {
                        match mode {
                            SubMode::StreamingL1 { .. }
                            | SubMode::TickByTick {
                                kind: TickByTickKind::BidAsk,
                            } => {
                                out.push(MarketEmission::TickPrice {
                                    key: key.clone(),
                                    tick: TickType::Bid,
                                    price: bid,
                                    size: None,
                                    attribs: TickAttribs::default(),
                                });
                                out.push(MarketEmission::TickPrice {
                                    key: key.clone(),
                                    tick: TickType::Ask,
                                    price: ask,
                                    size: None,
                                    attribs: TickAttribs::default(),
                                });
                            }
                            SubMode::TickByTick {
                                kind: TickByTickKind::MidPoint,
                            } => {
                                out.push(MarketEmission::TickPrice {
                                    key: key.clone(),
                                    tick: TickType::MarkPrice,
                                    price: (bid + ask) * 0.5,
                                    size: None,
                                    attribs: TickAttribs::default(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            DbnEmission::Ohlcv {
                ts,
                open,
                high,
                low,
                close,
                volume,
                ..
            } => {
                let bar = Bar {
                    time: *ts,
                    open: *open,
                    high: *high,
                    low: *low,
                    close: *close,
                    volume: *volume,
                    wap: (*high + *low) * 0.5,
                    count: 0,
                };
                for key in self.subs_order.iter().filter(|k| k.symbol == symbol) {
                    if let Some(SubMode::Historical(_)) = self.subs.get(key) {
                        out.push(MarketEmission::HistoricalBatch {
                            key: key.clone(),
                            bars: vec![bar.clone()],
                            is_complete: false,
                        });
                    }
                }
            }
        }
        out
    }

    fn update_snapshot(
        &mut self,
        symbol: &SymbolKey,
        bid: Option<f64>,
        ask: Option<f64>,
        last: Option<f64>,
        last_size: Option<i64>,
        ts: VirtualInstant,
    ) {
        let entry = self
            .snapshot_cache
            .entry(symbol.clone())
            .or_insert(Snapshot {
                bid: f64::NAN,
                ask: f64::NAN,
                last: f64::NAN,
                volume: None,
                ts,
            });
        if let Some(b) = bid {
            entry.bid = b;
        }
        if let Some(a) = ask {
            entry.ask = a;
        }
        if let Some(l) = last {
            entry.last = l;
        }
        if let Some(v) = last_size {
            entry.volume = Some(v);
        }
        entry.ts = ts;
    }
}

impl MarketDataEngine for ReplayEngine {
    fn subscribe(&mut self, key: SubKey, mode: SubMode) -> Result<(), MarketDataError> {
        self.subs.insert(key, mode);
        self.subs_dirty = true;
        Ok(())
    }

    fn unsubscribe(&mut self, key: &SubKey) {
        self.subs.remove(key);
        self.subs_dirty = true;
    }

    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
        self.refresh_subs_order();
        let mut out = Vec::new();
        let mut reader = match self.reader.take() {
            Some(r) => r,
            None => return out,
        };
        loop {
            let due = matches!(reader.peek_ts(), Ok(Some(ts)) if ts <= now);
            if !due {
                break;
            }
            let rec = match reader.next_record() {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(_) => break,
            };
            out.extend(self.emissions_for(&rec));
        }
        self.reader = Some(reader);
        out
    }

    fn snapshot(&self, symbol: &SymbolKey) -> Option<Snapshot> {
        self.snapshot_cache.get(symbol).cloned()
    }
}

// ---------------------------------------------------------------------------
// Historical bars — synthetic fast-forward path.
// ---------------------------------------------------------------------------

/// Fast-forward a `SyntheticEngine` and aggregate the emitted trade ticks
/// into OHLCV bars of the requested `bar_size`. Used by the engine glue
/// when `REQ_HISTORICAL_DATA` is served off the synthetic source.
pub fn historical_bars_synthetic(
    engine: &mut crate::market_data::generator::SyntheticEngine,
    symbol: &SymbolKey,
    req: &HistoricalReq,
) -> Vec<Bar> {
    let (duration, bar_size) = parse_duration_and_bar_size(&req.duration, &req.bar_size);
    let from = VirtualInstant::ZERO;
    let ticks = engine.fast_forward_trades(
        symbol,
        from,
        duration,
        std::time::Duration::from_millis(250),
    );
    let mut bars = Vec::new();
    if ticks.is_empty() {
        return bars;
    }
    let mut bucket_start = ticks[0].0;
    let mut open = ticks[0].1;
    let mut high = ticks[0].1;
    let mut low = ticks[0].1;
    let mut close = ticks[0].1;
    let mut volume: i64 = 0;
    let mut count = 0i32;

    for (ts, price, size) in ticks.iter().copied() {
        if ts.as_duration().saturating_sub(bucket_start.as_duration()) >= bar_size {
            bars.push(Bar {
                time: bucket_start,
                open,
                high,
                low,
                close,
                volume,
                wap: (high + low) * 0.5,
                count,
            });
            bucket_start = ts;
            open = price;
            high = price;
            low = price;
            volume = 0;
            count = 0;
        }
        high = high.max(price);
        low = low.min(price);
        close = price;
        volume = volume.saturating_add(size);
        count += 1;
    }
    bars.push(Bar {
        time: bucket_start,
        open,
        high,
        low,
        close,
        volume,
        wap: (high + low) * 0.5,
        count,
    });
    bars
}

fn parse_duration_and_bar_size(
    duration: &str,
    bar_size: &str,
) -> (std::time::Duration, std::time::Duration) {
    let d = parse_ib_duration(duration).unwrap_or(std::time::Duration::from_secs(86_400));
    let b = parse_ib_bar_size(bar_size).unwrap_or(std::time::Duration::from_secs(60));
    (d, b)
}

fn parse_ib_duration(s: &str) -> Option<std::time::Duration> {
    let mut parts = s.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?.to_ascii_uppercase();
    let secs = match unit.as_str() {
        "S" => n,
        "D" => n * 86_400,
        "W" => n * 7 * 86_400,
        "M" => n * 30 * 86_400,
        "Y" => n * 365 * 86_400,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(secs))
}

fn parse_ib_bar_size(s: &str) -> Option<std::time::Duration> {
    let mut parts = s.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?.to_ascii_lowercase();
    let secs = match unit.as_str() {
        "sec" | "secs" | "second" | "seconds" => n,
        "min" | "mins" | "minute" | "minutes" => n * 60,
        "hour" | "hours" => n * 3600,
        "day" | "days" => n * 86_400,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{ReqId, SessionId};

    fn sym(name: &str, cid: i32) -> SymbolKey {
        SymbolKey {
            contract_id: cid,
            symbol: name.into(),
        }
    }

    #[test]
    fn empty_replay_step_returns_nothing() {
        let mut eng = ReplayEngine::empty();
        let em = eng.step(VirtualInstant::from_secs(10));
        assert!(em.is_empty());
    }

    #[test]
    fn parse_ib_duration_and_bar_size() {
        assert_eq!(
            parse_ib_duration("1 D"),
            Some(std::time::Duration::from_secs(86_400))
        );
        assert_eq!(
            parse_ib_duration("30 S"),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            parse_ib_bar_size("5 secs"),
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(
            parse_ib_bar_size("1 min"),
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn subscribe_stream_with_no_reader_does_nothing() {
        let mut eng = ReplayEngine::empty();
        eng.subscribe(
            SubKey {
                session: SessionId(1),
                req_id: ReqId(1),
                symbol: sym("AAPL", 1),
            },
            SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        )
        .unwrap();
        assert!(eng.step(VirtualInstant::from_secs(5)).is_empty());
    }

    #[test]
    fn historical_synthetic_aggregates_bars() {
        use crate::market_data::generator::{SymbolPreset, SyntheticEngine};
        let mut eng = SyntheticEngine::new(42);
        let s = sym("AAPL", 1);
        eng.register(s.clone(), SymbolPreset::Liquid, 100.0);
        let req = HistoricalReq {
            contract: midas_broker_core::ContractSpec::Stock {
                symbol: "AAPL".into(),
                exchange: "SMART".into(),
                currency: "USD".into(),
            },
            end_date_time: String::new(),
            duration: "300 S".into(),
            bar_size: "1 min".into(),
            what_to_show: "TRADES".into(),
            use_rth: true,
            format_date: 1,
            keep_up_to_date: false,
        };
        let bars = historical_bars_synthetic(&mut eng, &s, &req);
        assert!(!bars.is_empty(), "expected at least one bar");
        // 300 seconds / 60 seconds per bar ≈ 5 bars.
        assert!(
            bars.len() >= 4 && bars.len() <= 7,
            "unexpected bar count: {}",
            bars.len()
        );
        for b in &bars {
            assert!(b.high >= b.low && b.open > 0.0 && b.close > 0.0);
        }
    }
}
