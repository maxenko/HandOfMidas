//! Length-prefixed frame reader / writer. Stage 02 implements.

use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::protocol::CodecError;

/// Byte-level framing for TWS wire messages (4-byte big-endian length prefix +
/// payload). Stage 02 owns the implementation; Stage 01 ships only the struct
/// plus `Decoder`/`Encoder` impls with `todo!()` bodies so the main server
/// binary can name the type.
#[derive(Default, Debug)]
pub struct TwsFrameCodec;

impl Decoder for TwsFrameCodec {
    type Item = Bytes;
    type Error = CodecError;

    fn decode(&mut self, _src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        todo!("Stage 02 — TwsFrameCodec::decode")
    }
}

impl Encoder<Bytes> for TwsFrameCodec {
    type Error = CodecError;

    fn encode(&mut self, _item: Bytes, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        todo!("Stage 02 — TwsFrameCodec::encode")
    }
}
