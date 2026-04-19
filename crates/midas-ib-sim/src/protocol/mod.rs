//! TWS wire protocol codec.
//!
//! Stage 02 owns this module. Stage 01 declares just the public shapes —
//! `IncomingMsg`, `OutgoingMsg`, `Handshake` — so other modules (sessions,
//! engine) can reference them.

pub mod framing;
pub mod handshake;
pub mod messages;

pub use self::messages::{IncomingMsg, OutgoingMsg};

/// Protocol version range advertised by the sim.
///
/// See ADR-003 (`plan/ib-sim/11-decisions.md`). We implement text framing only
/// across this range; protobuf (v201+) is explicitly not implemented.
pub const MIN_VERSION: i32 = 176;
pub const MAX_VERSION: i32 = 221;

/// Codec-level errors. Stage 02 fills in real variants.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    #[error("unsupported message id: {0}")]
    UnsupportedMessageId(i32),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(i32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
