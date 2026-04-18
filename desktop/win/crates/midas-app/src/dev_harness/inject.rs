//! Parse a JSON payload into a [`TickerMsg`] for `inject_ticker_msg`.
//!
//! Wire format is **internally-tagged**: the JSON object carries a
//! `"type"` field naming the variant, plus variant-specific peer
//! fields. For example:
//!
//! ```json
//! {"type": "SetLegPrice", "role": "Entry", "price": 184.5}
//! {"type": "SetBracketMode", "side": "Buy"}
//! {"type": "CancelBracket"}
//! ```
//!
//! This format is hand-parsed rather than auto-derived because
//! [`TickerMsg`]'s transitive type graph includes some types that
//! do not yet implement `Deserialize` (notably `StoredLevel` and
//! `TickerState`'s hydrate path). Variants that depend on those are
//! rejected with [`InjectError::NotSupported`] — callers should use
//! click/drag simulation or fixtures for those cases.

use midas_chart::widget::order_bracket::{EntryType, LegRole};
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

use crate::order_panel::OrderSide;
use crate::ticker_state::{EditingField, TickerMsg};

#[derive(Debug, Error)]
pub enum InjectError {
    #[error("unknown TickerMsg variant: {0}")]
    UnknownVariant(String),
    #[error("variant not supported for injection: {0} (use click/drag or load_fixture)")]
    NotSupported(&'static str),
    #[error("payload shape mismatch: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("missing `type` field on inject_ticker_msg payload")]
    MissingType,
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

pub fn parse(value: &serde_json::Value) -> Result<TickerMsg, InjectError> {
    let obj = value.as_object().ok_or(InjectError::MissingType)?;
    let ty = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(InjectError::MissingType)?;

    match ty {
        // ── Simple unit variants ─────────────────────────────────
        "CancelBracket" => Ok(TickerMsg::CancelBracket),
        "SaveBracket" => Ok(TickerMsg::SaveBracket),
        "DeleteBracket" => Ok(TickerMsg::DeleteBracket),
        "RecallBracket" => Ok(TickerMsg::RecallBracket),
        "CancelEdit" => Ok(TickerMsg::CancelEdit),
        "TogglePin" => Ok(TickerMsg::TogglePin),
        "UndoSnap" => Ok(TickerMsg::UndoSnap),
        "SubmitOrder" => Ok(TickerMsg::SubmitOrder),
        "OrderCancelled" => Ok(TickerMsg::OrderCancelled),

        // ── Simple field variants ────────────────────────────────
        "SetBracketMode" => {
            // `side` absent or null disables brackets.
            let side: Option<OrderSide> = match obj.get("side") {
                Some(v) if !v.is_null() => Some(serde_json::from_value(v.clone())?),
                _ => None,
            };
            Ok(TickerMsg::SetBracketMode(side))
        }

        "EnsureDraftBracket" => {
            #[derive(Deserialize)]
            struct P {
                side: OrderSide,
                entry_type: EntryType,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::EnsureDraftBracket {
                side: p.side,
                entry_type: p.entry_type,
            })
        }

        "SetLegPrice" => {
            #[derive(Deserialize)]
            struct P {
                role: LegRole,
                price: f64,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::SetLegPrice {
                role: p.role,
                price: p.price,
            })
        }

        "SetTpEnabled" => Ok(TickerMsg::SetTpEnabled(required_bool(obj, "enabled")?)),
        "SetSlEnabled" => Ok(TickerMsg::SetSlEnabled(required_bool(obj, "enabled")?)),
        "SetQuantity" => Ok(TickerMsg::SetQuantity(required_f64(obj, "quantity")?)),
        "SetSide" => Ok(TickerMsg::SetSide(required_side(obj, "side")?)),
        "SetEntryType" => {
            let entry_type: EntryType = obj
                .get("entry_type")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .ok_or(InjectError::MissingField("entry_type"))?;
            Ok(TickerMsg::SetEntryType(entry_type))
        }

        "DragLeg" => {
            #[derive(Deserialize)]
            struct P {
                role: LegRole,
                new_price: f64,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::DragLeg {
                role: p.role,
                new_price: p.new_price,
            })
        }

        "BeginEdit" => {
            let field: EditingField = obj
                .get("field")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .ok_or(InjectError::MissingField("field"))?;
            Ok(TickerMsg::BeginEdit(field))
        }

        "UpdateEditValue" => Ok(TickerMsg::UpdateEditValue(required_string(obj, "value")?)),

        "CommitEdit" => {
            #[derive(Deserialize)]
            struct P {
                field: EditingField,
                value: String,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::CommitEdit {
                field: p.field,
                value: p.value,
            })
        }

        "MaybeSnap" => {
            #[derive(Deserialize)]
            struct P {
                current_price: f64,
                gatr_abs: Option<f64>,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::MaybeSnap {
                current_price: p.current_price,
                gatr_abs: p.gatr_abs,
            })
        }

        "UpdateMarketData" => {
            #[derive(Deserialize)]
            struct P {
                last_price: f64,
                gatr_abs: Option<f64>,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::UpdateMarketData {
                last_price: p.last_price,
                gatr_abs: p.gatr_abs,
            })
        }

        "OrderPending" => {
            let id_str = required_string(obj, "order_id")?;
            let order_id = Uuid::parse_str(&id_str)
                .map_err(|e| serde_json::Error::custom(format!("uuid: {e}")))?;
            Ok(TickerMsg::OrderPending { order_id })
        }

        "OrderFilled" => {
            #[derive(Deserialize)]
            struct P {
                filled_qty: f64,
                avg_price: f64,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::OrderFilled {
                filled_qty: p.filled_qty,
                avg_price: p.avg_price,
            })
        }

        "OrderPartialFill" => Ok(TickerMsg::OrderPartialFill {
            filled_qty: required_f64(obj, "filled_qty")?,
        }),

        "OrderRejected" => Ok(TickerMsg::OrderRejected {
            reason: required_string(obj, "reason")?,
        }),

        "SaveCameraState" => {
            #[derive(Deserialize)]
            struct P {
                time_start: f64,
                time_end: f64,
                price_low: f64,
                price_high: f64,
                was_at_live_edge: bool,
            }
            let p: P = serde_json::from_value(value.clone())?;
            Ok(TickerMsg::SaveCameraState {
                time_start: p.time_start,
                time_end: p.time_end,
                price_low: p.price_low,
                price_high: p.price_high,
                was_at_live_edge: p.was_at_live_edge,
            })
        }

        // ── Variants that depend on types without Deserialize ────
        "AddLevel" | "UpdateLevel" => Err(InjectError::NotSupported(
            "AddLevel / UpdateLevel — StoredLevel lacks Deserialize",
        )),
        "Hydrated" => Err(InjectError::NotSupported(
            "Hydrated — use load_fixture to replace state",
        )),
        "RemoveLevel" => Ok(TickerMsg::RemoveLevel(required_usize(obj, "index")?)),
        "ToggleLevelLock" => Ok(TickerMsg::ToggleLevelLock(required_usize(obj, "index")?)),

        other => Err(InjectError::UnknownVariant(other.to_owned())),
    }
}

// ── Scalar helpers ───────────────────────────────────────────────────

fn required_bool(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<bool, InjectError> {
    obj.get(field)
        .and_then(|v| v.as_bool())
        .ok_or(InjectError::MissingField(field))
}

fn required_f64(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<f64, InjectError> {
    obj.get(field)
        .and_then(|v| v.as_f64())
        .ok_or(InjectError::MissingField(field))
}

fn required_usize(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<usize, InjectError> {
    obj.get(field)
        .and_then(|v| v.as_u64())
        .map(|u| u as usize)
        .ok_or(InjectError::MissingField(field))
}

fn required_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<String, InjectError> {
    obj.get(field)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or(InjectError::MissingField(field))
}

fn required_side(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<OrderSide, InjectError> {
    obj.get(field)
        .cloned()
        .map(serde_json::from_value::<OrderSide>)
        .transpose()?
        .ok_or(InjectError::MissingField(field))
}

// Implement a pragma so `serde_json::Error::custom` compiles (trait
// import needed — keeps call sites clean above).
use serde::de::Error as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cancel_bracket_unit_variant() {
        let v = serde_json::json!({"type": "CancelBracket"});
        let msg = parse(&v).unwrap();
        assert!(matches!(msg, TickerMsg::CancelBracket));
    }

    #[test]
    fn parse_set_leg_price() {
        let v = serde_json::json!({
            "type": "SetLegPrice",
            "role": "Entry",
            "price": 184.5,
        });
        let msg = parse(&v).unwrap();
        match msg {
            TickerMsg::SetLegPrice { price, .. } => assert!((price - 184.5).abs() < 1e-9),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_set_bracket_mode_some() {
        let v = serde_json::json!({"type": "SetBracketMode", "side": "Buy"});
        let msg = parse(&v).unwrap();
        match msg {
            TickerMsg::SetBracketMode(Some(OrderSide::Buy)) => {}
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_set_bracket_mode_null_clears() {
        let v = serde_json::json!({"type": "SetBracketMode", "side": null});
        let msg = parse(&v).unwrap();
        assert!(matches!(msg, TickerMsg::SetBracketMode(None)));
    }

    #[test]
    fn parse_set_bracket_mode_missing_side_clears() {
        let v = serde_json::json!({"type": "SetBracketMode"});
        let msg = parse(&v).unwrap();
        assert!(matches!(msg, TickerMsg::SetBracketMode(None)));
    }

    #[test]
    fn unknown_variant_rejected() {
        let v = serde_json::json!({"type": "NotAVariant"});
        match parse(&v) {
            Err(InjectError::UnknownVariant(s)) => assert_eq!(s, "NotAVariant"),
            _ => panic!("expected UnknownVariant"),
        }
    }

    #[test]
    fn level_variants_rejected_with_clear_message() {
        let v = serde_json::json!({"type": "AddLevel"});
        assert!(matches!(parse(&v), Err(InjectError::NotSupported(_))));
    }

    #[test]
    fn missing_type_field_rejected() {
        let v = serde_json::json!({"role": "Entry"});
        assert!(matches!(parse(&v), Err(InjectError::MissingType)));
    }

    #[test]
    fn order_pending_parses_uuid() {
        let v = serde_json::json!({
            "type": "OrderPending",
            "order_id": "550e8400-e29b-41d4-a716-446655440000",
        });
        let msg = parse(&v).unwrap();
        assert!(matches!(msg, TickerMsg::OrderPending { .. }));
    }
}
