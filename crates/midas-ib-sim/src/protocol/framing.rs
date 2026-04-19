//! Length-prefixed frame reader / writer.
//!
//! Post-handshake TWS traffic is `[u32 BE length][NUL-delimited ASCII payload]`.
//! This module implements both the one-shot frame-codec (`TwsFrameCodec`, which
//! round-trips raw payload bytes) and the richer `TwsCodec` state machine
//! which emits structured [`RawFrame`] values (one frame = a vector of
//! NUL-delimited field payloads).
//!
//! ## State machine
//!
//! The TWS connection has two phases:
//! - **Pre-handshake**: raw bytes flowing, driven explicitly by
//!   [`handshake::server_handshake`](crate::protocol::handshake::server_handshake)
//!   reading directly off the socket. In this state `Decoder::decode` returns
//!   `Ok(None)` without consuming bytes — the codec is effectively dormant.
//! - **Framed**: length-prefixed frames. `Decoder::decode` reads the 4-byte BE
//!   length header, then the payload, splits on NUL into fields, and emits a
//!   [`RawFrame`].
//!
//! Callers transition from `PreHandshake` → `Framed` by calling
//! [`TwsCodec::set_framed`] once the handshake completes.

use bytes::{Buf, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::protocol::{ProtocolError, MAX_FRAME_LEN};

/// Codec state — pre-handshake bytes pass through untouched; post-handshake
/// bytes are length-framed and split into NUL-delimited fields.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum CodecState {
    /// Initial state. Decoder yields nothing; caller drives the socket
    /// directly to handshake.
    #[default]
    PreHandshake,
    /// Framed mode — length-prefixed, NUL-delimited fields.
    Framed,
}

/// A single TWS wire frame, post length-prefix strip and NUL split.
///
/// The protocol is tolerant of trailing empty fields (many IB messages end
/// with one or more empty optional fields). We preserve them exactly as
/// received so downstream parsers can rely on stable field counts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RawFrame {
    /// NUL-separated payload fields. An N-NUL payload yields N fields (the
    /// final trailing NUL terminates the last field rather than producing
    /// an extra empty one, matching the IB convention).
    pub fields: Vec<Bytes>,
}

impl RawFrame {
    /// Construct a frame from already-split fields.
    pub fn new(fields: Vec<Bytes>) -> Self {
        Self { fields }
    }

    /// Serialise the payload (without the length prefix): fields joined by
    /// NUL with a terminating NUL.
    ///
    /// An empty `fields` vec produces an empty payload, not a single NUL.
    pub fn encode_payload(&self) -> BytesMut {
        let total: usize = self.fields.iter().map(|f| f.len() + 1).sum();
        let mut out = BytesMut::with_capacity(total);
        for f in &self.fields {
            out.extend_from_slice(f);
            out.extend_from_slice(b"\0");
        }
        out
    }
}

// ---------------------------------------------------------------------------
// TwsCodec — the state-machine-driven structured codec.
// ---------------------------------------------------------------------------

/// Stateful TWS codec. Post-handshake decodes `[u32 BE length][payload]`
/// frames into [`RawFrame`] values; pre-handshake is a no-op that defers to
/// the handshake driver reading the socket directly.
#[derive(Default, Debug)]
pub struct TwsCodec {
    state: CodecState,
}

impl TwsCodec {
    pub fn new() -> Self {
        Self::default()
    }

    /// Explicitly start in `Framed` state — useful for tests that don't want
    /// to run the handshake.
    pub fn new_framed() -> Self {
        Self {
            state: CodecState::Framed,
        }
    }

    /// Transition to the `Framed` state. Called by the session driver once
    /// [`server_handshake`](crate::protocol::handshake::server_handshake)
    /// returns.
    pub fn set_framed(&mut self) {
        self.state = CodecState::Framed;
    }

    pub fn state(&self) -> CodecState {
        self.state
    }
}

impl Decoder for TwsCodec {
    type Item = RawFrame;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.state {
            CodecState::PreHandshake => {
                // The handshake reads the socket directly; the codec is dormant.
                Ok(None)
            }
            CodecState::Framed => decode_framed(src),
        }
    }
}

impl Encoder<RawFrame> for TwsCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: RawFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        encode_framed(&item, dst)
    }
}

impl Encoder<&RawFrame> for TwsCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: &RawFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        encode_framed(item, dst)
    }
}

/// Decode a single length-prefixed frame from `src`. Returns `Ok(None)` if
/// not enough bytes are buffered yet. On success the consumed bytes are
/// drained from `src`.
fn decode_framed(src: &mut BytesMut) -> Result<Option<RawFrame>, ProtocolError> {
    // Need at least 4 bytes for the length prefix.
    if src.len() < 4 {
        // Reserve more capacity so the framed reader can make progress.
        src.reserve(4 - src.len());
        return Ok(None);
    }

    // Peek at the length prefix without consuming.
    let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    let len = len as usize;
    let frame_total = 4 + len;
    if src.len() < frame_total {
        src.reserve(frame_total - src.len());
        return Ok(None);
    }

    // Commit: consume the prefix, then the payload.
    src.advance(4);
    let payload = src.split_to(len).freeze();
    Ok(Some(split_payload(&payload)))
}

