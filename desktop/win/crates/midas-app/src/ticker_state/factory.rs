//! Factory methods for [`TickerState`].
//!
//! Three constructors:
//!
//! - [`TickerState::new`]: bare-minimum defaults for a fresh symbol.
//! - [`TickerState::new_with_defaults`]: populates all 8 compound keys
//!   with sensible bracket defaults derived from the current price.
//! - [`TickerState::from_legacy`]: migration path from the old
//!   `TickerOrderIntent` + `AnnotationStore` data (levels used to live
//!   in a separate `LevelStore`; retired in audit P2b).

use std::collections::HashMap;

use chrono::Utc;
use midas_annotation_types::order_bracket::{EntryType, OrderBracket};
use midas_annotation_types::AnnotationId;

use crate::annotation_store::{StoredLevel, SymbolKey};
use crate::order_panel::OrderSide;

use super::price_defaults::default_initial_prices;
use super::{EntryMemory, GatrAnchor, TickerOrderIntentV1, TickerState, CURRENT_VERSION};

/// All `(OrderSide, EntryType)` compound keys.
#[allow(dead_code)] // used by new_with_defaults which is used by tests
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
            bracket_mode: None,
            live_bracket: None,
            live_annotation_id: None,
            levels: Vec::new(),
            last_price: None,
            gatr_abs: None,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            session: super::TickerSessionFlags::default(),
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            camera_was_at_live_edge: true,
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
    #[allow(dead_code)] // used by tests and future slices
    pub fn new_with_defaults(symbol: SymbolKey, current_price: f64, gatr_abs: Option<f64>) -> Self {
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
            bracket_mode: None,
            live_bracket: None,
            live_annotation_id: None,
            levels: Vec::new(),
            last_price: Some(current_price),
            gatr_abs,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            session: super::TickerSessionFlags::default(),
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            camera_was_at_live_edge: true,
            updated_at: Utc::now(),
            generation: 0,
        }
    }

    /// Construct a ticker state from legacy data sources.
    ///
    /// Copies all fields from a v1 `TickerOrderIntentV1`, then overlays
    /// levels, bracket, and annotation ID from `AnnotationStore`.
    /// Last-write-wins: redb data (the intent) takes priority for
    /// fields that exist in both sources.
    #[allow(dead_code)] // used by tests and v1 migration
    pub fn from_legacy(
        intent: TickerOrderIntentV1,
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
            bracket_mode: None,
            live_bracket: bracket,
            live_annotation_id: annotation_id.or(intent.live_annotation_id),
            levels,
            last_price: None,
            gatr_abs: None,
            editing_field: None,
            editing_value: None,
            pre_snap: None,
            session: super::TickerSessionFlags::default(),
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            camera_was_at_live_edge: true,
            updated_at: intent.updated_at,
            generation: 0,
        }
    }
}
