#![allow(dead_code)] // Public API surface for future slices; many items only used by tests.

//! Per-symbol ticker state machine.
//!
//! `TickerState` is the single source of truth for all per-symbol state:
//! order brackets, entry type memories, GATR anchors, price levels, and
//! market data snapshots. Every UI surface (charts, order panels,
//! watchlists) renders from a `TickerState` via read-only getters. All
//! mutations go through [`TickerState::apply`], which returns a
//! `Vec<TickerEffect>` that the caller interprets.
//!
//! # INVARIANT: all state mutations go through `apply()`.
//!
//! No public setter exists for any field. The only public mutation
//! method is `apply(msg: TickerMsg) -> Vec<TickerEffect>`. This is
//! module-boundary enforcement: code outside `ticker_state` cannot
//! mutate fields directly.
//!
mod apply;
mod factory;
pub mod persist;
pub mod price_defaults;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use midas_chart::widget::order_bracket::{EntryType, OrderBracket};
use midas_chart::widget::AnnotationId;
use serde::{Deserialize, Serialize};

use crate::annotation_store::SymbolKey;
use crate::level_store::StoredLevel;
use crate::order_panel::{OrderSide, PriceInputMode, StopLossType};

// ── Types formerly in ticker_order_intent ──────────────────────────

/// Panel memory for a single `(OrderSide, EntryType)` bucket.
///
/// All of these fields mirror the string-shaped inputs on
/// [`crate::order_panel::OrderPanelState`]; they are stored as
/// `Option<f64>` / `String` rather than parsed values because the
/// user may be mid-typing when a snapshot is taken, and the panel
/// wants to round-trip the exact textual input.
///
/// # Stop-Loss-on-by-default rule
///
/// [`EntryMemory::default`] sets `sl_enabled = true`. Each compound
/// key tracks its own opt-out independently: toggling SL off in
/// `(Buy, Stop)` does **not** turn it off in `(Buy, Limit)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryMemory {
    /// Most recent entry price (Limit / StopLimit / Stop) or offset
    /// (future offset mode). `None` until the user has touched this
    /// compound key at least once.
    #[serde(default)]
    pub entry_price_or_offset: Option<f64>,
    /// Most recent quantity. `None` until touched.
    #[serde(default)]
    pub quantity: Option<f64>,
    /// Whether TP was enabled the last time the user visited this
    /// compound key.
    #[serde(default)]
    pub tp_enabled: bool,
    /// Textual TP value as entered (preserves user's formatting).
    #[serde(default)]
    pub tp_value: String,
    /// TP price input mode (Absolute / Offset / Percent).
    #[serde(default = "default_price_input_mode")]
    pub tp_mode: PriceInputMode,
    /// Whether SL was enabled the last time the user visited this
    /// compound key. **Defaults to `true`.**
    #[serde(default = "default_true")]
    pub sl_enabled: bool,
    /// Textual SL value as entered.
    #[serde(default)]
    pub sl_value: String,
    /// SL price input mode.
    #[serde(default = "default_price_input_mode")]
    pub sl_mode: PriceInputMode,
    /// SL order type (Stop / StopLimit).
    #[serde(default = "default_stop_loss_type")]
    pub sl_type: StopLossType,
    /// Textual SL limit price (only meaningful when
    /// `sl_type == StopLossType::StopLimit`).
    #[serde(default)]
    pub sl_limit_value: String,
}

impl Default for EntryMemory {
    fn default() -> Self {
        Self {
            entry_price_or_offset: None,
            quantity: None,
            tp_enabled: false,
            tp_value: String::new(),
            tp_mode: PriceInputMode::Absolute,
            sl_enabled: true, // SL-on-by-default per D2.
            sl_value: String::new(),
            sl_mode: PriceInputMode::Absolute,
            sl_type: StopLossType::Stop,
            sl_limit_value: String::new(),
        }
    }
}

