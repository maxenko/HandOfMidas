//! Minimal wrapper over the `dbn` crate's encoder.
//!
//! The session recorder decodes TWS market-data messages (`TICK_PRICE`,
//! `TICK_SIZE`, `HISTORICAL_DATA`, …) into canonical Databento records so the
//! Stage-03 replay engine can consume them directly.
//!
//! Only a narrow slice of the dbn schema is produced here:
//!
//! - `TradeMsg` for trade prints (TICK_PRICE with tick types LAST/ALL_LAST)
//! - Future expansion: MBP1 for bid/ask quotes, OHLCV for historical bars.
//!
//! Price scaling: TWS fields are IEEE-754 `f64` USD; dbn uses fixed-point
//! `i64` with a 1e-9 multiplier. See [`to_fixed_price`].

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use dbn::encode::{dbn::Encoder as DbnCrateEncoder, EncodeRecord};
use dbn::{FlagSet, MetadataBuilder, RecordHeader, SType, Schema, TradeMsg};

/// Multiplier applied when converting floating-point USD prices into dbn's
/// integer fixed-point representation.
pub const FIXED_PRICE_SCALE: f64 = 1_000_000_000.0;

/// Convert a floating-point price into dbn's `i64` fixed-point form.
pub fn to_fixed_price(px: f64) -> i64 {
    (px * FIXED_PRICE_SCALE).round() as i64
}

/// Errors from the dbn encoder wrapper.
#[derive(Debug, thiserror::Error)]
pub enum DbnEncoderError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dbn: {0}")]
    Dbn(String),
}

impl From<dbn::Error> for DbnEncoderError {
    fn from(e: dbn::Error) -> Self {
        Self::Dbn(e.to_string())
    }
}

/// Trade tick as captured from a decoded TWS `TICK_PRICE` message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeTick {
    pub ts_event_nanos: u64,
    pub instrument_id: u32,
    pub price: f64,
    pub size: u32,
}

/// Wraps a `dbn::Encoder` with a narrow API tailored to the sim's use case.
pub struct DbnEncoder<W: Write> {
    inner: DbnCrateEncoder<W>,
}

impl DbnEncoder<BufWriter<File>> {
    /// Create a new `.dbn` file at `path`, recording the TRADES schema.
    pub fn create(path: impl AsRef<Path>, dataset: &str) -> Result<Self, DbnEncoderError> {
        let file = File::create(path)?;
        Self::with_writer(BufWriter::new(file), dataset)
    }
}

impl<W: Write> DbnEncoder<W> {
    /// Build an encoder over an arbitrary writer. Writes metadata headers
    /// immediately.
    pub fn with_writer(writer: W, dataset: &str) -> Result<Self, DbnEncoderError> {
        let metadata = MetadataBuilder::new()
            .dataset(dataset.to_string())
            .schema(Some(Schema::Trades))
            .start(0)
            .stype_in(Some(SType::InstrumentId))
            .stype_out(SType::InstrumentId)
            .build();
        let inner = DbnCrateEncoder::new(writer, &metadata)?;
        Ok(Self { inner })
    }

    /// Encode one trade record.
    pub fn encode_trade(&mut self, tick: TradeTick) -> Result<(), DbnEncoderError> {
        let hd = RecordHeader::new::<TradeMsg>(
            dbn::rtype::MBP_0,
            0,
            tick.instrument_id,
            tick.ts_event_nanos,
        );
        let msg = TradeMsg {
            hd,
            price: to_fixed_price(tick.price),
            size: tick.size,
            action: b'T' as std::os::raw::c_char,
            side: b'N' as std::os::raw::c_char,
            flags: FlagSet::empty(),
            depth: 0,
            ts_recv: tick.ts_event_nanos,
            ts_in_delta: 0,
            sequence: 0,
        };
        self.inner.encode_record(&msg)?;
        Ok(())
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> Result<(), DbnEncoderError> {
        self.inner.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbn::decode::dbn::Decoder as DbnDecoder;
    use dbn::decode::{DbnMetadata, DecodeRecord};
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn fixed_price_roundtrip() {
        let p = to_fixed_price(174.505);
        assert_eq!(p, 174_505_000_000);
    }

    #[test]
    fn encode_trade_to_memory_decodes() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut enc = DbnEncoder::with_writer(&mut buf, "IB.SIM").unwrap();
            enc.encode_trade(TradeTick {
                ts_event_nanos: 1_700_000_001,
                instrument_id: 42,
                price: 100.25,
                size: 200,
            })
            .unwrap();
            enc.flush().unwrap();
        }

        let mut dec = DbnDecoder::new(Cursor::new(buf)).unwrap();
        let meta = dec.metadata();
        assert_eq!(meta.dataset, "IB.SIM");
        assert_eq!(meta.schema, Some(Schema::Trades));
        let rec = dec.decode_record::<TradeMsg>().unwrap().unwrap();
        assert_eq!(rec.hd.instrument_id, 42);
        assert_eq!(rec.price, 100_250_000_000);
        assert_eq!(rec.size, 200);
    }

    #[test]
    fn encode_trade_to_file_decodes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cap.dbn");
        {
            let mut enc = DbnEncoder::create(&path, "IB.SIM").unwrap();
            enc.encode_trade(TradeTick {
                ts_event_nanos: 1_000,
                instrument_id: 7,
                price: 50.0,
                size: 10,
            })
            .unwrap();
            enc.flush().unwrap();
        }
        let file = File::open(&path).unwrap();
        let mut dec = DbnDecoder::new(file).unwrap();
        let rec = dec.decode_record::<TradeMsg>().unwrap().unwrap();
        assert_eq!(rec.hd.instrument_id, 7);
    }
}
