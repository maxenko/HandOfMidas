//! Factory methods for [`TickerState`].
//!
//! Three constructors:
//!
//! - [`TickerState::new`]: bare-minimum defaults for a fresh symbol.
//! - [`TickerState::new_with_defaults`]: populates all 8 compound keys
//!   with sensible bracket defaults derived from the current price.
//! - [`TickerState::from_legacy`]: migration path from the old
//!   `TickerOrderIntent` + `LevelStore` + `AnnotationStore` data.

use std::collections::HashMap;

use chrono::Utc;
use midas_chart::widget::order_bracket::{EntryType, OrderBracket};
use midas_chart::widget::AnnotationId;

use crate::annotation_store::SymbolKey;
use crate::level_store::StoredLevel;
use crate::order_panel::OrderSide;
use crate::ticker_order_intent::price_defaults::default_initial_prices;
use crate::ticker_order_intent::{EntryMemory, GatrAnchor, TickerOrderIntent};

use super::{TickerState, CURRENT_VERSION};

/// All `(OrderSide, EntryType)` compound keys.
const ALL_COMPOUND_KEYS: [(OrderSide, EntryType); 8] = [
    (OrderSide::Buy, EntryType::Market),
    (OrderSide::Buy, EntryType::Limit),
    (OrderSide::Buy, EntryType::Stop),
    (OrderSide::Buy, EntryType::StopLimit),
    (OrderSide::Sell, EntryType::Market),
    (OrderSide::Sell, EntryType::Limit),
    (OrderSide::Sell, EntryType::Stop),
    (OrderSide::Sell, EntryType::StopLimit),
];

impl TickerState {
    /// Construct a fresh, empty ticker state for a symbol.
    ///
    /// All fields are at their defaults; no entry memory buckets are
    /// populated. `sl_enabled = true` on every `EntryMemory::default()`.
    pub fn new(symbol: SymbolKey) -> Self {
        Self {
            symbol,
            version: CURRENT_VERSION,
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Market,
            entries: HashMap::new(),
            gatr_anchor: GatrAnchor::default(),
            pinned: false,
            live_bracket: None,
            live_annotation_id: None,
            levels: Vec::new(),
            last_price: None,
            gatr_abs: None,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            updated_at: Utc::now(),
            generation: 0,
        }
    }

    /// Construct a ticker state with sensible bracket defaults for all
    /// 8 compound keys.
    ///
    /// Uses [`default_initial_prices`] to populate each
    /// `(OrderSide, EntryType)` bucket with prices derived from
    /// `current_price` and `gatr_abs`.
    pub fn new_with_defaults(
        symbol: SymbolKey,
        current_price: f64,
        gatr_abs: Option<f64>,
    ) -> Self {
        let mut entries = HashMap::new();

        for (side, entry_type) in ALL_COMPOUND_KEYS {
            let prices = default_initial_prices(side, entry_type, current_price, gatr_abs);
            let memory = EntryMemory {
                entry_price_or_offset: Some(prices.entry),
                quantity: Some(100.0),
                tp_enabled: false,
                tp_value: format!("{:.2}", prices.take_profit),
                sl_enabled: true,
                sl_value: format!("{:.2}", prices.stop_loss),
                ..EntryMemory::default()
            };
            entries.insert((side, entry_type), memory);
        }

        Self {
            symbol,
            version: CURRENT_VERSION,
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Market,
            entries,
            gatr_anchor: GatrAnchor::default(),
            pinned: false,
            live_bracket: None,
            live_annotation_id: None,
            levels: Vec::new(),
            last_price: Some(current_price),
            gatr_abs,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            updated_at: Utc::now(),
            generation: 0,
        }
    }

    /// Construct a ticker state from legacy data sources.
    ///
    /// Copies all fields from a `TickerOrderIntent`, then overlays
    /// levels, bracket, and annotation ID from `AnnotationStore` /
    /// `LevelStore`. Last-write-wins: redb data (the intent) takes
    /// priority for fields that exist in both sources.
    pub fn from_legacy(
        intent: TickerOrderIntent,
        levels: Vec<StoredLevel>,
        bracket: Option<OrderBracket>,
        annotation_id: Option<AnnotationId>,
    ) -> Self {
        Self {
            symbol: intent.symbol,
            version: CURRENT_VERSION,
            last_side: intent.last_side,
            last_entry_type: intent.last_entry_type,
            entries: intent.entries,
            gatr_anchor: intent.gatr_anchor,
            pinned: intent.pinned,
            live_bracket: bracket,
            live_annotation_id: annotation_id.or(intent.live_annotation_id),
            levels,
            last_price: None,
            gatr_abs: None,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            updated_at: intent.updated_at,
            generation: 0,
        }
    }
}