/// GATR snap anchor: the last-known price + GATR for a symbol.
///
/// Both fields are `Option` because the very first time an intent is
/// bootstrapped, there is nothing to anchor against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GatrAnchor {
    /// The price at the last user-initiated upsert. `None` means
    /// "never touched" — the snap rule cannot fire.
    #[serde(default)]
    pub anchor_price: Option<f64>,
    /// The absolute GATR at the last user-initiated upsert.
    #[serde(default)]
    pub anchor_gatr: Option<f64>,
}

fn default_price_input_mode() -> PriceInputMode {
    PriceInputMode::Absolute
}

fn default_stop_loss_type() -> StopLossType {
    StopLossType::Stop
}

fn default_true() -> bool {
    true
}

/// Serde helper for `entries`. Represents the map as an array of
/// `[side, entry_type, memory]` triples so that JSON — which requires
/// string keys on objects — can round-trip it losslessly.
pub(crate) mod entries_serde {
    use super::{EntryMemory, EntryType, HashMap, OrderSide};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Triple {
        side: OrderSide,
        entry_type: EntryType,
        memory: EntryMemory,
    }

    pub(crate) fn serialize<S: Serializer>(
        map: &HashMap<(OrderSide, EntryType), EntryMemory>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let vec: Vec<Triple> = map
            .iter()
            .map(|((side, entry_type), memory)| Triple {
                side: *side,
                entry_type: *entry_type,
                memory: memory.clone(),
            })
            .collect();
        vec.serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<HashMap<(OrderSide, EntryType), EntryMemory>, D::Error> {
        let vec = Vec::<Triple>::deserialize(d)?;
        Ok(vec
            .into_iter()
            .map(|t| ((t.side, t.entry_type), t.memory))
            .collect())
    }
}

pub use apply::{TickerEffect, TickerMsg};

/// Snapshot of a saved per-ticker camera position.
///
/// Returned by [`TickerState::saved_camera`] when all four f64 fields
/// are present. The caller uses this to restore the viewport in
/// `bind_chart_to_symbol`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedCamera {
    /// Saved time range start (epoch ms).
    pub time_start: f64,
    /// Saved time range end (epoch ms).
    pub time_end: f64,
    /// Saved price range bottom.
    pub price_low: f64,
    /// Saved price range top.
    pub price_high: f64,
    /// Whether the user was at the live edge when saved.
    pub was_at_live_edge: bool,
}

/// Current on-disk schema version for [`TickerState`].
///
/// Version 2 supersedes `TickerOrderIntent` v1. The migration path is
/// [`migrate_v1_v2`].
pub const CURRENT_VERSION: u32 = 2;

