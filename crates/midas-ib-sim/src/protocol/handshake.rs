//! TWS `API\0` handshake + version negotiation. Stage 02 implements.

use crate::protocol::CodecError;

/// Result of a successful client handshake.
#[derive(Clone, Debug)]
pub struct Handshake {
    pub client_version: i32,
    pub negotiated_version: i32,
    pub start_time: String,
}

/// Parse the initial bytes of a new connection up to and including the
/// client's `START_API` message, returning the negotiated version.
///
/// Stage 02 implementation TBD; stub signature lets session tasks name it.
pub async fn perform_handshake<R>(_reader: &mut R) -> Result<Handshake, CodecError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    todo!("Stage 02 — perform_handshake")
}