/// Split a NUL-delimited payload into fields. A final trailing NUL is
/// treated as the terminator of the last field, not as an extra empty field.
/// An empty payload yields an empty `fields` vec.
fn split_payload(payload: &Bytes) -> RawFrame {
    if payload.is_empty() {
        return RawFrame::default();
    }

    let mut fields = Vec::new();
    let mut start = 0usize;
    for (i, &b) in payload.iter().enumerate() {
        if b == 0 {
            fields.push(payload.slice(start..i));
            start = i + 1;
        }
    }
    // If the payload did not end with NUL, pick up the trailing chunk as
    // another field (protocol tolerance — real IB traffic always NUL-terminates).
    if start < payload.len() {
        fields.push(payload.slice(start..payload.len()));
    }
    RawFrame { fields }
}

/// Encode a frame: `[u32 BE length][field1 NUL field2 NUL ... NUL]`.
fn encode_framed(frame: &RawFrame, dst: &mut BytesMut) -> Result<(), ProtocolError> {
    let payload_len: usize = frame.fields.iter().map(|f| f.len() + 1).sum();
    if payload_len > MAX_FRAME_LEN as usize {
        return Err(ProtocolError::FrameTooLarge(payload_len as u32));
    }

    dst.reserve(4 + payload_len);
    dst.extend_from_slice(&(payload_len as u32).to_be_bytes());
    for f in &frame.fields {
        dst.extend_from_slice(f);
        dst.extend_from_slice(b"\0");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TwsFrameCodec — the thin byte-payload codec (back-compat name).
// ---------------------------------------------------------------------------

/// Byte-level framing for TWS wire messages. Decodes a single payload as raw
/// `Bytes`, encodes a `Bytes` payload with the length prefix. Useful for the
/// handshake driver which doesn't need field-level parsing.
#[derive(Default, Debug)]
pub struct TwsFrameCodec;

impl Decoder for TwsFrameCodec {
    type Item = Bytes;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
        if len > MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(len));
        }
        let len = len as usize;
        if src.len() < 4 + len {
            return Ok(None);
        }
        src.advance(4);
        Ok(Some(src.split_to(len).freeze()))
    }
}

