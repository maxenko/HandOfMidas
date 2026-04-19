//! Shared field-decode helpers used by multiple incoming-message parsers.
//!
//! Every helper consumes from a [`FieldReader`] and advances it; errors
//! surface as [`ProtocolError::Field`]. None of these know about message
//! boundaries — the per-message parser supplies the server_version context.
//!
//! Clippy's `field_reassign_with_default` fires here because each helper
//! starts with `ContractSpec::default()` and then fills sub-blocks gated by
//! server version. That's the clearest expression of the protocol — hoisting
//! the gated reads into a giant struct-literal obscures the version gates.
//! Lint disabled at module scope.
#![allow(clippy::field_reassign_with_default)]

use crate::protocol::messages::fields::FieldReader;
use crate::protocol::messages::server_versions as sv;
use crate::protocol::messages::types::{
    ComboLeg, ContractSpec, DeltaNeutralContract, OrderComboLeg, TagValue,
};
use crate::protocol::{ProtocolError, ServerVersion};

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

/// Parse a full contract payload following `PLACE_ORDER`'s field layout
/// (the widest superset). Used by every message that embeds a contract.
///
/// Mirrors `rust-ibapi`'s v1.2.2 encoder ordering. See
/// `src/orders/encoders.rs::encode_place_order` for the canonical reference.
///
/// Behaviour is keyed on whether the caller wants `ORDER`-shaped fields
/// (includes `primary_exchange`, `trading_class`, `security_id_type`,
/// `security_id`) via the [`ContractShape`] flag.
pub(super) fn parse_contract_for_place_order(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<ContractSpec, ProtocolError> {
    let version = sv_.raw();
    let mut c = ContractSpec::default();

    if version >= sv::PLACE_ORDER_CONID {
        c.contract_id = r.read_i32()?;
    }
    c.symbol = r.read_string()?;
    c.security_type = r.read_string()?;
    c.last_trade_date_or_contract_month = r.read_string()?;
    c.strike = r.read_f64()?;
    c.right = r.read_string()?;
    c.multiplier = r.read_string()?;
    c.exchange = r.read_string()?;
    c.primary_exchange = r.read_string()?;
    c.currency = r.read_string()?;
    c.local_symbol = r.read_string()?;
    if version >= sv::TRADING_CLASS {
        c.trading_class = r.read_string()?;
    }
    if version >= sv::SEC_ID_TYPE {
        c.security_id_type = r.read_string()?;
        c.security_id = r.read_string()?;
    }

    Ok(c)
}

/// Parse the contract block used by `REQ_CONTRACT_DATA` (msg id 9).
pub(super) fn parse_contract_for_req_contract_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<ContractSpec, ProtocolError> {
    let version = sv_.raw();
    let mut c = ContractSpec::default();

    if version >= sv::CONTRACT_CONID {
        c.contract_id = r.read_i32()?;
    }
    c.symbol = r.read_string()?;
    c.security_type = r.read_string()?;
    c.last_trade_date_or_contract_month = r.read_string()?;
    c.strike = r.read_f64()?;
    c.right = r.read_string()?;

    if version >= 15 {
        c.multiplier = r.read_string()?;
    }

    // Exchange block. From server_version PRIMARYEXCH onwards, both exchange
    // and primary_exchange are separate fields. Before that they may be fused
    // into a `"EXCHANGE:PRIMARY"` single field when exchange is SMART/BEST.
    if version >= sv::PRIMARYEXCH {
        c.exchange = r.read_string()?;
        c.primary_exchange = r.read_string()?;
    } else if version >= sv::LINKING {
        let fused = r.read_string()?;
        if let Some((ex, prim)) = fused.split_once(':') {
            c.exchange = ex.to_owned();
            c.primary_exchange = prim.to_owned();
        } else {
            c.exchange = fused;
        }
    }

    c.currency = r.read_string()?;
    c.local_symbol = r.read_string()?;

    if version >= sv::TRADING_CLASS {
        c.trading_class = r.read_string()?;
    }
    if version >= sv::INCLUDE_EXPIRED_IN_REQ_CONTRACT_DATA {
        c.include_expired = r.read_bool()?;
    }
    if version >= sv::SEC_ID_TYPE {
        c.security_id_type = r.read_string()?;
        c.security_id = r.read_string()?;
    }
    if version >= sv::BOND_ISSUERID {
        c.issuer_id = r.read_string()?;
    }

    Ok(c)
}

/// Parse the contract block used by market-data / real-time-bars / historical
/// / mkt-depth. Same shape as PLACE_ORDER's contract minus the `security_id_type`
/// / `security_id` fields that the order encoder adds.
pub(super) fn parse_contract_for_market_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<ContractSpec, ProtocolError> {
    let version = sv_.raw();
    let mut c = ContractSpec::default();

    // Market-data requests always carry contract_id (REQ_MKT_DATA_CONID gated
    // on old versions, but >=47 has it).
    c.contract_id = r.read_i32()?;
    c.symbol = r.read_string()?;
    c.security_type = r.read_string()?;
    c.last_trade_date_or_contract_month = r.read_string()?;
    c.strike = r.read_f64()?;
    c.right = r.read_string()?;
    c.multiplier = r.read_string()?;
    c.exchange = r.read_string()?;
    c.primary_exchange = r.read_string()?;
    c.currency = r.read_string()?;
    c.local_symbol = r.read_string()?;

    if version >= sv::TRADING_CLASS {
        c.trading_class = r.read_string()?;
    }

    Ok(c)
}

/// Parse a contract block for `REQ_HISTORICAL_DATA` (msg id 20). Order-matching
/// shape with `include_expired` instead of a sec_id block.
pub(super) fn parse_contract_for_historical_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<ContractSpec, ProtocolError> {
    let version = sv_.raw();
    let mut c = ContractSpec::default();

    if version >= sv::TRADING_CLASS {
        c.contract_id = r.read_i32()?;
    }
    c.symbol = r.read_string()?;
    c.security_type = r.read_string()?;
    c.last_trade_date_or_contract_month = r.read_string()?;
    c.strike = r.read_f64()?;
    c.right = r.read_string()?;
    c.multiplier = r.read_string()?;
    c.exchange = r.read_string()?;
    c.primary_exchange = r.read_string()?;
    c.currency = r.read_string()?;
    c.local_symbol = r.read_string()?;
    if version >= sv::TRADING_CLASS {
        c.trading_class = r.read_string()?;
    }
    c.include_expired = r.read_bool()?;

    Ok(c)
}

/// Parse a contract block for `REQ_REAL_TIME_BARS` (msg id 50).
pub(super) fn parse_contract_for_realtime_bars(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<ContractSpec, ProtocolError> {
    let version = sv_.raw();
    let mut c = ContractSpec::default();

    if version >= sv::TRADING_CLASS {
        c.contract_id = r.read_i32()?;
    }
    c.symbol = r.read_string()?;
    c.security_type = r.read_string()?;
    c.last_trade_date_or_contract_month = r.read_string()?;
    c.strike = r.read_f64()?;
    c.right = r.read_string()?;
    c.multiplier = r.read_string()?;
    c.exchange = r.read_string()?;
    c.primary_exchange = r.read_string()?;
    c.currency = r.read_string()?;
    c.local_symbol = r.read_string()?;
    if version >= sv::TRADING_CLASS {
        c.trading_class = r.read_string()?;
    }

    Ok(c)
}

// ---------------------------------------------------------------------------
// TagValue lists — two distinct wire conventions.
// ---------------------------------------------------------------------------

/// Read a length-prefixed list of `(tag, value)` pairs:
/// `<count>\0<tag1>\0<val1>\0<tag2>\0<val2>\0...`.
///
/// Used for `algo_params`, `smart_combo_routing_params`, etc.
pub(super) fn parse_tag_value_list(
    r: &mut FieldReader<'_>,
) -> Result<Vec<TagValue>, ProtocolError> {
    let n = r.read_i32()?;
    if n < 0 {
        return Err(ProtocolError::Field(format!(
            "negative tagvalue count: {n}"
        )));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let tag = r.read_string()?;
        let value = r.read_string()?;
        out.push(TagValue::new(tag, value));
    }
    Ok(out)
}

/// Read the single-field concatenated form — `"tag1=val1;tag2=val2;"` —
/// used for `options`, `chart_options`, `order_misc_options`. Empty string
/// yields an empty vec.
pub(super) fn parse_tag_value_string(
    r: &mut FieldReader<'_>,
) -> Result<Vec<TagValue>, ProtocolError> {
    let raw = r.read_string()?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in raw.split_terminator(';') {
        if entry.is_empty() {
            continue;
        }
        let (tag, value) = entry.split_once('=').unwrap_or((entry, ""));
        out.push(TagValue::new(tag.to_owned(), value.to_owned()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Combo legs & delta-neutral contract — PLACE_ORDER sub-blocks.
// ---------------------------------------------------------------------------

pub(super) fn parse_contract_combo_legs(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<Vec<ComboLeg>, ProtocolError> {
    let n = r.read_i32()?;
    if n < 0 {
        return Err(ProtocolError::Field(format!(
            "negative combo-leg count: {n}"
        )));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let mut leg = ComboLeg {
            contract_id: r.read_i32()?,
            ratio: r.read_i32()?,
            action: r.read_string()?,
            exchange: r.read_string()?,
            open_close: r.read_i32()?,
            ..Default::default()
        };
        if sv_.raw() >= sv::SSHORT_COMBO_LEGS {
            leg.short_sale_slot = r.read_i32()?;
            leg.designated_location = r.read_string()?;
        }
        if sv_.raw() >= sv::SSHORTX_OLD {
            leg.exempt_code = r.read_i32()?;
        }
        out.push(leg);
    }
    Ok(out)
}

pub(super) fn parse_order_combo_legs(
    r: &mut FieldReader<'_>,
) -> Result<Vec<OrderComboLeg>, ProtocolError> {
    let n = r.read_i32()?;
    if n < 0 {
        return Err(ProtocolError::Field(format!(
            "negative order-combo-leg count: {n}"
        )));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(OrderComboLeg {
            price: r.read_opt_f64()?,
        });
    }
    Ok(out)
}

/// Delta-neutral attached contract — a tri-field block prefixed by a bool.
pub(super) fn parse_delta_neutral_contract(
    r: &mut FieldReader<'_>,
) -> Result<Option<DeltaNeutralContract>, ProtocolError> {
    let present = r.read_bool()?;
    if !present {
        return Ok(None);
    }
    Ok(Some(DeltaNeutralContract {
        contract_id: r.read_i32()?,
        delta: r.read_f64()?,
        price: r.read_f64()?,
    }))
}

// ---------------------------------------------------------------------------
// ExecutionFilter — fixed 7-field block used by REQ_EXECUTIONS.
// ---------------------------------------------------------------------------

pub(super) fn parse_execution_filter(
    r: &mut FieldReader<'_>,
) -> Result<crate::protocol::messages::types::ExecutionFilter, ProtocolError> {
    use crate::protocol::messages::types::ExecutionFilter;
    Ok(ExecutionFilter {
        client_id: r.read_i32()?,
        acct_code: r.read_string()?,
        time: r.read_string()?,
        symbol: r.read_string()?,
        sec_type: r.read_string()?,
        exchange: r.read_string()?,
        side: r.read_string()?,
    })
}
