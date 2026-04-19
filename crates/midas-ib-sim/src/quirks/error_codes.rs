//! Canonical IB error code table. Every sim-emitted error routes through a
//! constant here so the pre-release real-IB verification pass is a single-file
//! diff (see `plan/ib-sim/05-quirk-modeling.md` §"Real-IB error code
//! validation").
//!
//! # Verification tags
//!
//! Each constant is tagged with a trailing comment:
//!
//! * `// VERIFIED` — value + message text confirmed against a captured real-IB
//!   paper session. Safe to ship.
//! * `// [unverified]` — sourced from IB docs / client samples / research. The
//!   pre-release capture pass (see the plan §"Real-IB error code validation")
//!   must confirm the value and flip the tag.
//!
//! The tags are documentation, not runtime flags — nothing checks them at
//! compile time. They exist so the engineer running the capture session has a
//! single-file grep target: any line still saying `[unverified]` is a
//! pre-release gate.

// ---------------------------------------------------------------------------
// Client / order errors (100-500 range).
// ---------------------------------------------------------------------------

/// 50 msg/sec ceiling exceeded — real IB emits this then drops the socket.
pub const MSG_RATE_EXCEEDED: i32 = 100; // VERIFIED

/// Client sent two `PLACE_ORDER` messages with the same `orderId`.
pub const DUPLICATE_ORDER_ID: i32 = 103; // VERIFIED

/// Attempt to modify an order that already filled.
pub const CANT_MODIFY_FILLED: i32 = 104; // VERIFIED

/// Limit/Stop price doesn't conform to the contract's minimum tick.
pub const PRICE_NOT_MIN_TICK: i32 = 110; // VERIFIED

/// Historical-data pacing violation — covers all three regimes (60/10min,
/// 6 identical in 2s, 15s identical cooldown).
pub const HISTORICAL_PACING: i32 = 162; // [unverified]

/// No security definition found for the requested contract.
pub const NO_SECURITY_DEF: i32 = 200; // VERIFIED

/// Order rejected by IB — reason is appended to the message.
pub const ORDER_REJECTED: i32 = 201; // VERIFIED

/// Order cancelled — informational.
pub const ORDER_CANCELLED: i32 = 202; // VERIFIED

/// `clientId` already in use — up to 32 concurrent connections per TWS.
pub const CLIENT_ID_IN_USE: i32 = 326; // VERIFIED

/// Requested market data isn't subscribed; falls back to delayed data when
/// `reqMarketDataType(3)` is in effect.
pub const MD_NOT_SUBSCRIBED: i32 = 354; // VERIFIED

// ---------------------------------------------------------------------------
// System / connectivity (1000s range). Emitted via `error()` with
// `orderId = -1`.
// ---------------------------------------------------------------------------

/// Connectivity between IB servers and TWS lost — gate order/data requests.
pub const FARM_LOST: i32 = 1100; // VERIFIED

/// Connectivity restored; streaming market data was dropped. Clients must
/// re-subscribe every `reqMktData` / `reqMktDepth` / `reqRealTimeBars`.
pub const FARM_RESTORED_NO_DATA: i32 = 1101; // VERIFIED

/// Connectivity restored; streaming data maintained. No client action needed.
pub const FARM_RESTORED_DATA: i32 = 1102; // VERIFIED

/// TWS restarting (~11:45 PM ET weekdays). Clients must reconnect.
pub const TWS_DAILY_RESTART: i32 = 1300; // [unverified]

// ---------------------------------------------------------------------------
// Data-farm status bulletins (2100s range). Emitted with `orderId = -1`;
// `reqId = -1` on the wire. Clients gate data requests on these.
// ---------------------------------------------------------------------------

/// One market-data farm is broken — e.g. `usfarm`. Paired with 2104 when the
/// farm returns. Emitted during periodic farm cycling (T2 feature flag).
pub const MD_FARM_BROKEN: i32 = 2103; // [unverified]

/// Market-data farm connection is OK — paired with a farm name like
/// `usfarm`. Emitted on every session start as part of the unsolicited
/// farm-status bulletin triplet.
pub const MD_FARM_OK_USFARM: i32 = 2104; // VERIFIED

/// HMDS (historical-data) farm is broken.
pub const HMDS_FARM_BROKEN: i32 = 2105; // [unverified]

/// HMDS data farm connection is OK — paired with `ushmds`. Emitted on session
/// start.
pub const HMDS_FARM_OK_USHMDS: i32 = 2106; // VERIFIED

/// Sec-def server disconnected — transient, usually recovers within seconds.
pub const SEC_DEF_FARM_DISCONNECTED: i32 = 2108; // [unverified]

/// Sec-def data farm connection is OK — paired with `secdefil`. Emitted on
/// session start.
pub const SEC_DEF_FARM_OK: i32 = 2158; // VERIFIED

// ---------------------------------------------------------------------------
// Advanced orders / algo / order-management (10000+ range).
// ---------------------------------------------------------------------------

/// Cancel arrived before the corresponding place-order was transmitted.
pub const ORDER_NOT_YET_TRANSMITTED: i32 = 10147; // VERIFIED

/// Streaming line cap exceeded (100 L1 tickers per session by default).
pub const LINE_CAP_OVERFLOW: i32 = 10197; // [unverified]

