//! Scenario verb → engine command translation.
//!
//! Stage 06 fills in verb dispatch. Scenario-local verbs (`Sleep`, `Include`,
//! `Assert*`) are handled by the runner directly and return `None` from
//! [`verb_to_cmd`] because they produce no `EngineCmd`.
//!
//! `AcceptOrder` is handled by the runner too, because the scenario YAML
//! carries an `order_ref` + optional bracket children that the flat
//! `EngineCmd::PlaceOrder` signature can't express. The runner consults the
//! helpers in this module to build each leg's command.
//!
//! Every other verb is a 1:1 mapping.

use std::time::Duration;

use midas_broker_core::{ContractSpec, SymbolKey};

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{
    EngineCmd, OrderId, OrderKind, PlaceOrderReq, ReqId, SessionId, SubMode, TickByTickKind,
};

use super::schema::{
    AcceptOrderArgs, EntryType, OrderSide, SessionSelector, SubscriptionKind, Verb,
};

/// Translate a parsed verb into an engine command. Scenario-local verbs
/// (`Sleep`, `SetClockMode`, `Include`, `Assert*`, `AcceptOrder`,
/// `CancelOrder`) return `None` — the runner handles them out-of-band.
pub fn verb_to_cmd(verb: &Verb) -> Option<EngineCmd> {
    Some(match verb {
        Verb::SubscribeMarketData(args) => EngineCmd::SubscribeMarketData {
            session: SessionId(0),
            req_id: ReqId(synth_req_id(&args.symbol)),
            contract: stock_contract(&args.symbol),
            mode: subscription_to_mode(args.subscription),
        },
        Verb::UnsubscribeMarketData(args) => EngineCmd::UnsubscribeMarketData {
            session: SessionId(0),
            req_id: ReqId(synth_req_id(&args.symbol)),
        },
        // Handled by the runner — returned `None` so accidental dispatch
        // through this function doesn't double-send.
        Verb::AcceptOrder(_) | Verb::CancelOrder(_) => return None,
        Verb::InjectDisconnect(args) => EngineCmd::InjectDisconnect {
            session: session_to_engine(args.session_id.as_ref()),
            reason: args.reason.clone().unwrap_or_default(),
        },
        Verb::InjectFarmOutage(args) => EngineCmd::InjectFarmOutage {
            code: args.code,
            farms: args.farms.clone(),
        },
        Verb::InjectFarmRestore(args) => EngineCmd::InjectFarmRestore {
            code: args.code,
            farms: args.farms.clone(),
        },
        Verb::InjectPacingViolation(args) => EngineCmd::InjectPacingViolation {
            session: session_to_engine(Some(&args.session_id)),
        },
        Verb::InjectLag(args) => EngineCmd::InjectLag {
            session: session_to_engine(Some(&args.session_id)),
            duration: parse_dur(&args.duration).unwrap_or(Duration::ZERO),
        },
        Verb::InjectBadFrame(_args) => {
            // `EngineCmd` has no BadFrame variant today — recorded via the
            // mock engine's projection. Return None so verb_to_cmd remains
            // total (no panic); the runner's dispatch logs a warn.
            return None;
        }
        Verb::InjectPriceJump(args) => EngineCmd::InjectPriceJump {
            symbol: symbol_key(&args.symbol),
            magnitude_pct: args.magnitude_pct,
        },
        Verb::InjectGap(args) => EngineCmd::InjectGap {
            symbol: symbol_key(&args.symbol),
            from: args.from,
            to: args.to,
        },
        Verb::InjectHalt(args) => EngineCmd::InjectHalt {
            symbol: symbol_key(&args.symbol),
            duration: parse_dur(&args.duration).unwrap_or(Duration::ZERO),
        },
        Verb::InjectBurst(args) => EngineCmd::InjectBurst {
            symbols: args.symbols.iter().map(|s| symbol_key(s)).collect(),
            multiplier: args.multiplier,
            duration: parse_dur(&args.duration).unwrap_or(Duration::ZERO),
        },
        Verb::InjectDailyRestart => EngineCmd::InjectDailyRestart,
        // These verbs don't have a dedicated `EngineCmd` variant — the mock
        // engine's MockCmd projection captures them for recording.
        Verb::InjectDuplicateOrderStatus(_)
        | Verb::InjectSlowCommissionReport(_)
        | Verb::InjectOutOfOrderEvents(_) => return None,

        Verb::Sleep(_)
        | Verb::SetClockMode(_)
        | Verb::Include(_)
        | Verb::Assert(_)
        | Verb::AssertClientReceived(_)
        | Verb::AssertClientEventOrder(_) => return None,
    })
}

