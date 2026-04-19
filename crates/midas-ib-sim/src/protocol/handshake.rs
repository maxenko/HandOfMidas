//! TWS `API\0` handshake + version negotiation.
//!
//! Wire sequence (client → server):
//!
//! ```text
//! 1. Client sends raw bytes "API\0" (4 bytes, no length prefix).
//! 2. Client sends [u32 BE length] ASCII "v{min}..{max}" (literal "..").
//! 3. Server replies [u32 BE length] ASCII "<server_version>\0<connection_time>\0".
//! 4. Subsequent traffic is [u32 BE length][NUL-delimited fields].
//! ```
//!
//! The handshake is driven by reading directly off the socket rather than
//! through the [`TwsCodec`](crate::protocol::framing::TwsCodec) — the codec
//! stays in `PreHandshake` state during this phase and is switched to
//! `Framed` only once [`server_handshake`] returns successfully.

use bytes::{Bytes, BytesMut};
use chrono::Utc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{HandshakeError, ServerVersion, MAX_FRAME_LEN, MAX_VERSION, MIN_VERSION};

/// Upper bound on the client's initial version-range frame. It's always a
/// handful of bytes (`"v100..221"` is 9); allow 64 to absorb forward growth
/// and reject obvious garbage.
pub const MAX_VERSION_FRAME_LEN: u32 = 64;

/// Successful end-state of the handshake, post version negotiation.
#[derive(Clone, Debug)]
pub struct NegotiatedSession {
    /// The version both parties will use. Always within `MIN_VERSION..=MAX_VERSION`.
    pub version: ServerVersion,
    /// The "connection time" string the sim sent back to the client (the
    /// exact UTC timestamp we advertised). Parent code records it for
    /// diagnostics / replay.
    pub connection_time: String,
}

/// Legacy handshake handle retained from Stage 01's scaffolding.
#[derive(Clone, Debug)]
pub struct Handshake {
    pub client_version: i32,
    pub negotiated_version: i32,
    pub start_time: String,
}

impl From<NegotiatedSession> for Handshake {
    fn from(s: NegotiatedSession) -> Self {
        Self {
            client_version: s.version.raw(),
            negotiated_version: s.version.raw(),
            start_time: s.connection_time,
        }
    }
}

/// Run the TWS server-side handshake against a freshly accepted socket.
///
/// `my_min..=my_max` is the inclusive server version range this sim
/// advertises. Returns the negotiated session on success.
///
/// Negotiation policy: pick the **highest** version present in both ranges.
/// If the ranges don't overlap we return
/// [`HandshakeError::NoSupportedVersion`] — no attempt is made to fall back
/// to a version outside our advertised range.
pub async fn server_handshake<S>(
    stream: &mut S,
    my_min: ServerVersion,
    my_max: ServerVersion,
) -> Result<NegotiatedSession, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // 1. "API\0" prefix (no length).
    let mut prefix = [0u8; 4];
    stream.read_exact(&mut prefix).await?;
    if &prefix != b"API\0" {
        return Err(HandshakeError::BadPrefix);
    }

    // 2. Length-prefixed "v{min}..{max}".
    let frame = read_length_prefixed(stream, MAX_VERSION_FRAME_LEN).await?;
    let (client_min, client_max) = parse_version_range(&frame)?;

    // 3. Pick the highest mutually-supported version.
    let sim_min = my_min.raw();
    let sim_max = my_max.raw();
    let lo = client_min.max(sim_min);
    let hi = client_max.min(sim_max);
    if lo > hi {
        return Err(HandshakeError::NoSupportedVersion {
            client_min,
            client_max,
            sim_min,
            sim_max,
        });
    }
    let chosen = ServerVersion::new(hi).ok_or(HandshakeError::NoSupportedVersion {
        client_min,
        client_max,
        sim_min,
        sim_max,
    })?;

    // 4. Reply: "<version>\0<connection_time>\0". IB's format is
    //    "YYYYMMDD HH:MM:SS <TZ>"; we always advertise UTC.
    let connection_time = Utc::now().format("%Y%m%d %H:%M:%S UTC").to_string();
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(chosen.to_string().as_bytes());
    payload.push(0);
    payload.extend_from_slice(connection_time.as_bytes());
    payload.push(0);
    write_length_prefixed(stream, &payload).await?;

    Ok(NegotiatedSession {
        version: chosen,
        connection_time,
    })
}

