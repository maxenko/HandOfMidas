//! Shared field writers used by multiple outgoing encoders (and, in Stage 02b,
//! by the incoming parsers).
//!
//! Encoding helpers are kept here so that the `OPEN_ORDER` / `EXECUTION_DATA`
//! / `POSITION` / `CONTRACT_DATA` encoders can share the contract field layout
//! without copy-paste drift. Stage 02b's parsers will add symmetric `read_*`
//! helpers in this module.

use midas_broker_core::{ContractSpec, OptionRight};

use crate::engine::types::{OrderKind, Side};
use crate::protocol::messages::fields::FieldWriter;

// ---------------------------------------------------------------------------
// ContractSpec projections
// ---------------------------------------------------------------------------

/// Return the 9 fields the `OPEN_ORDER` encoder emits for a contract's
/// logical projection: `(symbol, sec_type, expiry, strike, right, exchange,
/// currency, local_symbol, trading_class)`. Multiplier is handled separately.
pub(crate) fn contract_open_order_fields(
    c: &ContractSpec,
) -> (&str, &str, &str, f64, &str, &str, &str, &str, &str) {
    match c {
        ContractSpec::Stock {
            symbol,
            exchange,
            currency,
        } => (symbol, "STK", "", 0.0, "", exchange, currency, symbol, ""),
        ContractSpec::Option {
            symbol,
            expiry,
            strike,
            right,
            exchange,
        } => (
            symbol,
            "OPT",
            expiry,
            strike.0,
            match right {
                OptionRight::Call => "C",
                OptionRight::Put => "P",
            },
            exchange,
            "USD",
            symbol,
            "",
        ),
        ContractSpec::Future {
            symbol,
            expiry,
            exchange,
        } => (symbol, "FUT", expiry, 0.0, "", exchange, "USD", symbol, ""),
        ContractSpec::Forex { pair } => (pair, "CASH", "", 0.0, "", "IDEALPRO", "USD", pair, ""),
    }
}

/// Emit the 12-field contract prefix used by `EXECUTION_DATA`: the 11 fields
/// of the contract projection + trading_class. Shares field-by-field order
/// with `rust-ibapi::orders::common::decoders::decode_execution_data`.
pub(crate) fn write_contract_execution_prefix(w: &mut FieldWriter, c: &ContractSpec) {
    match c {
        ContractSpec::Stock {
            symbol,
            exchange,
            currency,
        } => {
            w.write_i32(0); // contract_id — sim-side ids aren't assigned yet
            w.write_string(symbol);
            w.write_string("STK");
            w.write_string(""); // last_trade_date_or_contract_month
            w.write_f64(0.0); // strike
            w.write_string(""); // right
            w.write_string(""); // multiplier
            w.write_string(exchange);
            w.write_string(currency);
            w.write_string(symbol); // local_symbol
            w.write_string(""); // trading_class
        }
        ContractSpec::Option {
            symbol,
            expiry,
            strike,
            right,
            exchange,
        } => {
            w.write_i32(0);
            w.write_string(symbol);
            w.write_string("OPT");
            w.write_string(expiry);
            w.write_f64(strike.0);
            w.write_string(match right {
                OptionRight::Call => "C",
                OptionRight::Put => "P",
            });
            w.write_string(""); // multiplier
            w.write_string(exchange);
            w.write_string("USD");
            w.write_string(symbol);
            w.write_string("");
        }
        ContractSpec::Future {
            symbol,
            expiry,
            exchange,
        } => {
            w.write_i32(0);
            w.write_string(symbol);
            w.write_string("FUT");
            w.write_string(expiry);
            w.write_f64(0.0);
            w.write_string("");
            w.write_string("");
            w.write_string(exchange);
            w.write_string("USD");
            w.write_string(symbol);
            w.write_string("");
        }
        ContractSpec::Forex { pair } => {
            w.write_i32(0);
            w.write_string(pair);
            w.write_string("CASH");
            w.write_string("");
            w.write_f64(0.0);
            w.write_string("");
            w.write_string("");
            w.write_string("IDEALPRO");
            w.write_string("USD");
            w.write_string(pair);
            w.write_string("");
        }
    }
}

// ---------------------------------------------------------------------------
// Enum -> wire-string projections
// ---------------------------------------------------------------------------

/// Side -> IB action string used by every order-related outgoing message.
pub(crate) fn side_action(side: Side) -> &'static str {
    match side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    }
}

/// OrderKind -> IB order-type string.
pub(crate) fn write_order_kind(k: OrderKind) -> &'static str {
    match k {
        OrderKind::Market => "MKT",
        OrderKind::Limit => "LMT",
        OrderKind::Stop => "STP",
        OrderKind::StopLimit => "STP LMT",
    }
}

/// TIF string — pass-through helper with a safe default for unknown values.
pub(crate) fn write_tif(tif: &str) -> &str {
    if tif.is_empty() {
        "DAY"
    } else {
        tif
    }
}
