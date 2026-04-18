//! Hand-rolled parser for `InjectBrokerEvent` payloads.
//!
//! Wire format is internally-tagged JSON, matching
//! [`super::inject`]'s convention:
//!
//! ```json
//! {"type": "BracketCreated", "parent_id": "...", "symbol": "AAPL", ...}
//! {"type": "OrderStatusChanged", "order_id": "...", "new_status": "Filled", ...}
//! ```
//!
//! Only the broker-event variants useful for testing the desktop
//! pipeline are supported. Market-data / depth variants are rejected —
//! those aren't exercised by the blotter or the annotation wiring.

use midas_broker::{BrokerEvent, OrderAction, OrderKind, TimeInForce};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum InjectBrokerError {
    #[error("unknown BrokerEvent variant: {0}")]
    UnknownVariant(String),
    #[error("variant not supported for injection: {0}")]
    NotSupported(&'static str),
    #[error("payload shape mismatch: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("missing `type` field on inject_broker_event payload")]
    MissingType,
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid field {0}: {1}")]
    InvalidField(&'static str, String),
}

pub fn parse(value: &serde_json::Value) -> Result<BrokerEvent, InjectBrokerError> {
    let obj = value.as_object().ok_or(InjectBrokerError::MissingType)?;
    let ty = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(InjectBrokerError::MissingType)?;

    match ty {
        "Connected" => Ok(BrokerEvent::Connected {
            server_version: required_i64(obj, "server_version").unwrap_or(0) as i32,
        }),
        "Disconnected" => Ok(BrokerEvent::Disconnected {
            reason: required_string(obj, "reason")?,
        }),
        "Reconnected" => Ok(BrokerEvent::Reconnected),

        "BracketCreated" => Ok(BrokerEvent::BracketCreated {
            parent_id: required_uuid(obj, "parent_id")?,
            take_profit_id: optional_uuid(obj, "take_profit_id")?,
            stop_loss_id: optional_uuid(obj, "stop_loss_id")?,
            symbol: required_string(obj, "symbol")?,
            action: parse_action(obj, "action")?,
            quantity: required_f64(obj, "quantity")?,
            tp_price: optional_f64(obj, "tp_price"),
            sl_price: optional_f64(obj, "sl_price"),
            reference_price: optional_f64(obj, "reference_price"),
            entry_kind: parse_kind(obj, "entry_kind")?,
            entry_limit_price: optional_f64(obj, "entry_limit_price"),
            entry_stop_price: optional_f64(obj, "entry_stop_price"),
            sl_limit_price: optional_f64(obj, "sl_limit_price"),
            tp_tif: parse_optional_tif(obj, "tp_tif")?,
            sl_tif: parse_optional_tif(obj, "sl_tif")?,
        }),

        "OrderSubmitted" => Ok(BrokerEvent::OrderSubmitted {
            order_id: required_uuid(obj, "order_id")?,
            ib_order_id: required_i64(obj, "ib_order_id")? as i32,
            ib_perm_id: required_i64(obj, "ib_perm_id")?,
        }),

        "OrderStatusChanged" => Ok(BrokerEvent::OrderStatusChanged {
            order_id: required_uuid(obj, "order_id")?,
            old_status: required_string(obj, "old_status")?,
            new_status: required_string(obj, "new_status")?,
            filled_qty: required_f64(obj, "filled_qty")?,
            remaining_qty: required_f64(obj, "remaining_qty")?,
            avg_fill_price: required_f64(obj, "avg_fill_price")?,
        }),

        "OrderFilled" => Ok(BrokerEvent::OrderFilled {
            order_id: required_uuid(obj, "order_id")?,
            ib_exec_id: required_string(obj, "ib_exec_id")?,
            shares: required_f64(obj, "shares")?,
            price: required_f64(obj, "price")?,
            commission: optional_f64(obj, "commission"),
        }),

        "OrderCancelled" => Ok(BrokerEvent::OrderCancelled {
            order_id: required_uuid(obj, "order_id")?,
            reason: required_string(obj, "reason")?,
        }),

        "OrderRejected" => Ok(BrokerEvent::OrderRejected {
            order_id: required_uuid(obj, "order_id")?,
            reason: required_string(obj, "reason")?,
        }),

        "PositionUpdate" => Ok(BrokerEvent::PositionUpdate {
            account: required_string(obj, "account")?,
            symbol: required_string(obj, "symbol")?,
            con_id: required_i64(obj, "con_id")? as i32,
            quantity: required_f64(obj, "quantity")?,
            avg_cost: required_f64(obj, "avg_cost")?,
        }),

        "BracketStatusChanged" | "OrderError" | "OrderValidationFailed" | "OrderCreated" => {
            Err(InjectBrokerError::NotSupported(
                "variant carries complex types; extend broker_inject.rs if needed",
            ))
        }

        "Tick"
        | "RealtimeBar"
        | "BarClosed"
        | "BarUpdated"
        | "HistoricalDataComplete"
        | "DepthUpdate" => Err(InjectBrokerError::NotSupported(
            "market-data events do not drive the blotter or annotation wiring",
        )),

        other => Err(InjectBrokerError::UnknownVariant(other.to_owned())),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

type Obj = serde_json::Map<String, serde_json::Value>;

fn required_string(obj: &Obj, field: &'static str) -> Result<String, InjectBrokerError> {
    obj.get(field)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or(InjectBrokerError::MissingField(field))
}

fn required_f64(obj: &Obj, field: &'static str) -> Result<f64, InjectBrokerError> {
    obj.get(field)
        .and_then(|v| v.as_f64())
        .ok_or(InjectBrokerError::MissingField(field))
}

fn optional_f64(obj: &Obj, field: &str) -> Option<f64> {
    obj.get(field).and_then(|v| v.as_f64())
}

fn required_i64(obj: &Obj, field: &'static str) -> Result<i64, InjectBrokerError> {
    obj.get(field)
        .and_then(|v| v.as_i64())
        .ok_or(InjectBrokerError::MissingField(field))
}

fn required_uuid(obj: &Obj, field: &'static str) -> Result<Uuid, InjectBrokerError> {
    let s = required_string(obj, field)?;
    Uuid::parse_str(&s).map_err(|e| InjectBrokerError::InvalidField(field, e.to_string()))
}

fn optional_uuid(obj: &Obj, field: &str) -> Result<Option<Uuid>, InjectBrokerError> {
    match obj.get(field) {
        Some(v) if !v.is_null() => {
            let s = v.as_str().ok_or_else(|| {
                InjectBrokerError::InvalidField(
                    Box::leak(field.to_owned().into_boxed_str()),
                    "expected string".to_owned(),
                )
            })?;
            Ok(Some(Uuid::parse_str(s).map_err(|e| {
                InjectBrokerError::InvalidField(
                    Box::leak(field.to_owned().into_boxed_str()),
                    e.to_string(),
                )
            })?))
        }
        _ => Ok(None),
    }
}

fn parse_action(obj: &Obj, field: &'static str) -> Result<OrderAction, InjectBrokerError> {
    let s = required_string(obj, field)?;
    match s.as_str() {
        "Buy" | "BUY" | "buy" => Ok(OrderAction::Buy),
        "Sell" | "SELL" | "sell" => Ok(OrderAction::Sell),
        other => Err(InjectBrokerError::InvalidField(field, other.to_owned())),
    }
}

fn parse_kind(obj: &Obj, field: &'static str) -> Result<OrderKind, InjectBrokerError> {
    let s = required_string(obj, field)?;
    match s.as_str() {
        "Market" | "MKT" => Ok(OrderKind::Market),
        "Limit" | "LMT" => Ok(OrderKind::Limit),
        "Stop" | "STP" => Ok(OrderKind::Stop),
        "StopLimit" | "STP_LMT" => Ok(OrderKind::StopLimit),
        "TrailingStop" | "TRAIL" => Ok(OrderKind::TrailingStop),
        other => Err(InjectBrokerError::InvalidField(field, other.to_owned())),
    }
}

fn parse_optional_tif(obj: &Obj, field: &str) -> Result<Option<TimeInForce>, InjectBrokerError> {
    let Some(v) = obj.get(field) else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let s = v.as_str().unwrap_or_default();
    Ok(Some(match s {
        "Day" | "DAY" => TimeInForce::Day,
        "Gtc" | "GTC" => TimeInForce::Gtc,
        "Ioc" | "IOC" => TimeInForce::Ioc,
        "Gtd" | "GTD" => TimeInForce::Gtd,
        "Opg" | "OPG" => TimeInForce::Opg,
        other => {
            return Err(InjectBrokerError::InvalidField(
                Box::leak(field.to_owned().into_boxed_str()),
                other.to_owned(),
            ));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bracket_created() {
        let v = serde_json::json!({
            "type": "BracketCreated",
            "parent_id": "550e8400-e29b-41d4-a716-446655440000",
            "take_profit_id": "550e8400-e29b-41d4-a716-446655440001",
            "stop_loss_id": null,
            "symbol": "AAPL",
            "action": "Buy",
            "quantity": 100.0,
            "tp_price": 195.0,
            "sl_price": null,
            "reference_price": 184.5,
            "entry_kind": "Market",
            "entry_limit_price": null,
            "entry_stop_price": null,
            "sl_limit_price": null,
            "tp_tif": "Day",
            "sl_tif": null,
        });
        let ev = parse(&v).unwrap();
        match ev {
            BrokerEvent::BracketCreated {
                symbol,
                quantity,
                take_profit_id,
                stop_loss_id,
                ..
            } => {
                assert_eq!(symbol, "AAPL");
                assert_eq!(quantity, 100.0);
                assert!(take_profit_id.is_some());
                assert!(stop_loss_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_order_filled() {
        let v = serde_json::json!({
            "type": "OrderFilled",
            "order_id": "550e8400-e29b-41d4-a716-446655440000",
            "ib_exec_id": "exec-1",
            "shares": 100.0,
            "price": 184.53,
            "commission": 0.25,
        });
        match parse(&v).unwrap() {
            BrokerEvent::OrderFilled { shares, price, .. } => {
                assert_eq!(shares, 100.0);
                assert!((price - 184.53).abs() < 1e-9);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn unknown_variant_rejected() {
        let v = serde_json::json!({"type": "Totally Made Up"});
        assert!(matches!(
            parse(&v),
            Err(InjectBrokerError::UnknownVariant(_))
        ));
    }

    #[test]
    fn market_data_rejected() {
        let v = serde_json::json!({"type": "Tick"});
        assert!(matches!(parse(&v), Err(InjectBrokerError::NotSupported(_))));
    }

    #[test]
    fn parses_position_update() {
        let v = serde_json::json!({
            "type": "PositionUpdate",
            "account": "DU123456",
            "symbol": "GME",
            "con_id": 208813720,
            "quantity": 100.0,
            "avg_cost": 18.5,
        });
        match parse(&v).unwrap() {
            BrokerEvent::PositionUpdate {
                account,
                symbol,
                con_id,
                quantity,
                avg_cost,
            } => {
                assert_eq!(account, "DU123456");
                assert_eq!(symbol, "GME");
                assert_eq!(con_id, 208813720);
                assert_eq!(quantity, 100.0);
                assert!((avg_cost - 18.5).abs() < 1e-9);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn position_update_supports_short_quantity() {
        let v = serde_json::json!({
            "type": "PositionUpdate",
            "account": "DU123456",
            "symbol": "AAPL",
            "con_id": 265598,
            "quantity": -50.0,
            "avg_cost": 175.25,
        });
        match parse(&v).unwrap() {
            BrokerEvent::PositionUpdate { quantity, .. } => assert_eq!(quantity, -50.0),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn position_update_rejects_missing_symbol() {
        let v = serde_json::json!({
            "type": "PositionUpdate",
            "account": "DU123456",
            "con_id": 1,
            "quantity": 100.0,
            "avg_cost": 10.0,
        });
        assert!(matches!(
            parse(&v),
            Err(InjectBrokerError::MissingField("symbol"))
        ));
    }

    #[test]
    fn position_update_rejects_missing_con_id() {
        let v = serde_json::json!({
            "type": "PositionUpdate",
            "account": "DU123456",
            "symbol": "AAPL",
            "quantity": 100.0,
            "avg_cost": 10.0,
        });
        assert!(matches!(
            parse(&v),
            Err(InjectBrokerError::MissingField("con_id"))
        ));
    }

    #[test]
    fn position_update_rejects_missing_quantity() {
        let v = serde_json::json!({
            "type": "PositionUpdate",
            "account": "DU123456",
            "symbol": "AAPL",
            "con_id": 1,
            "avg_cost": 10.0,
        });
        assert!(matches!(
            parse(&v),
            Err(InjectBrokerError::MissingField("quantity"))
        ));
    }
}
