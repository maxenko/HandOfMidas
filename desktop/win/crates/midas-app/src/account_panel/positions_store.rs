//! In-memory per-account position store.
//!
//! Receives `BrokerEvent::PositionUpdate` (either individually during
//! backfill or batched through the coalesced subscription) and maintains
//! a symbol-keyed map of raw position records. Derived fields (market
//! value, P/L, change %) are computed at render time in the Positions
//! tab (Slice 5) — this store only owns the raw inputs.
//!
//! **Probe findings (Slice 4):**
//! - Last-price source is `midas_broker::BrokerEvent::Tick { last: Option<f64>, .. }`;
//!   Slice 5 wires the `last` field into [`PositionStore::update_last_price`].
//! - `BrokerEvent::AccountPnlUpdated` does NOT exist. Closest is
//!   `BrokerEvent::PnlUpdate { daily_pnl, unrealized_pnl, realized_pnl }`
//!   which is account-wide, not per-symbol — so per-position
//!   `realized_pnl` / `daily_pnl` render as em-dash in Slice 5, as the
//!   plan accepts.
//!
//! Removal rule: an incoming update with `qty == 0` (within a 1e-9 abs
//! tolerance) removes the symbol rather than keeping a zero row, matching
//! IB's convention.
//!
//! Every mutation bumps `generation` so the view layer can cheaply
//! detect when it needs to rebuild its display rows.

use std::collections::HashMap;
use std::time::Instant;

/// Absolute tolerance used when deciding whether an incoming quantity
/// counts as "zero" (triggering row removal).
const QTY_ZERO_EPSILON: f64 = 1e-9;

/// Raw position record. Derived columns (P/L, market value, side tint)
/// live in the Positions tab's display-row builder, not here.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionRaw {
    /// Trading symbol, e.g. `"AAPL"`. Lowercase normalisation is NOT
    /// applied — the broker decides casing.
    pub symbol: String,
    /// Signed share count. `> 0` = long, `< 0` = short. `== 0` is never
    /// stored (the store removes the row instead).
    pub qty: f64,
    /// Average cost basis per share.
    pub avg_cost: f64,
    /// Most-recent last-trade price. `None` until a `Tick` arrives for
    /// this symbol (Slice 5 wiring).
    pub last_price: Option<f32>,
    /// Monotonic-clock timestamp of the most recent `last_price` write.
    /// Used by the Positions tab to grey out stale prices.
    pub last_price_ts: Option<Instant>,
    /// Reference price at session open. Feeds the `% change` column in
    /// Slice 5. `None` until the broker surfaces a day-open value.
    pub session_open_price: Option<f32>,
}

/// Symbol-keyed store of [`PositionRaw`] rows.
///
/// Written to by:
/// - The single-event path inside `Message::BrokerEventReceived`
///   ([`Self::apply`]) — used during reconnect backfills where events
///   arrive one-at-a-time.
/// - The coalesced path from `positions_subscription`
///   ([`Self::apply_batch`]) — used during steady-state updates.
///
/// Both paths are idempotent: last write wins. The store is always
/// mutated on iced's single-threaded `update()`, so no synchronisation
/// is needed.
#[derive(Default, Debug, Clone)]
pub struct PositionStore {
    positions: HashMap<String, PositionRaw>,
    generation: u64,
}

#[allow(dead_code)] // `update_last_price`, `positions`, `generation`, `is_empty` consumed by Slice 5.
impl PositionStore {
    /// Fresh, empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a single `PositionUpdate` event. `qty == 0` (within
    /// tolerance) removes the symbol; otherwise inserts or overwrites
    /// the row in-place, preserving `last_price`/`session_open_price`
    /// if a prior row existed.
    pub fn apply(&mut self, update: &midas_broker::BrokerEvent) {
        if let midas_broker::BrokerEvent::PositionUpdate {
            symbol,
            quantity,
            avg_cost,
            ..
        } = update
        {
            self.upsert(symbol, *quantity, *avg_cost);
        }
    }

    /// Apply a coalesced batch of already-parsed position rows.
    ///
    /// `batch` is expected to be pre-folded by
    /// `fold_latest_per_symbol` so each symbol appears at most once.
    /// This method still tolerates duplicates (last wins) for
    /// robustness.
    pub fn apply_batch(&mut self, batch: &[PositionRaw]) {
        for raw in batch {
            self.upsert(&raw.symbol, raw.qty, raw.avg_cost);
        }
    }

