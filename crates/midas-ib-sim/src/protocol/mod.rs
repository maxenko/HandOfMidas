//! TWS wire protocol codec.
//!
//! Stage 02a implements the foundation layer: framing, field codec,
//! handshake, and the central types (`ServerVersion`, `RawFrame`,
//! `NegotiatedSession`). Stage 02b / 02c layer concrete `IncomingMsg` /
//! `OutgoingMsg` message parsers on top.

pub mod framing;
pub mod handshake;
pub mod messages;

pub use self::framing::{CodecState, RawFrame, TwsCodec, TwsFrameCodec};
pub use self::handshake::{perform_handshake, server_handshake, Handshake, NegotiatedSession};
pub use self::messages::{IncomingMsg, OutgoingMsg};

/// Protocol version range advertised by the sim.
///
/// See ADR-003 (`plan/ib-sim/11-decisions.md`). We implement text framing only
/// across this range; protobuf (v201+) is explicitly not implemented.
pub const MIN_VERSION: i32 = 176;
pub const MAX_VERSION: i32 = 221;

/// Strongly-typed TWS server version. Values outside `MIN_VERSION..=MAX_VERSION`
/// are rejected at construction.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ServerVersion(i32);

impl ServerVersion {
    /// Lowest version the sim advertises.
    pub const MIN: ServerVersion = ServerVersion(MIN_VERSION);
    /// Highest version the sim advertises.
    pub const MAX: ServerVersion = ServerVersion(MAX_VERSION);

    /// Construct a `ServerVersion`, returning `None` if it sits outside the
    /// advertised range.
    pub fn new(v: i32) -> Option<Self> {
        if (MIN_VERSION..=MAX_VERSION).contains(&v) {
            Some(ServerVersion(v))
        } else {
            None
        }
    }

    /// Raw i32 version number used on the wire.
    pub fn raw(self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for ServerVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Codec-level errors surfaced during frame decode/encode.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    #[error("frame length {0} exceeds maximum of {max} bytes", max = MAX_FRAME_LEN)]
    FrameTooLarge(u32),
    #[error("unsupported message id: {0}")]
    UnsupportedMsgId(i32),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(i32),
    #[error("field decode error: {0}")]
    Field(String),
    #[error("utf-8 decode error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("integer parse error: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    #[error("float parse error: {0}")]
    ParseFloat(#[from] std::num::ParseFloatError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors surfaced during the `API\0` + version handshake.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("expected `API\\0` prefix")]
    BadPrefix,
    #[error("version frame malformed: {0}")]
    BadVersionFrame(String),
    #[error(
        "no overlap between client range v{client_min}..{client_max} and sim range v{sim_min}..{sim_max}"
    )]
    NoSupportedVersion {
        client_min: i32,
        client_max: i32,
        sim_min: i32,
        sim_max: i32,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
}

/// Backwards-compatible alias retained for Stage-01 call sites that named
/// the old type. `ProtocolError` is the canonical name for Stage 02+.
pub type CodecError = ProtocolError;

/// Hard cap on a single frame's payload. 16 MiB — an order of magnitude above
/// anything IB has ever emitted, still well under memory blow-up territory.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
