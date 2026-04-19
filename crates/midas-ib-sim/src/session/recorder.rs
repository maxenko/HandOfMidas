//! [`Recorder`] — buffers pcap + dbn writes during a live or proxied session.
//!
//! Callers drive the recorder from the bridge between the client socket and
//! the sim socket; each direction of wire bytes is copied through
//! [`Recorder::record_client_to_sim`] / [`Recorder::record_sim_to_client`],
//! which tees them to disk without otherwise affecting the stream.
//!
//! For decoded market-data messages that the Stage-03 replay engine will
//! consume, the recorder also owns a dbn encoder that callers feed through
//! [`Recorder::record_decoded_trade`].
//!
//! # Time source
//!
//! The recorder holds an [`Instant`] taken at [`Recorder::start`] time plus a
//! wall-clock `start_ts_nanos` written into the pcap header. All subsequent
//! record timestamps are monotonic offsets from that `Instant`, so they stay
//! strictly non-decreasing even if the wall clock jumps.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::session::dbn_encoder::{DbnEncoder, DbnEncoderError, TradeTick};
use crate::session::pcap::{Direction, TwsPcapHeader, TwsPcapRecord, TwsPcapWriter, ZstdFile};

/// Errors surfaced while recording a session.
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dbn: {0}")]
    Dbn(#[from] DbnEncoderError),
}

/// Either a raw or zstd-compressed pcap writer — chosen once at construction.
pub enum PcapSink {
    Raw(TwsPcapWriter<BufWriter<File>>),
    Zstd(TwsPcapWriter<ZstdFile>),
}

impl PcapSink {
    fn write_record(&mut self, rec: &TwsPcapRecord) -> std::io::Result<()> {
        match self {
            Self::Raw(w) => w.write_record(rec),
            Self::Zstd(w) => w.write_record(rec),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Raw(w) => w.flush(),
            Self::Zstd(w) => w.flush(),
        }
    }
}

/// A live session recorder.
pub struct Recorder {
    pcap_writer: PcapSink,
    dbn_writer: Option<DbnEncoder<BufWriter<File>>>,
    start: Instant,
}

impl Recorder {
    /// Start a fresh recording. `out_stem` is the path *without* extension;
    /// the recorder appends `.tws.pcap` (plus `.zst` if `compress`) and
    /// `.dbn`.
    ///
    /// `server_version_neg` is stamped into the pcap header; set it to the
    /// TWS server version returned by the handshake.
    pub fn start(
        out_stem: impl AsRef<Path>,
        server_version_neg: u16,
        compress: bool,
        dbn_dataset: Option<&str>,
    ) -> Result<Self, RecorderError> {
        let start_ts_nanos: i128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        let header = TwsPcapHeader::new(server_version_neg, start_ts_nanos);

        let stem = out_stem.as_ref();
        let pcap_path = if compress {
            let mut p = stem.to_path_buf();
            p.set_extension("tws.pcap.zst");
            p
        } else {
            let mut p = stem.to_path_buf();
            p.set_extension("tws.pcap");
            p
        };
        let pcap_writer = if compress {
            PcapSink::Zstd(TwsPcapWriter::create_zstd(&pcap_path, header)?)
        } else {
            PcapSink::Raw(TwsPcapWriter::create(&pcap_path, header)?)
        };

        let dbn_writer = if let Some(dataset) = dbn_dataset {
            let mut p = stem.to_path_buf();
            p.set_extension("dbn");
            Some(DbnEncoder::create(&p, dataset)?)
        } else {
            None
        };

        Ok(Self {
            pcap_writer,
            dbn_writer,
            start: Instant::now(),
        })
    }

    /// Alternative constructor for tests and structured callers.
    pub fn from_parts(
        pcap_writer: PcapSink,
        dbn_writer: Option<DbnEncoder<BufWriter<File>>>,
    ) -> Self {
        Self {
            pcap_writer,
            dbn_writer,
            start: Instant::now(),
        }
    }

    /// Monotonic ns since recorder start. Saturates instead of overflowing.
    fn elapsed_ns(&self) -> u64 {
        self.start
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    /// Record raw bytes flowing *from* the client *into* the sim.
    pub fn record_client_to_sim(&mut self, bytes: &[u8]) -> Result<(), RecorderError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let rec = TwsPcapRecord::new(self.elapsed_ns(), Direction::ClientToSim, bytes.to_vec());
        self.pcap_writer.write_record(&rec)?;
        Ok(())
    }