/// Translate a bracket parent's [`AcceptOrderArgs`] into a `PlaceOrder` command.
pub fn accept_order_to_cmd(args: &AcceptOrderArgs, order_id: OrderId) -> EngineCmd {
    let (kind, limit, aux) = resolve_entry(args);
    EngineCmd::PlaceOrder {
        session: SessionId(0),
        req: PlaceOrderReq {
            order_id,
            contract: stock_contract(args.symbol.as_deref().unwrap_or("UNKNOWN")),
            side: super::runner::side_to_engine(args.side),
            total_quantity: args.quantity,
            kind,
            limit_price: limit,
            aux_price: aux,
            tif: "DAY".into(),
            account: args.account.clone().unwrap_or_else(|| "DU1".into()),
            parent_id: None,
            oca_group: None,
            transmit: true,
        },
    }
}

/// Child legs of a bracket, derived from the parent's entry + TP/SL offsets.
pub(crate) struct BracketLeg {
    pub child_ref: String,
    pub side: OrderSide,
    pub kind: OrderKind,
    pub limit: Option<f64>,
    pub aux: Option<f64>,
    pub quantity: f64,
    pub symbol: String,
    pub account: String,
}

pub(crate) fn bracket_children(
    parent: &AcceptOrderArgs,
    _parent_id: OrderId,
    parent_ref: &str,
) -> Vec<BracketLeg> {
    let opposite = match parent.side {
        OrderSide::Buy => OrderSide::Sell,
        OrderSide::Sell => OrderSide::Buy,
    };
    let sign = match parent.side {
        OrderSide::Buy => 1.0,
        OrderSide::Sell => -1.0,
    };
    let base = parent.limit_price.or(parent.stop_price).unwrap_or(0.0);
    let mut legs = Vec::new();
    if let Some(tp_offset) = parent.tp_offset {
        legs.push(BracketLeg {
            child_ref: format!("{parent_ref}-tp"),
            side: opposite,
            kind: OrderKind::Limit,
            limit: Some(base + sign * tp_offset),
            aux: None,
            quantity: parent.quantity,
            symbol: parent.symbol.clone().unwrap_or_default(),
            account: parent.account.clone().unwrap_or_else(|| "DU1".into()),
        });
    }
    if let Some(sl_offset) = parent.sl_offset {
        legs.push(BracketLeg {
            child_ref: format!("{parent_ref}-sl"),
            side: opposite,
            kind: OrderKind::Stop,
            limit: None,
            aux: Some(base - sign * sl_offset.abs()),
            quantity: parent.quantity,
            symbol: parent.symbol.clone().unwrap_or_default(),
            account: parent.account.clone().unwrap_or_else(|| "DU1".into()),
        });
    }
    legs
}

pub(crate) fn bracket_leg_to_cmd(
    leg: &BracketLeg,
    child_id: OrderId,
    parent_id: OrderId,
) -> EngineCmd {
    EngineCmd::PlaceOrder {
        session: SessionId(0),
        req: PlaceOrderReq {
            order_id: child_id,
            contract: stock_contract(&leg.symbol),
            side: super::runner::side_to_engine(leg.side),
            total_quantity: leg.quantity,
            kind: leg.kind,
            limit_price: leg.limit,
            aux_price: leg.aux,
            tif: "GTC".into(),
            account: leg.account.clone(),
            parent_id: Some(parent_id),
            oca_group: Some(format!("bracket-{}", parent_id.0)),
            transmit: true,
        },
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn resolve_entry(args: &AcceptOrderArgs) -> (OrderKind, Option<f64>, Option<f64>) {
    match args.entry.unwrap_or(EntryType::Market) {
        EntryType::Market => (OrderKind::Market, None, None),
        EntryType::Limit => (OrderKind::Limit, args.limit_price, None),
        EntryType::Stop => (OrderKind::Stop, None, args.stop_price),
        EntryType::StopLimit => (OrderKind::StopLimit, args.limit_price, args.stop_price),
    }
}

fn session_to_engine(sel: Option<&SessionSelector>) -> SessionId {
    match sel {
        Some(SessionSelector::Id(i)) => SessionId(*i),
        // `"all"` maps to session 0 for the mock — real engine branches on
        // the name in Wave 2.
        Some(SessionSelector::Named(_)) | None => SessionId(0),
    }
}

fn symbol_key(sym: &str) -> SymbolKey {
    SymbolKey {
        contract_id: synth_contract_id(sym),
        symbol: sym.to_string(),
    }
}

fn stock_contract(sym: &str) -> ContractSpec {
    ContractSpec::Stock {
        symbol: sym.to_string(),
        exchange: "SMART".into(),
        currency: "USD".into(),
    }
}

fn synth_req_id(sym: &str) -> i32 {
    // Deterministic id per symbol — same symbol always maps to same req_id.
    let mut hash = 5381i32;
    for b in sym.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as i32);
    }
    hash.unsigned_abs() as i32
}

