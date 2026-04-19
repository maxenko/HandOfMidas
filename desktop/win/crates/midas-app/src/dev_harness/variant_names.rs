//! Stable string names for [`TickerMsg`] and [`TickerEffect`] variants.
//!
//! Used by the event log to populate the `variant` field that
//! `wait_for_event` matches against. Kept hand-rolled (not derived) so
//! the wire schema is stable even if the enums gain `#[serde(rename)]`
//! attributes later.

use crate::ticker_state::{TickerEffect, TickerMsg};

/// Returns `true` for variants that fire at market-data tick rate and
/// should be excluded from the event log by default. Enable via a
/// future `set_event_log_filter` command if diagnosing something.
pub fn is_tick_rate(msg: &TickerMsg) -> bool {
    matches!(msg, TickerMsg::UpdateMarketData { .. })
}

pub fn ticker_msg_variant(msg: &TickerMsg) -> &'static str {
    match msg {
        TickerMsg::SetBracketMode(_) => "SetBracketMode",
        TickerMsg::EnsureDraftBracket { .. } => "EnsureDraftBracket",
        TickerMsg::CancelBracket => "CancelBracket",
        TickerMsg::SaveBracket => "SaveBracket",
        TickerMsg::DeleteBracket => "DeleteBracket",
        TickerMsg::RecallBracket => "RecallBracket",
        TickerMsg::SetLegPrice { .. } => "SetLegPrice",
        TickerMsg::SetTpEnabled(_) => "SetTpEnabled",
        TickerMsg::SetSlEnabled(_) => "SetSlEnabled",
        TickerMsg::SetQuantity(_) => "SetQuantity",
        TickerMsg::SetSide(_) => "SetSide",
        TickerMsg::SetEntryType(_) => "SetEntryType",
        TickerMsg::DragLeg { .. } => "DragLeg",
        TickerMsg::BeginEdit(_) => "BeginEdit",
        TickerMsg::UpdateEditValue(_) => "UpdateEditValue",
        TickerMsg::CommitEdit { .. } => "CommitEdit",
        TickerMsg::CancelEdit => "CancelEdit",
        TickerMsg::MaybeSnap { .. } => "MaybeSnap",
        TickerMsg::TogglePin => "TogglePin",
        TickerMsg::UndoSnap => "UndoSnap",
        TickerMsg::UpdateMarketData { .. } => "UpdateMarketData",
        TickerMsg::AddLevel(_) => "AddLevel",
        TickerMsg::RemoveLevel(_) => "RemoveLevel",
        TickerMsg::UpdateLevel { .. } => "UpdateLevel",
        TickerMsg::ToggleLevelLock(_) => "ToggleLevelLock",
        TickerMsg::SubmitOrder => "SubmitOrder",
        TickerMsg::OrderPending { .. } => "OrderPending",
        TickerMsg::OrderFilled { .. } => "OrderFilled",
        TickerMsg::OrderPartialFill { .. } => "OrderPartialFill",
        TickerMsg::OrderRejected { .. } => "OrderRejected",
        TickerMsg::OrderCancelled => "OrderCancelled",
        TickerMsg::SaveCameraState { .. } => "SaveCameraState",
        TickerMsg::Hydrated(_) => "Hydrated",
        TickerMsg::MarkSnappedThisSession => "MarkSnappedThisSession",
        TickerMsg::MarkAnchorSeedToastShown => "MarkAnchorSeedToastShown",
        TickerMsg::StoreGatrUndo(_) => "StoreGatrUndo",
        TickerMsg::ClearGatrUndo => "ClearGatrUndo",
    }
}

pub fn ticker_effect_variant(effect: &TickerEffect) -> &'static str {
    match effect {
        TickerEffect::ProjectBracket(_) => "ProjectBracket",
        TickerEffect::RemoveBracket(_) => "RemoveBracket",
        TickerEffect::ProjectLevel { .. } => "ProjectLevel",
        TickerEffect::RemoveLevel { .. } => "RemoveLevel",
        TickerEffect::Toast { .. } => "Toast",
        TickerEffect::PersistDirty => "PersistDirty",
        TickerEffect::SubmitToBroker { .. } => "SubmitToBroker",
    }
}

/// Stable wire-level variant name for a [`midas_broker::BrokerEvent`].
pub fn broker_event_variant(event: &midas_broker::BrokerEvent) -> &'static str {
    use midas_broker::BrokerEvent as E;
    match event {
        E::Connected { .. } => "Connected",
        E::Disconnected { .. } => "Disconnected",
        E::Reconnecting { .. } => "Reconnecting",
        E::Reconnected => "Reconnected",
        E::OrderCreated { .. } => "OrderCreated",
        E::OrderSubmitted { .. } => "OrderSubmitted",
        E::OrderStatusChanged { .. } => "OrderStatusChanged",
        E::OrderFilled { .. } => "OrderFilled",
        E::OrderRejected { .. } => "OrderRejected",
        E::OrderCancelled { .. } => "OrderCancelled",
        E::OrderError { .. } => "OrderError",
        E::OrderValidationFailed { .. } => "OrderValidationFailed",
        E::BracketCreated { .. } => "BracketCreated",
        E::BracketStatusChanged { .. } => "BracketStatusChanged",
        E::Tick { .. } => "Tick",
        E::RealtimeBar { .. } => "RealtimeBar",
        E::BarClosed { .. } => "BarClosed",
        E::BarUpdated { .. } => "BarUpdated",
        E::HistoricalDataComplete { .. } => "HistoricalDataComplete",
        E::DepthUpdate { .. } => "DepthUpdate",
        E::PositionUpdate { .. } => "PositionUpdate",
        E::AccountValueUpdate { .. } => "AccountValueUpdate",
        E::PnlUpdate { .. } => "PnlUpdate",
        E::Warning { .. } => "Warning",
        E::DataFarmStatus { .. } => "DataFarmStatus",
        E::Error { .. } => "Error",
        E::OrderSnapshot { .. } => "OrderSnapshot",
    }
}

/// Tick-rate broker events we skip in the event log to keep the file
/// bounded during IB-attached sessions.
pub fn is_tick_rate_broker(event: &midas_broker::BrokerEvent) -> bool {
    use midas_broker::BrokerEvent as E;
    matches!(
        event,
        E::Tick { .. }
            | E::RealtimeBar { .. }
            | E::BarClosed { .. }
            | E::BarUpdated { .. }
            | E::DepthUpdate { .. }
    )
}

/// Pull a symbol out of a `BrokerEvent` when the variant carries one.
pub fn broker_event_symbol(event: &midas_broker::BrokerEvent) -> Option<String> {
    use midas_broker::BrokerEvent as E;
    match event {
        E::BracketCreated { symbol, .. } => Some(symbol.clone()),
        E::PositionUpdate { symbol, .. } => Some(symbol.clone()),
        E::Tick { symbol, .. }
        | E::RealtimeBar { symbol, .. }
        | E::BarClosed { symbol, .. }
        | E::BarUpdated { symbol, .. }
        | E::HistoricalDataComplete { symbol, .. }
        | E::DepthUpdate { symbol, .. } => Some(symbol.symbol.clone()),
        _ => None,
    }
}
