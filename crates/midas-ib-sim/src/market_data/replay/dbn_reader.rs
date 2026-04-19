//! Streaming Databento `.dbn` reader — thin wrapper over `dbn::decode::dbn::Decoder`.
//!
//! The sim only consumes three record schemas:
//! - `Trade` (trades) → `TickType::Last`
//! - `Mbp1`  (top-of-book quote, alias `Tbbo`) → `TickType::Bid` / `TickType::Ask`
//! - `Ohlcv` (minute/daily bars) → `HistoricalBatch`
//!
//! Other record types are silently skipped. The reader buffers exactly one
//! record ahead so `peek_ts` can be used by the engine's step-loop to decide
//! whether the next record is due at `now`.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use dbn::decode::dbn::Decoder as DbnDecoder;
use dbn::record::{OhlcvMsg, TbboMsg, TradeMsg};
use dbn::VersionUpgradePolicy;

use crate::engine::clock::VirtualInstant;
use crate::market_data::MarketDataError;

/// Projection of a DBN record onto the small set of types the sim consumes.
#[derive(Clone, Debug)]
pub enum DbnEmission {
    /// A trade — one `TickType::Last` emission.
    Trade {
        ts: VirtualInstant,
        instrument_id: u32,
        price: f64,
        size: i64,
    },
    /// A top-of-book quote — `TickType::Bid` and `TickType::Ask` emissions.
    Quote {
        ts: VirtualInstant,
        instrument_id: u32,
        bid: f64,
        ask: f64,
        bid_size: i64,
        ask_size: i64,
    },
    /// An aggregated OHLCV bar — `HistoricalBatch`-bound.
    Ohlcv {
        ts: VirtualInstant,
        instrument_id: u32,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
}

impl DbnEmission {
    pub fn ts(&self) -> VirtualInstant {
        match self {
            Self::Trade { ts, .. } | Self::Quote { ts, .. } | Self::Ohlcv { ts, .. } => *ts,
        }
    }

    pub fn instrument_id(&self) -> u32 {
        match self {
            Self::Trade { instrument_id, .. }
            | Self::Quote { instrument_id, .. }
            | Self::Ohlcv { instrument_id, .. } => *instrument_id,
        }
    }
}

/// Streaming reader over a `.dbn` file.
pub struct DbnReader {
    decoder: DbnDecoder<BufReader<File>>,
    /// Wall-clock UTC nanos of the first record — used as the virtual-time
    /// anchor so DBN's absolute `ts_event` is remapped to `VirtualInstant`
    /// since-start.
    anchor_ns: Option<u64>,
    /// One-record look-ahead buffer for `peek_ts`.
    buffered: Option<DbnEmission>,
    finished: bool,
}

impl DbnReader {
    /// Open a `.dbn` file at `path`. Compression is inferred automatically.
    pub fn open(path: &Path) -> Result<Self, MarketDataError> {
        let decoder = DbnDecoder::with_upgrade_policy(
            BufReader::new(File::open(path)?),
            VersionUpgradePolicy::Upgrade,
        )
        .map_err(|e| MarketDataError::Io(std::io::Error::other(e.to_string())))?;
        Ok(Self {
            decoder,
            anchor_ns: None,
            buffered: None,
            finished: false,
        })
    }

    /// Peek the virtual time of the next record without consuming it.
    pub fn peek_ts(&mut self) -> Result<Option<VirtualInstant>, MarketDataError> {
        if self.buffered.is_none() {
            self.advance()?;
        }
        Ok(self.buffered.as_ref().map(|e| e.ts()))
    }

    /// Consume and return the next record if any.
    pub fn next_record(&mut self) -> Result<Option<DbnEmission>, MarketDataError> {
        if self.buffered.is_none() {
            self.advance()?;
        }
        Ok(self.buffered.take())
    }

