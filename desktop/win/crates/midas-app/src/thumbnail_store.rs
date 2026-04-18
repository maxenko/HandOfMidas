//! Per-ticker thumbnail interval preference.
//!
//! Session-scoped map from symbol to the user's last-chosen
//! [`Timeframe`] for that ticker's grid-cell thumbnail. Mirrors the
//! [`ChartViewStore`](crate::chart_view::ChartViewStore) pattern used
//! elsewhere in the app — per-ticker state, in-memory only, resets on
//! app restart.

use std::collections::HashMap;

use midas_core::Timeframe;

/// Cycle order for the interval-cycling click handler.
///
/// The click handler advances through this slice in order, looping
/// back to the first entry after the last. When the stored interval
/// is not a member (e.g. user configured `H1` via some other surface),
/// the next cycle resets to the first entry.
const CYCLE: &[Timeframe] = &[Timeframe::M1, Timeframe::M5, Timeframe::D1];

/// Per-ticker thumbnail interval preference.
///
/// Returns the stored override for a symbol, or the configured
/// default (`M5` by construction) for unknown symbols.
#[derive(Debug, Clone)]
pub struct ThumbnailStore {
    /// Per-symbol overrides. Key is the symbol as provided by the
    /// caller (case is preserved; callers should normalise if needed).
    intervals: HashMap<String, Timeframe>,
    /// Fallback returned by [`get`](ThumbnailStore::get) for symbols
    /// not present in `intervals`.
    default: Timeframe,
}

impl ThumbnailStore {
    /// Create an empty store with the default interval set to `M5`.
    pub fn new() -> Self {
        Self {
            intervals: HashMap::new(),
            default: Timeframe::M5,
        }
    }

    /// Create an empty store with an explicit default interval.
    #[allow(dead_code)] // exposed for Slice 5 (config-driven default)
    pub fn with_default(tf: Timeframe) -> Self {
        Self {
            intervals: HashMap::new(),
            default: tf,
        }
    }

    /// Return the stored interval for `symbol`, or the default if no
    /// override has been set for this symbol.
    pub fn get(&self, symbol: &str) -> Timeframe {
        self.intervals.get(symbol).copied().unwrap_or(self.default)
    }

    /// Advance the stored interval for `symbol` to the next entry in
    /// [`CYCLE`], wrapping around. If the current interval is not a
    /// member of the cycle, reset to the first entry. Returns the new
    /// value (also stored).
    pub fn cycle(&mut self, symbol: &str) -> Timeframe {
        let current = self.get(symbol);
        let next = match CYCLE.iter().position(|tf| *tf == current) {
            Some(idx) => CYCLE[(idx + 1) % CYCLE.len()],
            None => CYCLE[0],
        };
        self.intervals.insert(symbol.to_string(), next);
        next
    }

    /// Explicit setter. Useful for tests and for restoring state from
    /// a future persistence layer (Slice 5).
    #[allow(dead_code)] // exposed for tests + Slice 5 restore
    pub fn set(&mut self, symbol: &str, tf: Timeframe) {
        self.intervals.insert(symbol.to_string(), tf);
    }
}

impl Default for ThumbnailStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_returns_m5_for_unknown_symbol() {
        let store = ThumbnailStore::new();
        assert_eq!(store.get("AAPL"), Timeframe::M5);
        assert_eq!(store.get("MSFT"), Timeframe::M5);
    }

    #[test]
    fn with_default_respects_override() {
        let store = ThumbnailStore::with_default(Timeframe::D1);
        assert_eq!(store.get("AAPL"), Timeframe::D1);
    }

    #[test]
    fn cycle_rotates_m1_m5_d1() {
        let mut store = ThumbnailStore::new();
        // Default is M5, so first cycle advances to D1.
        assert_eq!(store.cycle("AAPL"), Timeframe::D1);
        // D1 -> M1 (wrap).
        assert_eq!(store.cycle("AAPL"), Timeframe::M1);
        // M1 -> M5.
        assert_eq!(store.cycle("AAPL"), Timeframe::M5);
        // M5 -> D1 (complete round trip).
        assert_eq!(store.cycle("AAPL"), Timeframe::D1);
    }

    #[test]
    fn cycle_preserves_other_symbols() {
        let mut store = ThumbnailStore::new();
        store.cycle("AAPL");
        store.cycle("AAPL");
        // MSFT was never cycled — still at default.
        assert_eq!(store.get("MSFT"), Timeframe::M5);
    }

    #[test]
    fn cycle_resets_to_first_when_current_not_in_cycle() {
        let mut store = ThumbnailStore::new();
        store.set("AAPL", Timeframe::H1); // not in CYCLE
        assert_eq!(store.cycle("AAPL"), Timeframe::M1);
    }

    #[test]
    fn set_then_get() {
        let mut store = ThumbnailStore::new();
        store.set("AAPL", Timeframe::D1);
        assert_eq!(store.get("AAPL"), Timeframe::D1);
        store.set("AAPL", Timeframe::M1);
        assert_eq!(store.get("AAPL"), Timeframe::M1);
    }
}