/// Convenience wrapper that reads the handshake using the sim's default
/// advertised range (`MIN_VERSION..=MAX_VERSION`).
pub async fn server_handshake_default<S>(
    stream: &mut S,
) -> Result<NegotiatedSession, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    server_handshake(
        stream,
        ServerVersion::new(MIN_VERSION).expect("MIN_VERSION in range"),
        ServerVersion::new(MAX_VERSION).expect("MAX_VERSION in range"),
    )
    .await
}

// ---------------------------------------------------------------------------
// Length-prefixed read/write helpers.
// ---------------------------------------------------------------------------

async fn read_length_prefixed<S>(stream: &mut S, max_len: u32) -> Result<Bytes, HandshakeError>
where
    S: AsyncRead + Unpin,
{
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await?;
    let len = u32::from_be_bytes(hdr);
    if len > max_len.min(MAX_FRAME_LEN) {
        return Err(HandshakeError::BadVersionFrame(format!(
            "length-prefixed frame too large: {len}"
        )));
    }
    let mut buf = BytesMut::with_capacity(len as usize);
    buf.resize(len as usize, 0);
    stream.read_exact(&mut buf).await?;
    Ok(buf.freeze())
}

async fn write_length_prefixed<S>(stream: &mut S, payload: &[u8]) -> Result<(), HandshakeError>
where
    S: AsyncWrite + Unpin,
{
    let len = payload.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

/// Parse a `v{min}..{max}` client version frame. Returns `(min, max)`.
///
/// Accepts the pre-v100 legacy single-version form (`"123"`) too, treating
/// it as a degenerate `(v, v)` range — our advertised floor rejects it in
/// negotiation anyway.
fn parse_version_range(frame: &[u8]) -> Result<(i32, i32), HandshakeError> {
    let s = std::str::from_utf8(frame).map_err(|_| {
        HandshakeError::BadVersionFrame("version frame not valid utf-8".to_string())
    })?;

    let core = s.strip_prefix('v').unwrap_or(s);
    if let Some(idx) = core.find("..") {
        let (lo, hi) = core.split_at(idx);
        let hi = &hi[2..];
        let lo: i32 = lo
            .parse()
            .map_err(|e| HandshakeError::BadVersionFrame(format!("min: {e}")))?;
        let hi: i32 = hi
            .parse()
            .map_err(|e| HandshakeError::BadVersionFrame(format!("max: {e}")))?;
        if lo > hi {
            return Err(HandshakeError::BadVersionFrame(format!(
                "inverted range v{lo}..{hi}"
            )));
        }
        Ok((lo, hi))
    } else {
        // Legacy single-version (pre-v100). Treat as (v, v).
        let v: i32 = core
            .parse()
            .map_err(|e| HandshakeError::BadVersionFrame(format!("single version: {e}")))?;
        Ok((v, v))
    }
}

// ---------------------------------------------------------------------------
// Legacy shim — Stage-01 call sites named this entry point.
// ---------------------------------------------------------------------------

/// Stage-01 compatibility wrapper. New code should call [`server_handshake`]
/// directly — this thin wrapper exists so existing session stubs keep
/// compiling.
pub async fn perform_handshake<S>(stream: &mut S) -> Result<Handshake, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    server_handshake_default(stream).await.map(Handshake::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn parse_single_range() {
        assert_eq!(parse_version_range(b"v176..221").unwrap(), (176, 221));
    }

    #[test]
    fn parse_single_version_legacy() {
        assert_eq!(parse_version_range(b"99").unwrap(), (99, 99));
    }

    #[test]
    fn parse_rejects_inverted_range() {
        assert!(matches!(
            parse_version_range(b"v200..100"),
            Err(HandshakeError::BadVersionFrame(_))
        ));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(matches!(
            parse_version_range(b"not-a-version"),
            Err(HandshakeError::BadVersionFrame(_))
        ));
    }

    fn make_client_handshake(range: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"API\0");
        out.extend_from_slice(&(range.len() as u32).to_be_bytes());
        out.extend_from_slice(range);
        out
    }

    #[tokio::test]
    async fn handshake_happy_path() {
        let (mut client, mut server) = duplex(1024);

        // Server side: run handshake.
        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });

        // Client side: write API prefix + range, then read reply.
        let req = make_client_handshake(b"v176..221");
        client.write_all(&req).await.unwrap();

        let mut hdr = [0u8; 4];
        client.read_exact(&mut hdr).await.unwrap();
        let len = u32::from_be_bytes(hdr) as usize;
        let mut reply = vec![0u8; len];
        client.read_exact(&mut reply).await.unwrap();

        let session = server_fut.await.unwrap().unwrap();
        // Reply format: "<version>\0<time>\0"
        let reply_str = std::str::from_utf8(&reply).unwrap();
        let parts: Vec<&str> = reply_str.split('\0').collect();
        // Last element is empty (trailing NUL).
        assert!(parts.len() >= 2, "reply: {reply_str:?}");
        assert_eq!(parts[0], session.version.to_string());
        assert_eq!(session.version, ServerVersion::MAX);
    }

    #[tokio::test]
    async fn handshake_picks_highest_overlap() {
        let (mut client, mut server) = duplex(1024);

        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });

        // Client range: 150..200 — overlap with our 176..221 is 176..200,
        // so negotiation picks 200.
        let req = make_client_handshake(b"v150..200");
        client.write_all(&req).await.unwrap();

        let mut hdr = [0u8; 4];
        client.read_exact(&mut hdr).await.unwrap();
        let len = u32::from_be_bytes(hdr) as usize;
        let mut reply = vec![0u8; len];
        client.read_exact(&mut reply).await.unwrap();

        let session = server_fut.await.unwrap().unwrap();
        assert_eq!(session.version.raw(), 200);
    }

    #[tokio::test]
    async fn handshake_no_overlap_rejects() {
        let (mut client, mut server) = duplex(1024);
        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });
        // Client: v100..150 — no overlap with sim's 176..221.
        let req = make_client_handshake(b"v100..150");
        client.write_all(&req).await.unwrap();

        let err = server_fut.await.unwrap().unwrap_err();
        assert!(matches!(
            err,
            HandshakeError::NoSupportedVersion {
                client_min: 100,
                client_max: 150,
                sim_min: 176,
                sim_max: 221,
            }
        ));
    }

    #[tokio::test]
    async fn handshake_bad_prefix_rejects() {
        let (mut client, mut server) = duplex(1024);
        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });
        // Wrong prefix: "XYZ\0".
        client.write_all(b"XYZ\0").await.unwrap();
        // Give enough bytes to let the server advance.
        client.shutdown().await.ok();

        let err = server_fut.await.unwrap().unwrap_err();
        assert!(matches!(err, HandshakeError::BadPrefix));
    }

    #[tokio::test]
    async fn handshake_truncated_prefix_rejects() {
        let (mut client, mut server) = duplex(1024);
        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });
        // Only 2 bytes, then EOF.
        client.write_all(b"AP").await.unwrap();
        drop(client);

        let err = server_fut.await.unwrap().unwrap_err();
        assert!(matches!(err, HandshakeError::Io(_)));
    }

    #[tokio::test]
    async fn handshake_oversize_version_frame_rejects() {
        let (mut client, mut server) = duplex(8192);
        let server_fut = tokio::spawn(async move {
            server_handshake(&mut server, ServerVersion::MIN, ServerVersion::MAX).await
        });
        // Valid "API\0" prefix, then claim a 1 MiB version frame.
        client.write_all(b"API\0").await.unwrap();
        client
            .write_all(&(1_000_000u32).to_be_bytes())
            .await
            .unwrap();
        // Don't actually send that much; the error surfaces before read_exact.
        drop(client);

        let err = server_fut.await.unwrap().unwrap_err();
        assert!(matches!(err, HandshakeError::BadVersionFrame(_)));
    }

    #[tokio::test]
    async fn handshake_via_default_wrapper() {
        let (mut client, mut server) = duplex(1024);
        let server_fut = tokio::spawn(async move { perform_handshake(&mut server).await });
        let req = make_client_handshake(b"v176..221");
        client.write_all(&req).await.unwrap();
        let mut hdr = [0u8; 4];
        client.read_exact(&mut hdr).await.unwrap();
        let len = u32::from_be_bytes(hdr) as usize;
        let mut reply = vec![0u8; len];
        client.read_exact(&mut reply).await.unwrap();

        let h = server_fut.await.unwrap().unwrap();
        assert_eq!(h.negotiated_version, MAX_VERSION);
    }
}
