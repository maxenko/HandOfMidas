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

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use midas_chart::widget::order_bracket::{EntryType, OrderBracket};
use midas_chart::widget::AnnotationId;
use serde::{Deserialize, Serialize};

use crate::annotation_store::SymbolKey;
use crate::level_store::StoredLevel;
use crate::order_panel::OrderSide;
use crate::ticker_order_intent::{EntryMemory, GatrAnchor, TickerOrderIntent};

pub use apply::{TickerEffect, TickerMsg};

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
    #[serde(default, with = "crate::ticker_order_intent::entries_serde")]
    entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    /// GATR snap anchor for this symbol.
    #[serde(default)]
    gatr_anchor: GatrAnchor,
    /// When `true`, the GATR snap rule skips this symbol.
    #[serde(default)]
    pinned: bool,

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

/// Convert a v1 [`TickerOrderIntent`] into a v2 [`TickerState`].
///
/// Copies all fields from the intent. Fields that did not exist in v1
/// (`live_bracket`, `levels`, `last_price`, `gatr_abs`, editing state,
/// pre-snap) default to `None`/empty.
pub fn migrate_v1_v2(intent: &TickerOrderIntent) -> TickerState {
    TickerState {
        symbol: intent.symbol.clone(),
        version: CURRENT_VERSION,
        last_side: intent.last_side,
        last_entry_type: intent.last_entry_type,
        entries: intent.entries.clone(),
        gatr_anchor: intent.gatr_anchor,
        pinned: intent.pinned,
        live_bracket: None,
        live_annotation_id: intent.live_annotation_id,
        levels: Vec::new(),
        last_price: None,
        gatr_abs: None,
        editing_field: None,
        editing_value: None,
        pre_snap: None,
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
