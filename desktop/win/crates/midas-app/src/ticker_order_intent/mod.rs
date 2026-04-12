//! Per-ticker order intent store.
//!
//! This module provides the foundation for remembering, per symbol,
//! the user's last-set order-bracket inputs (side, entry type, prices,
//! quantity, TP/SL settings, GATR anchor, pin state, etc.), so that
//! switching charts or restarting the app does not drop that context.
//!
//! # Design
//!
//! - **Data model** lives here (`mod.rs`): `TickerOrderIntent`,
//!   `EntryMemory`, `GatrAnchor`. All are `serde`-friendly; the file on
//!   disk is human-readable JSON (one row per symbol).
//! - **In-memory cache** lives in [`store`]: a `parking_lot::RwLock`
//!   around a `HashMap<SymbolKey, Arc<TickerOrderIntent>>`. All reads
//!   are sync and lock-free after the clone of the `Arc`.
//! - **Actor** lives in [`actor`]: built on
//!   [`mailbox_processor::MailboxProcessor::new_blocking`] because
//!   `redb` is a sync API. A dedicated background thread debounces
//!   writes at 75 ms so a 60 Hz drag does not fsync 60 times per second.
//! - **Handle** lives in [`handle`]: the public facade used by the app.
//!   Snapshots are sync (iced's `update()` is sync); writes are
//!   effectively sync too — the cache mutation is synchronous, only
//!   the persistence fan-out is deferred.
//!
//! # Stale-cache-between-write-and-refresh rule
//!
//! [`handle::TickerOrderIntentHandle::upsert`] mutates the in-memory
//! cache **synchronously before returning**. The very next
//! [`handle::TickerOrderIntentHandle::snapshot`] call sees the new
//! value. The background flush only affects *persistence*, not
//! visibility. This guarantee is what lets the reducer in Slice 3
//! "write intent, then refresh panel view" as a straight-line match arm.
//!
//! # Slice 1a scope
//!
//! This slice ships the core store, the actor, coalesced flush with
//! shutdown drain, inline validation, and the
//! [`reducer::OrderIntentAppMsg`] enum + a stub `apply_order_intent_msg`
//! that returns `Task::none()` for every variant. Slices 2, 3, 4 wire
//! the handle into `MidasApp`, fill in the reducer arms, and add the
//! GATR / pin decorator affordances. Failure-mode hardening
//! (multi-instance lock, corruption recovery, disk-full) is Slice 1b.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use midas_chart::widget::order_bracket::EntryType;
use midas_chart::widget::AnnotationId;
use serde::{Deserialize, Serialize};

use crate::annotation_store::SymbolKey;
use crate::order_panel::{OrderSide, PriceInputMode, StopLossType};

pub mod actor;
pub(crate) mod bootstrap;
pub mod gatr_snap;
pub mod handle;
pub mod price_defaults;
pub mod reducer;
pub mod store;
pub mod validate;

#[cfg(test)]
mod tests;

// Frozen Slice 1a public API. A handful of items do not yet have a
// call site in Slice 2 — those land in Slices 3–5 (`TickerIntentAccess`
// for reducer tests, `validate` for Slice 3 intake, `UpsertOutcome`
// for Slice 3 reducer branches, etc.). The re-exports stay as-is so
// downstream slices do not have to re-open this module.
#[allow(unused_imports)] // frozen API; some items consumed in Slices 3–5
pub use actor::{IntentError, IntentSource, NoOpReason, OrderIntentMsg, OrderIntentReply};
#[allow(unused_imports)] // frozen API; some items consumed in Slices 3–5
pub use handle::{TickerIntentAccess, TickerOrderIntentHandle};
pub use reducer::{apply_order_intent_msg, OrderIntentAppMsg};
#[allow(unused_imports)] // frozen API; some items consumed in Slices 3–5
pub use store::{TickerOrderIntentStore, UpsertOutcome};
#[allow(unused_imports)] // frozen API; some items consumed in Slices 3–5
pub use validate::{validate, IntentDefect};