/// Canonical message text for a given error code. Returned as `&'static str`
/// because these strings ship with the binary and never allocate.
///
/// Unknown codes resolve to `"Unknown error code"` so the caller can still
/// emit an `ErrMsg` frame even if a new constant lands before its message is
/// added here.
pub fn message(code: i32) -> &'static str {
    match code {
        // Client / order errors.
        MSG_RATE_EXCEEDED => "Max rate of messages per second has been exceeded.",
        DUPLICATE_ORDER_ID => "Duplicate order id",
        CANT_MODIFY_FILLED => "Cannot modify a filled order",
        PRICE_NOT_MIN_TICK => {
            "The price does not conform to the minimum price variation for this contract"
        }
        HISTORICAL_PACING => {
            "Historical Market Data Service error message: Historical data request pacing violation"
        }
        NO_SECURITY_DEF => "No security definition has been found for the request",
        ORDER_REJECTED => "Order rejected - reason",
        ORDER_CANCELLED => "Order cancelled",
        CLIENT_ID_IN_USE => "Client id is already in use",
        MD_NOT_SUBSCRIBED => {
            "Requested market data is not subscribed. Displaying delayed market data."
        }

        // Connectivity / system.
        FARM_LOST => "Connectivity between IB and TWS has been lost.",
        FARM_RESTORED_NO_DATA => {
            "Connectivity between IB and Trader Workstation has been restored - data lost."
        }
        FARM_RESTORED_DATA => {
            "Connectivity between IB and Trader Workstation has been restored - data maintained."
        }
        TWS_DAILY_RESTART => "TWS is restarting. Please reconnect.",

        // Data-farm bulletins.
        MD_FARM_BROKEN => "Market data farm connection is broken:usfarm",
        MD_FARM_OK_USFARM => "Market data farm connection is OK:usfarm",
        HMDS_FARM_BROKEN => "HMDS data farm connection is broken:ushmds",
        HMDS_FARM_OK_USHMDS => "HMDS data farm connection is OK:ushmds",
        SEC_DEF_FARM_DISCONNECTED => "Sec-def data farm connection is disconnected:secdefil",
        SEC_DEF_FARM_OK => "Sec-def data farm connection is OK:secdefil",

        // Advanced orders / algo.
        ORDER_NOT_YET_TRANSMITTED => "OrderId that needs to be cancelled is not yet transmitted.",
        LINE_CAP_OVERFLOW => "Max number of tickers has been reached",

        _ => "Unknown error code",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_messages_match_table() {
        // Sanity: each code in the table returns its canonical text, not the
        // fallback. If a future edit accidentally drops a match arm, this
        // flags the regression.
        assert_eq!(
            message(MSG_RATE_EXCEEDED),
            "Max rate of messages per second has been exceeded."
        );
        assert_eq!(
            message(LINE_CAP_OVERFLOW),
            "Max number of tickers has been reached"
        );
        assert_eq!(
            message(HISTORICAL_PACING),
            "Historical Market Data Service error message: Historical data request pacing violation"
        );
        assert_eq!(
            message(MD_FARM_OK_USFARM),
            "Market data farm connection is OK:usfarm"
        );
        assert_eq!(
            message(HMDS_FARM_OK_USHMDS),
            "HMDS data farm connection is OK:ushmds"
        );
        assert_eq!(
            message(SEC_DEF_FARM_OK),
            "Sec-def data farm connection is OK:secdefil"
        );
        assert_eq!(
            message(FARM_LOST),
            "Connectivity between IB and TWS has been lost."
        );
        assert_eq!(message(CLIENT_ID_IN_USE), "Client id is already in use");
        assert!(!message(MD_NOT_SUBSCRIBED).is_empty());
    }

    #[test]
    fn unknown_code_falls_back() {
        assert_eq!(message(-999), "Unknown error code");
        assert_eq!(message(42), "Unknown error code");
    }

    #[test]
    fn constants_have_expected_values() {
        // Guard against accidental value drift — each of these is the value
        // the plan froze; any change must be justified against a real-IB
        // capture.
        assert_eq!(MSG_RATE_EXCEEDED, 100);
        assert_eq!(DUPLICATE_ORDER_ID, 103);
        assert_eq!(HISTORICAL_PACING, 162);
        assert_eq!(CLIENT_ID_IN_USE, 326);
        assert_eq!(MD_NOT_SUBSCRIBED, 354);
        assert_eq!(FARM_LOST, 1100);
        assert_eq!(FARM_RESTORED_NO_DATA, 1101);
        assert_eq!(FARM_RESTORED_DATA, 1102);
        assert_eq!(TWS_DAILY_RESTART, 1300);
        assert_eq!(MD_FARM_BROKEN, 2103);
        assert_eq!(MD_FARM_OK_USFARM, 2104);
        assert_eq!(HMDS_FARM_BROKEN, 2105);
        assert_eq!(HMDS_FARM_OK_USHMDS, 2106);
        assert_eq!(SEC_DEF_FARM_DISCONNECTED, 2108);
        assert_eq!(SEC_DEF_FARM_OK, 2158);
        assert_eq!(ORDER_NOT_YET_TRANSMITTED, 10147);
        assert_eq!(LINE_CAP_OVERFLOW, 10197);
    }
}
