//! Canonical IB error code table. Every sim-emitted error routes through a
//! constant here so the pre-release real-IB verification pass is a single-file
//! diff (see `plan/ib-sim/05-quirk-modeling.md` §"Real-IB error code
//! validation").

// VERIFIED = captured from real IB paper in the research phase.
// [unverified] = sourced from docs / IB samples; must be confirmed in the
// pre-release validation pass and the label updated.

pub const MSG_RATE_EXCEEDED: i32 = 100; // VERIFIED
pub const DUPLICATE_ORDER_ID: i32 = 103; // VERIFIED
pub const CANT_MODIFY_FILLED: i32 = 104; // VERIFIED
pub const PRICE_NOT_MIN_TICK: i32 = 110; // VERIFIED
pub const HISTORICAL_PACING: i32 = 162; // [unverified]
pub const NO_SECURITY_DEF: i32 = 200; // VERIFIED
pub const ORDER_REJECTED: i32 = 201; // VERIFIED
pub const ORDER_CANCELLED: i32 = 202; // VERIFIED
pub const CLIENT_ID_IN_USE: i32 = 326; // VERIFIED
pub const MD_NOT_SUBSCRIBED: i32 = 354; // VERIFIED
pub const ORDER_NOT_YET_TRANSMITTED: i32 = 10147; // VERIFIED
pub const LINE_CAP_OVERFLOW: i32 = 10197; // [unverified]
pub const FARM_LOST: i32 = 1100; // VERIFIED
pub const FARM_RESTORED_NO_DATA: i32 = 1101; // VERIFIED
pub const FARM_RESTORED_DATA: i32 = 1102; // VERIFIED
pub const TWS_DAILY_RESTART: i32 = 1300; // [unverified]
pub const MD_FARM_OK_USFARM: i32 = 2104; // VERIFIED
pub const HMDS_FARM_OK_USHMDS: i32 = 2106; // VERIFIED
pub const SEC_DEF_FARM_OK: i32 = 2158; // VERIFIED

/// Canonical message text for a given error code. Returned as `&'static str`
/// because these strings ship with the binary and never allocate.
pub fn message(code: i32) -> &'static str {
    match code {
        MSG_RATE_EXCEEDED => "Max rate of messages per second has been exceeded.",
        LINE_CAP_OVERFLOW => "Max number of tickers has been reached",
        HISTORICAL_PACING => {
            "Historical Market Data Service error message: Historical data request pacing violation"
        }
        MD_FARM_OK_USFARM => "Market data farm connection is OK:usfarm",
        HMDS_FARM_OK_USHMDS => "HMDS data farm connection is OK:ushmds",
        SEC_DEF_FARM_OK => "Sec-def data farm connection is OK:secdefil",
        FARM_LOST => "Connectivity between IB and TWS has been lost.",
        FARM_RESTORED_NO_DATA => {
            "Connectivity between IB and Trader Workstation has been restored - data lost."
        }
        FARM_RESTORED_DATA => {
            "Connectivity between IB and Trader Workstation has been restored - data maintained."
        }
        TWS_DAILY_RESTART => "TWS is restarting. Please reconnect.",
        DUPLICATE_ORDER_ID => "Duplicate order id",
        MD_NOT_SUBSCRIBED => {
            "Requested market data is not subscribed. Displaying delayed market data."
        }
        NO_SECURITY_DEF => "No security definition has been found for the request",
        ORDER_REJECTED => "Order rejected - reason",
        ORDER_CANCELLED => "Order cancelled",
        _ => "Unknown error code",
    }
}
