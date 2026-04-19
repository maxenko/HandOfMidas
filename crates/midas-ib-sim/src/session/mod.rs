//! Stage 07 — session recording.
//!
//! Capture real IB paper-gateway sessions into a deterministic, replayable
//! format. Two artifacts per session:
//!
//! 1. `.tws.pcap` — our own binary log of wire bytes, append-only, optionally
//!    zstd-compressed. `[TwsPcapHeader][TwsPcapRecord × N]`.
//! 2. `.dbn`   — decoded market-data messages in Databento format, for the
//!    market-data replay engine.
//!
//! See `plan/ib-sim/07-session-recording.md` for the full design.

pub mod anonymize;
pub mod calibrate;
pub mod dbn_encoder;
pub mod pcap;
pub mod proxy;
pub mod recorder;
pub mod replayer;

pub use anonymize::{AnonymizeConfig, AnonymizeError, Anonymizer};
pub use calibrate::{calibrate_dbn, CalibratedPreset, CalibrationError};
pub use dbn_encoder::{DbnEncoder, DbnEncoderError};
pub use pcap::{
    Direction, TwsPcapHeader, TwsPcapReader, TwsPcapRecord, TwsPcapWriter, PCAP_MAGIC, PCAP_VERSION,
};
pub use proxy::{run_proxy, ProxyConfig, ProxyError};
pub use recorder::{Recorder, RecorderError};
pub use replayer::{ReplayEmission, ReplayMode, Replayer, ReplayerError};