/// Current on-disk schema version for [`TickerOrderIntent`].
///
/// Bumped whenever the persisted shape changes in a non-forward-compatible
/// way. `serde`'s `#[serde(default)]` handles additive fields without a
/// version bump; only breaking changes touch this constant.
pub const CURRENT_VERSION: u32 = 1;

/// Per-ticker order intent: the panel + bracket memory for one symbol.
///
/// This is the persistent counterpart of the per-symbol panel / bracket
/// state. It is intentionally broader than a single `OrderBracket`: it
/// covers *all* per-compound-key memory (Buy/Sell × Market/Limit/Stop/
/// StopLimit) so that flipping between order types does not lose the
/// inputs the user previously entered for that combo.
///
/// See `plan/ticker-order-state/README.md` section D2 for the
/// source-of-truth rules between this struct and `AnnotationStore`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TickerOrderIntent {
    /// Schema version. Present on every blob so [`migrate_v0_v1`] can
    /// distinguish an unversioned legacy row from a current one.
    #[serde(default = "default_version")]
    pub version: u32,
    /// The symbol this intent belongs to. Redundant with the map key
    /// but stored on-disk so the row is self-describing.
    pub symbol: SymbolKey,
    /// Which side the user most recently used for this symbol. The
    /// panel hydrates to this on symbol activation.
    #[serde(default = "default_side")]
    pub last_side: OrderSide,
    /// Which entry type the user most recently used for this symbol.
    #[serde(default)]
    pub last_entry_type: EntryType,
    /// Per-compound-key panel memory. Eight possible buckets, one per
    /// `(OrderSide, EntryType)` combination. Missing buckets deserialize
    /// to [`EntryMemory::default`] (which has `sl_enabled = true`).
    ///
    /// On disk this is serialized as an array of `[side, entry_type,
    /// memory]` triples, because JSON object keys must be strings.
    #[serde(default, with = "entries_serde")]
    pub entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    /// GATR snap anchor: the last-known price + GATR for this symbol,
    /// used by Slice 4 to decide whether to offer a snap.
    #[serde(default)]
    pub gatr_anchor: GatrAnchor,
    /// If a live bracket annotation exists for this symbol, its ID.
    /// `None` means no live bracket (fresh session, after cancel).
    #[serde(default)]
    pub live_annotation_id: Option<AnnotationId>,
    /// Broker-side order identifier, once Slice 5 wires this to
    /// `midas-broker`. Unused in Slice 1a but part of the locked shape
    /// so downstream slices do not re-open the struct.
    #[serde(default)]
    pub broker_order_id: Option<uuid::Uuid>,
    /// When `true`, the GATR snap rule skips this symbol. Exposed via
    /// a decorator pin toggle in Slice 4.
    #[serde(default)]
    pub pinned: bool,
    /// When this row was last written. Used by the GATR recency guard.
    #[serde(default = "default_updated_at")]
    pub updated_at: DateTime<Utc>,
}

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
/// bootstrapped, there is nothing to anchor against. See the
/// "first-touch-endorses" discussion in the plan's D4 section.
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

impl TickerOrderIntent {
    /// Construct a fresh, empty intent for a symbol. All fields are
    /// at their defaults; no buckets are populated.
    #[allow(dead_code)] // used by Slice 3 reducer constructors
    pub fn new(symbol: SymbolKey) -> Self {
        Self {
            version: CURRENT_VERSION,
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

/// Decode a persisted blob into a current-shape [`TickerOrderIntent`],
/// running any needed migrations.
///
/// Slice 1a's migration is a no-op: the on-disk shape *is* v1. The
/// function exists so that a future v2 landing can add a real migration
/// without having to touch every call site.
#[allow(dead_code)] // call site lands on v1 → v2 migration work
pub fn migrate_v0_v1(blob: &[u8]) -> Result<TickerOrderIntent, IntentDefect> {
    serde_json::from_slice::<TickerOrderIntent>(blob).map_err(|e| IntentDefect::DecodeFailed {
        reason: e.to_string(),
    })
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_side() -> OrderSide {
    OrderSide::Buy
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

fn default_updated_at() -> DateTime<Utc> {
    Utc::now()
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
