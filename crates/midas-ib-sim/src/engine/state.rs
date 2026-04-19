//! Per-session engine state.
//!
//! Stage 01 scaffold — the `SessionState` struct declares the fields every
//! handler will touch; Wave 2 stages fill in mutation logic via the engine
//! actor (never directly; no other module holds a `&mut Engine`).

use std::collections::BTreeSet;

use tokio::sync::broadcast;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{OrderId, ReqId, SessionId};
use crate::protocol::OutgoingMsg;

/// State we track for a single client connection.
#[derive(Debug)]
pub struct SessionState {
    pub id: SessionId,
    pub client_id: i32,
    pub peer_addr: String,
    pub connected_at: VirtualInstant,

    /// Active streaming L1 subscriptions (contributes to the 100-line cap).
    pub streaming_reqs: BTreeSet<ReqId>,
    /// Active tick-by-tick subscriptions (separate 5-line cap).
    pub tbt_reqs: BTreeSet<ReqId>,
    /// Active real-time 5-second bar subscriptions.
    pub rtbar_reqs: BTreeSet<ReqId>,
    /// Active historical-data requests (pending completion).
    pub historical_reqs: BTreeSet<ReqId>,

    /// Orders placed by this session (for `reqGlobalCancel` scoping).
    pub owned_orders: BTreeSet<OrderId>,

    /// Outbound wire sender — session's own TCP write half, wrapped in a
    /// broadcast-style sender so the engine can fan-out without coupling to
    /// the connection task.
    pub outbound: broadcast::Sender<OutgoingMsg>,

    /// Simple counters for observability.
    pub msgs_in: u64,
    pub msgs_out: u64,
}

impl SessionState {
    /// Construct an empty session record. Wave 2 Stage 02 fills in the real
    /// arguments once the handshake pipeline lands.
    pub fn new(
        id: SessionId,
        client_id: i32,
        peer_addr: String,
        connected_at: VirtualInstant,
        outbound: broadcast::Sender<OutgoingMsg>,
    ) -> Self {
        Self {
            id,
            client_id,
            peer_addr,
            connected_at,
            streaming_reqs: BTreeSet::new(),
            tbt_reqs: BTreeSet::new(),
            rtbar_reqs: BTreeSet::new(),
            historical_reqs: BTreeSet::new(),
            owned_orders: BTreeSet::new(),
            outbound,
            msgs_in: 0,
            msgs_out: 0,
        }
    }
}