/// Per-symbol ticker state: the complete, authoritative record for one
/// symbol's order brackets, entry memories, levels, and market data.
///
/// All fields are private. Public access is through getters; the only
/// mutation path is [`TickerState::apply`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickerState {
    /// The symbol this state belongs to.
    symbol: SymbolKey,
    /// Schema version. Always [`CURRENT_VERSION`] for newly created
    /// instances.
    #[serde(default = "default_version")]
    version: u32,

    // ── Order entry memory (from TickerOrderIntent) ──────────────
    /// Which side the user most recently used for this symbol.
    #[serde(default = "default_side")]
    last_side: OrderSide,
    /// Which entry type the user most recently used for this symbol.
    #[serde(default)]
    last_entry_type: EntryType,
    /// Per-compound-key panel memory. Eight possible buckets, one per
    /// `(OrderSide, EntryType)` combination.
    #[serde(default, with = "entries_serde")]
    entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    /// GATR snap anchor for this symbol.
    #[serde(default)]
    gatr_anchor: GatrAnchor,
    /// When `true`, the GATR snap rule skips this symbol.
    #[serde(default)]
    pinned: bool,

    // ── Bracket mode (BUY / X / SELL toggle) ─────────────────────
    /// Whether brackets are active for this symbol.
    /// `None` = X (no brackets), `Some(Buy)` = BUY active,
    /// `Some(Sell)` = SELL active.  Default is `None` (X).
    #[serde(default)]
    bracket_mode: Option<OrderSide>,

    // ── Live bracket (projected to AnnotationStore via effects) ──
    /// The owned live bracket for this symbol. Projected to
    /// `AnnotationStore` via `TickerEffect::ProjectBracket`.
    #[serde(default)]
    live_bracket: Option<OrderBracket>,
    /// Annotation ID of the live bracket in `AnnotationStore`.
    #[serde(default)]
    live_annotation_id: Option<AnnotationId>,

    // ── Levels (from LevelStore) ─────────────────────────────────
    /// Price levels for this symbol. Not serialized because
    /// `StoredLevel` does not implement `Serialize`/`Deserialize`;
    /// levels are migrated from TOML config in Slice 4.
    #[serde(skip)]
    levels: Vec<StoredLevel>,

    // ── Market data (ephemeral, not persisted) ───────────────────
    /// Last known price from the market data feed.
    #[serde(skip)]
    last_price: Option<f64>,
    /// Current absolute GATR value for this symbol.
    #[serde(skip)]
    gatr_abs: Option<f64>,

    // ── Editing focus lock ───────────────────────────────────────
    /// Which field the user is currently editing, if any.
    #[serde(skip)]
    editing_field: Option<EditingField>,
    /// In-progress text for the locked editing field.
    #[serde(skip)]
    editing_value: Option<String>,

    // ── Undo ─────────────────────────────────────────────────────
    /// Pre-snap state for GATR undo. Contains the bracket and state
    /// snapshot before the last snap, plus the instant it was taken.
    #[serde(skip)]
    pre_snap: Option<(Box<PreSnapState>, std::time::Instant)>,

    // ── Camera (per-ticker viewport restore) ────────────────────
    /// Saved camera time range start (epoch ms). `None` for fresh
    /// tickers that have never been viewed.
    #[serde(default)]
    camera_time_start: Option<f64>,
    /// Saved camera time range end (epoch ms).
    #[serde(default)]
    camera_time_end: Option<f64>,
    /// Saved camera price range bottom.
    #[serde(default)]
    camera_price_low: Option<f64>,
    /// Saved camera price range top.
    #[serde(default)]
    camera_price_high: Option<f64>,
    /// Whether the user was at the live edge (most recent candle visible)
    /// when the camera was last saved. Defaults to `true` so a fresh
    /// ticker shows the latest data on first restore.
    #[serde(default = "default_true")]
    camera_was_at_live_edge: bool,

    // ── Metadata ─────────────────────────────────────────────────
    /// When this state was last written.
    #[serde(default = "default_updated_at")]
    updated_at: DateTime<Utc>,
    /// Monotonic generation counter. Bumped on every `apply()` call
    /// that actually mutates state.
    #[serde(default)]
    generation: u64,
}

// ── Getters ─────────────────────────────────────────────────────────

impl TickerState {
    /// The symbol this state belongs to.
    pub fn symbol(&self) -> &SymbolKey {
        &self.symbol
    }

    /// The user's most recently used side for this symbol.
    pub fn last_side(&self) -> OrderSide {
        self.last_side
    }

    /// The user's most recently used entry type for this symbol.
    pub fn last_entry_type(&self) -> EntryType {
        self.last_entry_type
    }

    /// The entry memory for the current `(last_side, last_entry_type)`
    /// compound key. Returns a default `EntryMemory` if the bucket has
    /// never been touched.
    pub fn active_entry_memory(&self) -> &EntryMemory {
        static DEFAULT: std::sync::LazyLock<EntryMemory> =
            std::sync::LazyLock::new(EntryMemory::default);
        self.entries
            .get(&(self.last_side, self.last_entry_type))
            .unwrap_or(&DEFAULT)
    }

    /// Whether brackets are active for this symbol.
    /// `None` = X (inactive), `Some(Buy)` = BUY, `Some(Sell)` = SELL.
    pub fn bracket_mode(&self) -> Option<OrderSide> {
        self.bracket_mode
    }

    /// The live bracket for this symbol, if any.
    pub fn live_bracket(&self) -> Option<&OrderBracket> {
        self.live_bracket.as_ref()
    }

    /// The annotation ID of the live bracket, if any.
    pub fn live_annotation_id(&self) -> Option<AnnotationId> {
        self.live_annotation_id
    }

    /// Price levels for this symbol.
    pub fn levels(&self) -> &[StoredLevel] {
        &self.levels
    }