    /// Update the last-price snapshot for a symbol, preserving every
    /// other field. Silently no-ops if the symbol is not present
    /// (a tick can arrive for a symbol the user watches but holds
    /// no position in — that's the watchlist's concern, not ours).
    pub fn update_last_price(&mut self, symbol: &str, price: f32) {
        if let Some(row) = self.positions.get_mut(symbol) {
            row.last_price = Some(price);
            row.last_price_ts = Some(Instant::now());
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Iterate over all held positions in arbitrary order.
    pub fn positions(&self) -> impl Iterator<Item = &PositionRaw> {
        self.positions.values()
    }

    /// Monotonic counter bumped on every mutation (insert / overwrite /
    /// removal / last-price update). The Positions tab diffs this
    /// against its last-seen generation to decide when to rebuild.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of currently-held positions.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether the store holds no positions.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    // ── internals ────────────────────────────────────────────────────

    fn upsert(&mut self, symbol: &str, qty: f64, avg_cost: f64) {
        if qty.abs() < QTY_ZERO_EPSILON {
            if self.positions.remove(symbol).is_some() {
                self.generation = self.generation.wrapping_add(1);
            }
            return;
        }

        match self.positions.get_mut(symbol) {
            Some(existing) => {
                existing.qty = qty;
                existing.avg_cost = avg_cost;
                // Preserve last_price / session_open_price across
                // overwrites — those arrive from different broker
                // streams and aren't in `PositionUpdate`.
            }
            None => {
                self.positions.insert(
                    symbol.to_owned(),
                    PositionRaw {
                        symbol: symbol.to_owned(),
                        qty,
                        avg_cost,
                        last_price: None,
                        last_price_ts: None,
                        session_open_price: None,
                    },
                );
            }
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(symbol: &str, qty: f64, avg_cost: f64) -> midas_broker::BrokerEvent {
        midas_broker::BrokerEvent::PositionUpdate {
            account: "TEST".to_string(),
            symbol: symbol.to_string(),
            con_id: 0,
            quantity: qty,
            avg_cost,
        }
    }

    fn raw(symbol: &str, qty: f64, avg_cost: f64) -> PositionRaw {
        PositionRaw {
            symbol: symbol.to_string(),
            qty,
            avg_cost,
            last_price: None,
            last_price_ts: None,
            session_open_price: None,
        }
    }

    #[test]
    fn apply_inserts_new_symbol() {
        let mut store = PositionStore::new();
        let g0 = store.generation();
        store.apply(&event("AAPL", 100.0, 150.0));
        assert_eq!(store.len(), 1);
        assert!(store.generation() > g0);
        let pos = store.positions().next().unwrap();
        assert_eq!(pos.symbol, "AAPL");
        assert_eq!(pos.qty, 100.0);
        assert_eq!(pos.avg_cost, 150.0);
    }

    #[test]
    fn apply_overwrites_existing_symbol_and_preserves_last_price() {
        let mut store = PositionStore::new();
        store.apply(&event("AAPL", 100.0, 150.0));
        store.update_last_price("AAPL", 175.5);
        store.apply(&event("AAPL", 200.0, 160.0));
        assert_eq!(store.len(), 1);
        let pos = store.positions().next().unwrap();
        assert_eq!(pos.qty, 200.0);
        assert_eq!(pos.avg_cost, 160.0);
        assert_eq!(pos.last_price, Some(175.5));
        assert!(pos.last_price_ts.is_some());
    }

    #[test]
    fn apply_removes_symbol_when_qty_is_zero() {
        let mut store = PositionStore::new();
        store.apply(&event("AAPL", 100.0, 150.0));
        assert_eq!(store.len(), 1);
        let g_before_remove = store.generation();
        store.apply(&event("AAPL", 0.0, 150.0));
        assert_eq!(store.len(), 0);
        assert!(store.generation() > g_before_remove);
    }

    #[test]
    fn apply_zero_qty_on_absent_symbol_does_not_bump_generation() {
        let mut store = PositionStore::new();
        let g0 = store.generation();
        store.apply(&event("AAPL", 0.0, 0.0));
        assert_eq!(store.generation(), g0, "no mutation = no generation bump");
    }

    #[test]
    fn apply_batch_coalesces_multiple_symbols() {
        let mut store = PositionStore::new();
        let batch = vec![
            raw("AAPL", 100.0, 150.0),
            raw("GME", -50.0, 18.0),
            raw("AS", 200.0, 12.0),
        ];
        store.apply_batch(&batch);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn apply_batch_last_wins_on_duplicate_symbol() {
        let mut store = PositionStore::new();
        let batch = vec![raw("AAPL", 100.0, 150.0), raw("AAPL", 200.0, 160.0)];
        store.apply_batch(&batch);
        assert_eq!(store.len(), 1);
        let pos = store.positions().next().unwrap();
        assert_eq!(pos.qty, 200.0);
        assert_eq!(pos.avg_cost, 160.0);
    }

    #[test]
    fn update_last_price_preserves_other_fields() {
        let mut store = PositionStore::new();
        store.apply(&event("AAPL", 100.0, 150.0));
        let g_before = store.generation();
        store.update_last_price("AAPL", 175.5);
        let pos = store.positions().next().unwrap();
        assert_eq!(pos.qty, 100.0);
        assert_eq!(pos.avg_cost, 150.0);
        assert_eq!(pos.last_price, Some(175.5));
        assert!(store.generation() > g_before);
    }

    #[test]
    fn update_last_price_on_absent_symbol_is_noop() {
        let mut store = PositionStore::new();
        let g0 = store.generation();
        store.update_last_price("AAPL", 100.0);
        assert_eq!(store.generation(), g0);
    }

    #[test]
    fn generation_is_monotonic_across_mixed_ops() {
        let mut store = PositionStore::new();
        let mut prev = store.generation();
        for qty in [10.0, 20.0, 30.0, 0.0, 5.0, 0.0] {
            store.apply(&event("AAPL", qty, 100.0));
            if (qty - 0.0f64).abs() > QTY_ZERO_EPSILON || !store.is_empty() {
                // Mutation happened — generation must advance.
                assert!(store.generation() >= prev);
                prev = store.generation();
            }
        }
    }

    #[test]
    fn non_position_broker_events_are_ignored() {
        let mut store = PositionStore::new();
        let ev = midas_broker::BrokerEvent::Connected { server_version: 1 };
        let g0 = store.generation();
        store.apply(&ev);
        assert_eq!(store.generation(), g0);
        assert!(store.is_empty());
    }

    #[test]
    fn qty_within_epsilon_is_treated_as_zero() {
        let mut store = PositionStore::new();
        store.apply(&event("AAPL", 100.0, 150.0));
        // Well within epsilon.
        store.apply(&event("AAPL", 1e-12, 150.0));
        assert_eq!(store.len(), 0);
    }
}