fn synth_contract_id(sym: &str) -> i32 {
    synth_req_id(sym) ^ 0x5f5f5f5f
}

fn subscription_to_mode(kind: SubscriptionKind) -> SubMode {
    match kind {
        SubscriptionKind::StreamingL1 => SubMode::StreamingL1 {
            snapshot: false,
            regulatory_snapshot: false,
        },
        SubscriptionKind::TickByTickLast => SubMode::TickByTick {
            kind: TickByTickKind::Last,
        },
        SubscriptionKind::TickByTickAllLast => SubMode::TickByTick {
            kind: TickByTickKind::AllLast,
        },
        SubscriptionKind::TickByTickBidAsk => SubMode::TickByTick {
            kind: TickByTickKind::BidAsk,
        },
        SubscriptionKind::TickByTickMidPoint => SubMode::TickByTick {
            kind: TickByTickKind::MidPoint,
        },
        SubscriptionKind::RealtimeBars5s => SubMode::RealtimeBars5s,
        SubscriptionKind::Historical => SubMode::Historical(crate::engine::types::HistoricalReq {
            contract: stock_contract(""),
            end_date_time: String::new(),
            duration: "1 D".into(),
            bar_size: "1 min".into(),
            what_to_show: "TRADES".into(),
            use_rth: true,
            format_date: 1,
            keep_up_to_date: false,
        }),
    }
}

fn parse_dur(s: &str) -> Option<Duration> {
    super::expr::interpreter::parse_duration(s).ok()
}

// `VirtualInstant` is re-exported here so callers can build fixture payloads
// via the injector without pulling it in transitively.
#[allow(dead_code)]
pub(crate) const _ENSURE_VINST_LINK: Option<VirtualInstant> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::schema::{
        FarmCodeArgs, InjectDisconnectArgs, InjectPriceJumpArgs, SubscribeMarketDataArgs,
    };

    #[test]
    fn subscribe_maps_to_engine_cmd() {
        let v = Verb::SubscribeMarketData(SubscribeMarketDataArgs {
            symbol: "AAPL".into(),
            subscription: SubscriptionKind::StreamingL1,
        });
        let cmd = verb_to_cmd(&v).expect("should map");
        assert!(matches!(cmd, EngineCmd::SubscribeMarketData { .. }));
    }

    #[test]
    fn disconnect_respects_session_selector() {
        let v = Verb::InjectDisconnect(InjectDisconnectArgs {
            session_id: Some(SessionSelector::Id(7)),
            reason: Some("test".into()),
        });
        match verb_to_cmd(&v).unwrap() {
            EngineCmd::InjectDisconnect { session, reason } => {
                assert_eq!(session.0, 7);
                assert_eq!(reason, "test");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn price_jump_carries_magnitude() {
        let v = Verb::InjectPriceJump(InjectPriceJumpArgs {
            symbol: "AAPL".into(),
            magnitude_pct: -3.5,
        });
        match verb_to_cmd(&v).unwrap() {
            EngineCmd::InjectPriceJump {
                symbol,
                magnitude_pct,
            } => {
                assert_eq!(symbol.symbol, "AAPL");
                assert!((magnitude_pct + 3.5).abs() < 1e-9);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn farm_outage_maps_code_and_farms() {
        let v = Verb::InjectFarmOutage(FarmCodeArgs {
            code: 1100,
            farms: vec!["usfarm".into()],
        });
        match verb_to_cmd(&v).unwrap() {
            EngineCmd::InjectFarmOutage { code, farms } => {
                assert_eq!(code, 1100);
                assert_eq!(farms, vec!["usfarm".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scenario_local_verbs_return_none() {
        use crate::scenario::schema::{AssertArgs, Expression, IncludeArgs, SleepArgs};
        assert!(verb_to_cmd(&Verb::Sleep(SleepArgs {
            duration: "1s".into(),
        }))
        .is_none());
        assert!(verb_to_cmd(&Verb::Include(IncludeArgs {
            path: "other.yaml".into(),
        }))
        .is_none());
        assert!(verb_to_cmd(&Verb::Assert(AssertArgs {
            cond: Expression::from("true"),
            message: None,
        }))
        .is_none());
    }
}
