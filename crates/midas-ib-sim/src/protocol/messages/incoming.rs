//! Client → sim message types. Stage 02 fills in the real variants;
//! Stage 01 ships an open shell so engine handlers can match on it.

use bytes::Bytes;

/// Client-originated wire message, post-decode.
#[derive(Clone, Debug)]
pub enum IncomingMsg {
    /// Raw frame bytes pending decode. Stage 02 replaces this with the real
    /// variant set (`StartApi`, `PlaceOrder`, `ReqMktData`, …) ~40 variants.
    Raw(Bytes),
}