    /// Record raw bytes flowing *from* the sim *out* to the client.
    pub fn record_sim_to_client(&mut self, bytes: &[u8]) -> Result<(), RecorderError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let rec = TwsPcapRecord::new(self.elapsed_ns(), Direction::SimToClient, bytes.to_vec());
        self.pcap_writer.write_record(&rec)?;
        Ok(())
    }

    /// Record a decoded trade tick — routed to the companion dbn stream when
    /// one is attached, otherwise a no-op.
    pub fn record_decoded_trade(&mut self, tick: TradeTick) -> Result<(), RecorderError> {
        if let Some(w) = self.dbn_writer.as_mut() {
            w.encode_trade(tick)?;
        }
        Ok(())
    }

    /// Flush both streams. Does NOT finalise the zstd frame — drop the
    /// recorder for that.
    pub fn flush(&mut self) -> Result<(), RecorderError> {
        self.pcap_writer.flush()?;
        if let Some(w) = self.dbn_writer.as_mut() {
            w.flush()?;
        }
        Ok(())
    }

    /// Finalise the recording: flush every stream *and* explicitly close
    /// the trailing zstd frame by consuming the underlying encoder. Use
    /// this on clean shutdown (SIGTERM / ctrl-c) to guarantee the file
    /// is decodable. If the process is SIGKILLed before `finalize` runs
    /// the zstd file is left unfinalised — `zstd::stream::Decoder` may
    /// be able to recover whole frames but the tail is lost.
    ///
    /// Consumes `self` so it can move the non-`Clone` inner writers.
    pub fn finalize(mut self) -> Result<(), RecorderError> {
        self.flush()?;
        // Drop the pcap writer explicitly. For Zstd this triggers
        // `AutoFinishEncoder`'s drop handler which calls `finish()`
        // internally, writing the zstd epilogue. For Raw it's just a
        // BufWriter drop (harmless).
        match self.pcap_writer {
            PcapSink::Raw(mut w) => {
                w.flush()?;
                drop(w);
            }
            PcapSink::Zstd(w) => {
                // Move out of the writer so the auto-finishing encoder
                // runs its drop immediately (before this function
                // returns), and any I/O error it surfaces propagates
                // through our `flush()` above. We can't call `finish()`
                // directly on `AutoFinishEncoder` because the wrapper
                // owns the signal; dropping it is the documented way to
                // finalise the frame.
                drop(w);
            }
        }
        if let Some(w) = self.dbn_writer {
            // Drop after an explicit flush above; no finish() is
            // needed for the dbn encoder since it's length-framed.
            drop(w);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::pcap::TwsPcapReader;
    use tempfile::TempDir;

    #[test]
    fn recorder_writes_both_directions_to_pcap() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("smoke");

        {
            let mut rec = Recorder::start(&stem, 210, false, None).unwrap();
            rec.record_client_to_sim(b"API\0v100..176").unwrap();
            rec.record_sim_to_client(b"176\x0020260418\0").unwrap();
            rec.record_client_to_sim(b"71\x001\x001\x00").unwrap();
            rec.flush().unwrap();
        }

        let pcap_path = {
            let mut p = stem.clone();
            p.set_extension("tws.pcap");
            p
        };
        let records = TwsPcapReader::open(&pcap_path).unwrap().read_all().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].direction, Direction::ClientToSim);
        assert_eq!(records[1].direction, Direction::SimToClient);
        // Timestamps are monotonic non-decreasing.
        assert!(records[0].ts_nanos_since_start <= records[1].ts_nanos_since_start);
        assert!(records[1].ts_nanos_since_start <= records[2].ts_nanos_since_start);
    }

    #[test]
    fn recorder_empty_bytes_is_noop() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("empty");
        {
            let mut rec = Recorder::start(&stem, 200, false, None).unwrap();
            rec.record_client_to_sim(b"").unwrap();
            rec.record_sim_to_client(b"").unwrap();
            rec.flush().unwrap();
        }
        let mut p = stem.clone();
        p.set_extension("tws.pcap");
        let records = TwsPcapReader::open(&p).unwrap().read_all().unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn recorder_zstd_compresses_on_disk() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("zst");
        {
            let mut rec = Recorder::start(&stem, 210, true, None).unwrap();
            // Write enough data that compression does something observable.
            for _ in 0..100 {
                rec.record_sim_to_client(&vec![b'A'; 4096]).unwrap();
            }
        }
        let mut p = stem.clone();
        p.set_extension("tws.pcap.zst");
        let size = std::fs::metadata(&p).unwrap().len();
        // 100 × 4096 bytes of repeating 'A' must compress to under 10% of raw.
        assert!(size < 40_000, "expected compressed size << raw, got {size}");
        let records = TwsPcapReader::open(&p).unwrap().read_all().unwrap();
        assert_eq!(records.len(), 100);
    }

    #[test]
    fn recorder_with_dbn_writes_both_files() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("withdbn");
        {
            let mut rec = Recorder::start(&stem, 210, false, Some("IB.SIM")).unwrap();
            rec.record_client_to_sim(b"hi").unwrap();
            rec.record_decoded_trade(TradeTick {
                ts_event_nanos: 1_000,
                instrument_id: 1,
                price: 100.0,
                size: 50,
            })
            .unwrap();
            rec.flush().unwrap();
        }
        let mut pcap = stem.clone();
        pcap.set_extension("tws.pcap");
        let mut dbn = stem.clone();
        dbn.set_extension("dbn");
        assert!(pcap.exists());
        assert!(dbn.exists());
    }
}