    /// Last known market price.
    pub fn last_price(&self) -> Option<f64> {
        self.last_price
    }

    /// Current absolute GATR value.
    pub fn gatr_abs(&self) -> Option<f64> {
        self.gatr_abs
    }

    /// Whether the GATR snap rule is pinned (skipped) for this symbol.
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    /// Whether the user is currently editing a field.
    pub fn is_editing(&self) -> bool {
        self.editing_field.is_some()
    }

    /// The in-progress text for the locked editing field, if any.
    pub fn editing_value(&self) -> Option<&str> {
        self.editing_value.as_deref()
    }

    /// Monotonic generation counter.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The GATR anchor for this symbol.
    pub fn gatr_anchor(&self) -> &GatrAnchor {
        &self.gatr_anchor
    }

    /// The full entries map (read-only).
    pub fn entries(&self) -> &HashMap<(OrderSide, EntryType), EntryMemory> {
        &self.entries
    }

    /// The current editing field, if any.
    pub fn editing_field(&self) -> Option<&EditingField> {
        self.editing_field.as_ref()
    }

    /// Whether the state has been updated since creation.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Schema version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Return the saved camera state, if all four f64 fields are present.
    ///
    /// Returns `None` for a fresh ticker that has never been viewed
    /// (any of the four fields is `None`). The `was_at_live_edge` flag
    /// is always available (defaults to `true`).
    pub fn saved_camera(&self) -> Option<SavedCamera> {
        match (
            self.camera_time_start,
            self.camera_time_end,
            self.camera_price_low,
            self.camera_price_high,
        ) {
            (Some(time_start), Some(time_end), Some(price_low), Some(price_high)) => {
                Some(SavedCamera {
                    time_start,
                    time_end,
                    price_low,
                    price_high,
                    was_at_live_edge: self.camera_was_at_live_edge,
                })
            }
            _ => None,
        }
    }

    // ── Effect-handler setters ──────────────────────────────────

    /// Set the live annotation ID. Called by the effect handler after
    /// `annotation_store.add()` returns the newly assigned ID.
    pub fn set_live_annotation_id(&mut self, id: Option<AnnotationId>) {
        self.live_annotation_id = id;
    }

    /// Set the live bracket. Called by the effect handler when
    /// recalling a bracket from `annotation_store` or injecting an
    /// externally constructed bracket.
    pub fn set_live_bracket(&mut self, bracket: Option<OrderBracket>) {
        self.live_bracket = bracket;
    }

    /// Set the cached market price. Called by the effect handler so
    /// `apply()` can reference the latest price for bracket defaults.
    pub fn set_last_price(&mut self, price: Option<f64>) {
        self.last_price = price;
    }

    /// Set the cached GATR absolute value.
    pub fn set_gatr_abs(&mut self, gatr: Option<f64>) {
        self.gatr_abs = gatr;
    }

    /// Inject levels during v1→v2 migration. Called once at startup
    /// when importing from TOML config.
    pub fn inject_levels(&mut self, levels: Vec<StoredLevel>) {
        self.levels = levels;
    }

    // ── Test-only setters ──────────────────────────────────────────

    /// Override `bracket_mode`. Test-only.
    #[cfg(test)]
    pub(crate) fn force_bracket_mode(&mut self, mode: Option<OrderSide>) {
        self.bracket_mode = mode;
    }

    /// Override the GATR anchor. Test-only.
    #[cfg(test)]
    pub(crate) fn force_gatr_anchor(&mut self, anchor: GatrAnchor) {
        self.gatr_anchor = anchor;
    }

    /// Override `updated_at`. Test-only — used to bypass the recency
    /// guard in GATR snap tests.
    #[cfg(test)]
    pub(crate) fn force_updated_at(&mut self, dt: DateTime<Utc>) {
        self.updated_at = dt;
    }

    /// Override the pre-snap instant. Test-only — used to simulate
    /// expired TTL in undo snap tests.
    #[cfg(test)]
    pub(crate) fn force_pre_snap_instant(&mut self, instant: std::time::Instant) {
        if let Some((_, ref mut ts)) = self.pre_snap {
            *ts = instant;
        }
    }
}