impl Encoder<Bytes> for TwsFrameCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: Bytes, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if item.len() > MAX_FRAME_LEN as usize {
            return Err(ProtocolError::FrameTooLarge(item.len() as u32));
        }
        dst.reserve(4 + item.len());
        dst.extend_from_slice(&(item.len() as u32).to_be_bytes());
        dst.extend_from_slice(&item);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn make_frame(fields: &[&[u8]]) -> RawFrame {
        RawFrame {
            fields: fields.iter().map(|f| Bytes::copy_from_slice(f)).collect(),
        }
    }

    // ---- RawFrame payload shape -----------------------------------------

    #[test]
    fn payload_encodes_nul_terminated() {
        let frame = make_frame(&[b"1", b"71", b"0"]);
        let payload = frame.encode_payload();
        assert_eq!(&payload[..], b"1\x0071\x000\x00");
    }

    #[test]
    fn payload_empty_frame_is_empty() {
        let frame = RawFrame::default();
        assert!(frame.encode_payload().is_empty());
    }

    #[test]
    fn payload_preserves_trailing_empty_fields() {
        let frame = make_frame(&[b"a", b"", b""]);
        let payload = frame.encode_payload();
        assert_eq!(&payload[..], b"a\x00\x00\x00");
    }

    // ---- TwsFrameCodec roundtrip ----------------------------------------

    #[test]
    fn frame_codec_roundtrip_simple() {
        let mut codec = TwsFrameCodec;
        let mut buf = BytesMut::new();
        let payload = Bytes::from_static(b"9\x001\x00100\x00");
        codec.encode(payload.clone(), &mut buf).unwrap();
        // Payload = "9" NUL "1" NUL "100" NUL = 8 bytes.
        assert_eq!(&buf[..4], &[0, 0, 0, 8]);
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, payload);
        assert!(buf.is_empty());
    }

    #[test]
    fn frame_codec_rejects_oversize_prefix() {
        let mut codec = TwsFrameCodec;
        let mut buf = BytesMut::from(&[0xFFu8, 0xFF, 0xFF, 0xFF][..]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(_)));
    }

    // ---- TwsCodec state machine -----------------------------------------

    #[test]
    fn codec_pre_handshake_does_not_consume() {
        let mut codec = TwsCodec::new();
        assert_eq!(codec.state(), CodecState::PreHandshake);
        let mut buf = BytesMut::from(&[0u8, 0, 0, 3, b'a', 0, 0][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
        // Bytes are untouched.
        assert_eq!(buf.len(), 7);
    }

    #[test]
    fn codec_transitions_to_framed() {
        let mut codec = TwsCodec::new();
        codec.set_framed();
        assert_eq!(codec.state(), CodecState::Framed);
    }

    // ---- TwsCodec roundtrip ---------------------------------------------

    #[test]
    fn tws_codec_roundtrip_single_frame() {
        let mut codec = TwsCodec::new_framed();
        let frame = make_frame(&[b"9", b"1", b"100"]);
        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        // Payload = "9\01\0100\0" = 8 bytes.
        assert_eq!(&buf[..4], &[0, 0, 0, 8]);
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, frame);
        assert!(buf.is_empty());
    }

    #[test]
    fn tws_codec_preserves_trailing_empty_fields() {
        let mut codec = TwsCodec::new_framed();
        // 4 fields, last two empty.
        let frame = make_frame(&[b"msg", b"v1", b"", b""]);
        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(decoded.fields.len(), 4);
        assert_eq!(decoded.fields[2], Bytes::new());
        assert_eq!(decoded.fields[3], Bytes::new());
    }

    #[test]
    fn tws_codec_handles_empty_payload() {
        let mut codec = TwsCodec::new_framed();
        // A zero-length frame is legal per spec.
        let mut buf = BytesMut::from(&[0u8, 0, 0, 0][..]);
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert!(decoded.fields.is_empty());
    }

    #[test]
    fn tws_codec_back_to_back_frames() {
        let mut codec = TwsCodec::new_framed();
        let f1 = make_frame(&[b"1", b"a"]);
        let f2 = make_frame(&[b"2", b"b"]);
        let mut buf = BytesMut::new();
        codec.encode(f1.clone(), &mut buf).unwrap();
        codec.encode(f2.clone(), &mut buf).unwrap();

        let d1 = codec.decode(&mut buf).unwrap().unwrap();
        let d2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(d1, f1);
        assert_eq!(d2, f2);
        // Third read returns None (buffer drained).
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn tws_codec_partial_frame_byte_by_byte() {
        let mut codec = TwsCodec::new_framed();
        let frame = make_frame(&[b"hello", b"world"]);
        let mut full = BytesMut::new();
        codec.encode(frame.clone(), &mut full).unwrap();

        // Feed byte-by-byte; expect None until the last byte is in.
        let mut sink = BytesMut::new();
        for (i, b) in full.iter().copied().enumerate() {
            sink.extend_from_slice(&[b]);
            let got = codec.decode(&mut sink).unwrap();
            if i + 1 < full.len() {
                assert!(got.is_none(), "frame decoded early at byte {i}");
            } else {
                assert_eq!(got, Some(frame.clone()));
            }
        }
        assert!(sink.is_empty());
    }

    #[test]
    fn tws_codec_rejects_oversize_frame() {
        let mut codec = TwsCodec::new_framed();
        let mut buf = BytesMut::from(&[0xFFu8, 0xFF, 0xFF, 0xFF][..]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(_)));
    }

    #[test]
    fn tws_codec_roundtrip_with_binary_field_data() {
        // Fields may carry any byte except NUL (strings are ASCII but the
        // codec itself is byte-transparent). Verify non-ASCII bytes survive.
        let mut codec = TwsCodec::new_framed();
        let frame = make_frame(&[b"\x01\x02\x03", b"tail"]);
        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, frame);
    }

    // ---- Proptest: arbitrary frames roundtrip ---------------------------

    proptest! {
        #[test]
        fn prop_roundtrip_arbitrary_frames(
            fields in prop::collection::vec(
                prop::collection::vec(any::<u8>().prop_filter("no NUL", |b| *b != 0), 0..32),
                0..16
            )
        ) {
            let frame = RawFrame {
                fields: fields.iter().map(|f| Bytes::copy_from_slice(f)).collect()
            };
            let mut codec = TwsCodec::new_framed();
            let mut buf = BytesMut::new();
            codec.encode(frame.clone(), &mut buf).unwrap();
            let decoded = codec.decode(&mut buf).unwrap().unwrap();
            prop_assert_eq!(decoded, frame);
            prop_assert!(buf.is_empty());
        }

        #[test]
        fn prop_concatenated_frames_all_decode(
            frames in prop::collection::vec(
                prop::collection::vec(
                    prop::collection::vec(any::<u8>().prop_filter("no NUL", |b| *b != 0), 0..16),
                    0..8,
                ),
                0..6,
            )
        ) {
            let original: Vec<RawFrame> = frames.iter().map(|fields| RawFrame {
                fields: fields.iter().map(|f| Bytes::copy_from_slice(f)).collect(),
            }).collect();

            let mut codec = TwsCodec::new_framed();
            let mut buf = BytesMut::new();
            for f in &original {
                codec.encode(f.clone(), &mut buf).unwrap();
            }
            let mut decoded = Vec::new();
            while let Some(f) = codec.decode(&mut buf).unwrap() {
                decoded.push(f);
            }
            prop_assert_eq!(decoded, original);
            prop_assert!(buf.is_empty());
        }

        #[test]
        fn prop_decoder_never_panics_on_random_bytes(data in prop::collection::vec(any::<u8>(), 0..2048)) {
            let mut codec = TwsCodec::new_framed();
            let mut buf = BytesMut::from(&data[..]);
            // Decode until we get None or an error — must not panic.
            while let Ok(Some(_)) = codec.decode(&mut buf) {
                // Keep consuming frames.
            }
        }
    }
}
