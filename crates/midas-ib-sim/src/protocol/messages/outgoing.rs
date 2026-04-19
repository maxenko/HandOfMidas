//! Sim → client message types. Stage 02 fills in the real variants.

use bytes::Bytes;

/// Sim-originated wire message, pre-encode. Stage 02 replaces `Raw` with the
/// concrete variant set (`TickPrice`, `OpenOrder`, `OrderStatus`, `ErrMsg`, …).
#[derive(Clone, Debug)]
pub enum OutgoingMsg {
    Raw(Bytes),
}
