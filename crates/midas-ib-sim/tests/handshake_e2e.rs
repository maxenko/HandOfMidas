//! Stage 02a — end-to-end handshake over a real TCP socket.
//!
//! Spins up a listener, accepts a connection, runs [`server_handshake`]
//! server-side, and a raw byte-sequence client-side. No `rust-ibapi` — that's
//! a Stage 02b/c concern once the message types exist.

use std::time::Duration;

use bytes::Bytes;
use midas_ib_sim::protocol::framing::{TwsCodec, TwsFrameCodec};
use midas_ib_sim::protocol::handshake::server_handshake;
use midas_ib_sim::protocol::{ServerVersion, MAX_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{Decoder, Encoder};

const LOCALHOST: &str = "127.0.0.1:0";

/// Minimal smoke: client sends `API\0 v176..221`, server handshakes and
/// replies with a valid `<version>\0<time>\0` frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_over_real_socket() {
    let listener = TcpListener::bind(LOCALHOST).await.expect("bind");
    let addr = listener.local_addr().unwrap();

    // Server: accept, handshake.
    let server_task = tokio::spawn(async move {
        let (mut stream, _peer) = listener.accept().await.expect("accept");
        let session = tokio::time::timeout(
            Duration::from_secs(2),
            server_handshake(&mut stream, ServerVersion::MIN, ServerVersion::MAX),
        )
        .await
        .expect("handshake timed out")
        .expect("handshake");
        session
    });

    // Client: write API prefix + version range, read reply.
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"API\0").await.unwrap();
    let range = b"v176..221";
    client
        .write_all(&(range.len() as u32).to_be_bytes())
        .await
        .unwrap();
    client.write_all(range).await.unwrap();

    let mut hdr = [0u8; 4];
    client.read_exact(&mut hdr).await.unwrap();
    let reply_len = u32::from_be_bytes(hdr) as usize;
    let mut reply = vec![0u8; reply_len];
    client.read_exact(&mut reply).await.unwrap();

    let session = server_task.await.expect("task join");
    // Reply must decode as "<version>\0<time>\0".
    let parts: Vec<&[u8]> = reply.split(|b| *b == 0).collect();
    // trailing NUL yields an empty final split — parts.len() >= 3
    assert!(parts.len() >= 2);
    let version_str = std::str::from_utf8(parts[0]).unwrap();
    let version_num: i32 = version_str.parse().unwrap();
    assert_eq!(version_num, session.version.raw());
    assert_eq!(version_num, MAX_VERSION);
    // Connection time should resemble "YYYYMMDD HH:MM:SS UTC".
    let ts = std::str::from_utf8(parts[1]).unwrap();
    assert!(ts.ends_with(" UTC"), "unexpected time format: {ts:?}");
    assert_eq!(ts, session.connection_time);
}

/// After the handshake, the caller switches the codec to `Framed` and can
/// encode + decode a RawFrame through the same socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_handshake_frame_roundtrip() {
    let listener = TcpListener::bind(LOCALHOST).await.expect("bind");
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut stream, _peer) = listener.accept().await.expect("accept");
        server_handshake(&mut stream, ServerVersion::MIN, ServerVersion::MAX)
            .await
            .expect("handshake");

        // Send a synthetic framed message back to the client. Use the
        // TwsFrameCodec against raw bytes (the structured codec is exercised
        // in the next test).
        let mut codec = TwsFrameCodec;
        let mut out = bytes::BytesMut::new();
        codec
            .encode(Bytes::from_static(b"9\x001\x00100\x00"), &mut out)
            .unwrap();
        stream.write_all(&out).await.unwrap();
        stream.flush().await.unwrap();
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"API\0").await.unwrap();
    let range = b"v176..221";
    client
        .write_all(&(range.len() as u32).to_be_bytes())
        .await
        .unwrap();
    client.write_all(range).await.unwrap();

    // Eat the handshake reply.
    let mut hdr = [0u8; 4];
    client.read_exact(&mut hdr).await.unwrap();
    let reply_len = u32::from_be_bytes(hdr) as usize;
    let mut reply = vec![0u8; reply_len];
    client.read_exact(&mut reply).await.unwrap();

    // Now read the synthetic post-handshake frame.
    let mut hdr = [0u8; 4];
    client.read_exact(&mut hdr).await.unwrap();
    let len = u32::from_be_bytes(hdr) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    assert_eq!(&payload[..], b"9\x001\x00100\x00");

    server_task.await.unwrap();
}

/// The `TwsCodec` in Framed state decodes the same synthetic frame the
/// handshake produces for subsequent traffic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_handshake_structured_codec_roundtrip() {
    let listener = TcpListener::bind(LOCALHOST).await.expect("bind");
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut stream, _peer) = listener.accept().await.expect("accept");
        server_handshake(&mut stream, ServerVersion::MIN, ServerVersion::MAX)
            .await
            .expect("handshake");

        let mut codec = TwsCodec::new();
        codec.set_framed();
        let frame = midas_ib_sim::protocol::framing::RawFrame {
            fields: vec![
                Bytes::from_static(b"9"),
                Bytes::from_static(b"1"),
                Bytes::from_static(b"100"),
                Bytes::from_static(b""),
            ],
        };
        let mut out = bytes::BytesMut::new();
        codec.encode(&frame, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();
        stream.flush().await.unwrap();
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"API\0").await.unwrap();
    let range = b"v201..221"; // rust-ibapi's current advertised range
    client
        .write_all(&(range.len() as u32).to_be_bytes())
        .await
        .unwrap();
    client.write_all(range).await.unwrap();

    // Drain the handshake reply.
    let mut hdr = [0u8; 4];
    client.read_exact(&mut hdr).await.unwrap();
    let reply_len = u32::from_be_bytes(hdr) as usize;
    let mut reply = vec![0u8; reply_len];
    client.read_exact(&mut reply).await.unwrap();

    // Decode the subsequent structured frame with TwsCodec.
    let mut client_codec = TwsCodec::new();
    client_codec.set_framed();
    let mut buf = bytes::BytesMut::new();
    // Pull bytes in a loop until we get a frame.
    loop {
        if let Some(frame) = client_codec.decode(&mut buf).unwrap() {
            assert_eq!(frame.fields.len(), 4);
            assert_eq!(&frame.fields[0][..], b"9");
            assert_eq!(&frame.fields[1][..], b"1");
            assert_eq!(&frame.fields[2][..], b"100");
            assert_eq!(&frame.fields[3][..], b"");
            break;
        }
        let mut tmp = [0u8; 64];
        let n = client.read(&mut tmp).await.unwrap();
        if n == 0 {
            panic!("server closed before emitting frame");
        }
        buf.extend_from_slice(&tmp[..n]);
    }

    server_task.await.unwrap();
}

/// A client whose advertised range doesn't overlap the sim's must be refused
/// before any reply bytes are written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_refuses_non_overlapping_version() {
    let listener = TcpListener::bind(LOCALHOST).await.expect("bind");
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut stream, _peer) = listener.accept().await.expect("accept");
        server_handshake(&mut stream, ServerVersion::MIN, ServerVersion::MAX).await
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"API\0").await.unwrap();
    let range = b"v100..150";
    client
        .write_all(&(range.len() as u32).to_be_bytes())
        .await
        .unwrap();
    client.write_all(range).await.unwrap();

    let result = server_task.await.expect("task join");
    assert!(result.is_err(), "server must reject non-overlapping range");
}
