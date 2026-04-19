//! Client ↔ sim message enums. Stage 02 owns the variants' payloads; Stage 01
//! declares just the enum shell so handler match arms can be written against it.

pub mod fields;
pub mod incoming;
pub mod outgoing;

pub use self::incoming::IncomingMsg;
pub use self::outgoing::OutgoingMsg;