    fn advance(&mut self) -> Result<(), MarketDataError> {
        if self.finished {
            return Ok(());
        }
        use dbn::decode::DecodeRecordRef;
        loop {
            let rec_ref = self
                .decoder
                .decode_record_ref()
                .map_err(|e| MarketDataError::Io(std::io::Error::other(e.to_string())))?;
            let Some(rec) = rec_ref else {
                self.finished = true;
                return Ok(());
            };
            if let Some(trade) = rec.get::<TradeMsg>() {
                let ts_ns = trade.hd.ts_event;
                let anchor = *self.anchor_ns.get_or_insert(ts_ns);
                let ts = virtual_instant_from_ns(ts_ns, anchor);
                self.buffered = Some(DbnEmission::Trade {
                    ts,
                    instrument_id: trade.hd.instrument_id,
                    price: fixed_to_f64(trade.price),
                    size: trade.size as i64,
                });
                return Ok(());
            }
            if let Some(q) = rec.get::<TbboMsg>() {
                let ts_ns = q.hd.ts_event;
                let anchor = *self.anchor_ns.get_or_insert(ts_ns);
                let ts = virtual_instant_from_ns(ts_ns, anchor);
                // TbboMsg = Mbp1Msg: levels[0] is best bid/ask.
                if let Some(pair) = q.levels.first() {
                    self.buffered = Some(DbnEmission::Quote {
                        ts,
                        instrument_id: q.hd.instrument_id,
                        bid: fixed_to_f64(pair.bid_px),
                        ask: fixed_to_f64(pair.ask_px),
                        bid_size: pair.bid_sz as i64,
                        ask_size: pair.ask_sz as i64,
                    });
                    return Ok(());
                }
                continue;
            }
            if let Some(b) = rec.get::<OhlcvMsg>() {
                let ts_ns = b.hd.ts_event;
                let anchor = *self.anchor_ns.get_or_insert(ts_ns);
                let ts = virtual_instant_from_ns(ts_ns, anchor);
                self.buffered = Some(DbnEmission::Ohlcv {
                    ts,
                    instrument_id: b.hd.instrument_id,
                    open: fixed_to_f64(b.open),
                    high: fixed_to_f64(b.high),
                    low: fixed_to_f64(b.low),
                    close: fixed_to_f64(b.close),
                    volume: b.volume as i64,
                });
                return Ok(());
            }
            // Not a schema we care about — skip silently.
            if rec.as_enum().is_err() {
                // Unknown record shape; break to avoid looping forever.
                self.finished = true;
                return Ok(());
            }
        }
    }
}

/// Convert a DBN fixed-point price (`1e-9` ticks) to f64 dollars.
#[inline]
pub(crate) fn fixed_to_f64(p: i64) -> f64 {
    if p == dbn::UNDEF_PRICE {
        return f64::NAN;
    }
    p as f64 / dbn::FIXED_PRICE_SCALE as f64
}

#[inline]
fn virtual_instant_from_ns(ts_ns: u64, anchor_ns: u64) -> VirtualInstant {
    let delta_ns = ts_ns.saturating_sub(anchor_ns);
    VirtualInstant::from_duration(std::time::Duration::from_nanos(delta_ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_to_f64_round_trips() {
        // 1.234567890 expressed in 1e-9 fixed-point.
        let raw: i64 = 1_234_567_890;
        let f = fixed_to_f64(raw);
        assert!((f - 1.234567890).abs() < 1e-9);
    }

    #[test]
    fn undef_price_maps_to_nan() {
        assert!(fixed_to_f64(dbn::UNDEF_PRICE).is_nan());
    }

    #[test]
    fn virtual_instant_anchor_subtracts_correctly() {
        let anchor: u64 = 1_700_000_000_000_000_000;
        let now = anchor + 500_000_000; // +500 ms
        let vi = virtual_instant_from_ns(now, anchor);
        assert_eq!(vi.as_duration(), std::time::Duration::from_millis(500));
    }
}