// ── EditingField ────────────────────────────────────────────────────

/// Which text field in the order panel the user is currently editing.
///
/// Used by the `BeginEdit`/`CommitEdit`/`CancelEdit` focus-lock flow
/// to suppress conflicting mutations (e.g. GATR snap) while the user
/// is typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EditingField {
    /// The limit price input.
    LimitPrice,
    /// The stop price input.
    StopPrice,
    /// The take-profit value input.
    TpValue,
    /// The stop-loss value input.
    SlValue,
    /// The stop-loss limit price input (StopLimit SL only).
    SlLimitValue,
    /// The quantity input.
    Quantity,
}

// ── PreSnapState ────────────────────────────────────────────────────

/// Holds the pre-snap bracket and state snapshot for GATR undo.
///
/// When the GATR snap rule fires, the current bracket and relevant
/// state fields are cloned into this struct. If the user clicks "Undo"
/// within the TTL window, the snapshot is restored.
#[derive(Debug, Clone)]
pub struct PreSnapState {
    /// The bracket before the snap was applied, if one existed.
    pub bracket: Option<Box<OrderBracket>>,
    /// Snapshot of the entries map before the snap.
    pub entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    /// Snapshot of the GATR anchor before the snap.
    pub gatr_anchor: GatrAnchor,
}

// ── v1 → v2 migration ──────────────────────────────────────────────

/// v1 on-disk shape (formerly `TickerOrderIntent`). Used only for
/// deserialization during the v1→v2 migration path and tests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TickerOrderIntentV1 {
    #[serde(default = "default_v1_version")]
    pub version: u32,
    #[serde(default = "default_symbol")]
    pub symbol: SymbolKey,
    #[serde(default = "default_side")]
    pub last_side: OrderSide,
    #[serde(default)]
    pub last_entry_type: EntryType,
    #[serde(default, with = "entries_serde")]
    pub entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    #[serde(default)]
    pub gatr_anchor: GatrAnchor,
    #[serde(default)]
    pub live_annotation_id: Option<AnnotationId>,
    #[serde(default)]
    pub broker_order_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_updated_at")]
    pub updated_at: DateTime<Utc>,
}

impl TickerOrderIntentV1 {
    /// Construct a fresh, empty v1 intent for a symbol.
    #[cfg(test)]
    pub(crate) fn new(symbol: SymbolKey) -> Self {
        Self {
            version: 1,
            symbol,
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Market,
            entries: HashMap::new(),
            gatr_anchor: GatrAnchor::default(),
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: Utc::now(),
        }
    }
}

fn default_v1_version() -> u32 {
    1
}

fn default_symbol() -> SymbolKey {
    SymbolKey::new("")
}

/// Convert a v1 [`TickerOrderIntentV1`] into a v2 [`TickerState`].
///
/// Copies all fields from the intent. Fields that did not exist in v1
/// (`live_bracket`, `levels`, `last_price`, `gatr_abs`, editing state,
/// pre-snap) default to `None`/empty.
pub fn migrate_v1_v2(intent: &TickerOrderIntentV1) -> TickerState {
    TickerState {
        symbol: intent.symbol.clone(),
        version: CURRENT_VERSION,
        last_side: intent.last_side,
        last_entry_type: intent.last_entry_type,
        entries: intent.entries.clone(),
        gatr_anchor: intent.gatr_anchor,
        pinned: intent.pinned,
        bracket_mode: None,
        live_bracket: None,
        live_annotation_id: intent.live_annotation_id,
        levels: Vec::new(),
        last_price: None,
        gatr_abs: None,
        editing_field: None,
        editing_value: None,
        pre_snap: None,
        camera_time_start: None,
        camera_time_end: None,
        camera_price_low: None,
        camera_price_high: None,
        camera_was_at_live_edge: true,
        updated_at: intent.updated_at,
        generation: 0,
    }
}

// ── Serde helpers ───────────────────────────────────────────────────

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_side() -> OrderSide {
    OrderSide::Buy
}

fn default_updated_at() -> DateTime<Utc> {
    Utc::now()
}
