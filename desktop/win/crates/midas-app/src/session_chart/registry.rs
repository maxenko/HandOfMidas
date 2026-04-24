//! [`SymbolSeriesRegistry`] — per-symbol shared-Arc lookup for the
//! new-stack chart panels, keyed by broker-core `SymbolKey`.
//!
//! ## Purpose
//!
//! Slice 2c of the chart-transition plan. Every session-chart panel
//! bound to a symbol ultimately writes into the SAME
//! `Arc<RwLock<CandleSeries>>` — the driver holds the writer end, the
//! widget holds read-only clones. The `QuoteBatch` handler needs a
//! way to find that shared Arc from a symbol string without walking
//! every floating window.
//!
//! The registry stores `Weak<RwLock<CandleSeries>>`. A weak handle
//! won't keep a closed-panel series alive, so once the last
//! `SessionChartDriver::Drop` releases its strong reference the
//! weak pointer returns `None` on `upgrade()`. Handlers prune these
//! on demand via [`SymbolSeriesRegistry::cleanup`].
//!
//! ## Design decisions
//!
//! - **DashMap over `Mutex<HashMap>`.** The registry is hit on every
//!   `QuoteBatch` handler tick (25+ ticks/sec across a full
//!   watchlist). DashMap's sharded concurrency lets the
//!   `QuoteBatch` handler and the driver's registration thread
//!   coexist without contention.
//! - **Weak pointers, not strong.** A lingering strong reference
//!   from the registry would keep a closed panel's series in memory.
//!   Weak references let the series free deterministically on panel
//!   close.
//! - **Keyed by `SymbolKey` only (no `RouterId`).** The plan's
//!   `RouterId` slot is future-facing — only one router exists
//!   today. When multi-router lands the key tuple widens here.

#![cfg(feature = "session_chart")]

use std::sync::{Arc, Weak};

use dashmap::DashMap;
use midas_bars::CandleSeries;
use midas_broker_core::SymbolKey;
use parking_lot::RwLock;

/// Shared-Arc lookup for session-chart candle series. Cheap to clone
/// (internal DashMap is `Arc`-backed); one instance lives on
/// `MidasApp`.
#[derive(Clone, Default)]
pub struct SymbolSeriesRegistry {
    inner: Arc<DashMap<SymbolKey, Weak<RwLock<CandleSeries>>>>,
}

impl SymbolSeriesRegistry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the series handle for a symbol. Drivers
    /// call this on spawn; the old weak handle is replaced silently.
    pub fn register(&self, sym: SymbolKey, series: &Arc<RwLock<CandleSeries>>) {
        tracing::debug!(
            target: "midas_app::session_chart::registry::register",
            symbol = %sym.symbol,
            "register series",
        );
        self.inner.insert(sym, Arc::downgrade(series));
    }

    /// Remove a symbol's entry. Drivers call this on drop — though
    /// the weak handle's failure-to-upgrade already papers over a
    /// missed deregister, the explicit call keeps the map tidy.
    pub fn deregister(&self, sym: &SymbolKey) {
        tracing::debug!(
            target: "midas_app::session_chart::registry::deregister",
            symbol = %sym.symbol,
            "deregister series",
        );
        self.inner.remove(sym);
    }

    /// Look up a shared series by symbol. Returns `None` when there
    /// is no entry OR when the entry's weak pointer has already
    /// expired (panel closed). Callers should not cache the returned
    /// `Arc` beyond the current call — the panel may close between
    /// ticks.
    pub fn get(&self, sym: &SymbolKey) -> Option<Arc<RwLock<CandleSeries>>> {
        let entry = self.inner.get(sym)?;
        entry.value().upgrade()
    }

    /// Sweep expired weak entries. Optional — the registry works
    /// correctly without cleanup (stale entries just waste a
    /// fixed-size slot), but handlers that want to keep the map
    /// compact can call this periodically.
    pub fn cleanup(&self) {
        self.inner.retain(|_, w| w.strong_count() > 0);
    }

    /// Number of registered entries (including expired weaks).
    /// Primarily for tests + diagnostics.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True iff the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use midas_bars::{BarPeriod, Symbol};
    use midas_calendar::xnys;
    use parking_lot::RwLock;

    use super::*;

    fn sym(s: &str) -> SymbolKey {
        SymbolKey {
            symbol: s.to_string(),
            contract_id: 0,
        }
    }

    fn fresh_series() -> Arc<RwLock<CandleSeries>> {
        let cal = xnys();
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            Symbol::new("SPY", cal.id()),
        )))
    }

    #[test]
    fn register_then_get_returns_the_same_arc() {
        let reg = SymbolSeriesRegistry::new();
        let s = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &s);
        let got = reg.get(&k).expect("registered");
        assert!(Arc::ptr_eq(&got, &s), "registry returned a different Arc");
    }

    #[test]
    fn get_missing_symbol_returns_none() {
        let reg = SymbolSeriesRegistry::new();
        assert!(reg.get(&sym("GOOG")).is_none());
    }

    #[test]
    fn expired_weak_returns_none_on_get() {
        let reg = SymbolSeriesRegistry::new();
        let k = sym("SPY");
        {
            let s = fresh_series();
            reg.register(k.clone(), &s);
            // Inside this scope `get` succeeds.
            assert!(reg.get(&k).is_some());
        }
        // `s` dropped → weak expired.
        assert!(
            reg.get(&k).is_none(),
            "expired weak must not upgrade to a live Arc",
        );
    }

    #[test]
    fn deregister_removes_entry() {
        let reg = SymbolSeriesRegistry::new();
        let s = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &s);
        assert_eq!(reg.len(), 1);
        reg.deregister(&k);
        assert_eq!(reg.len(), 0);
        assert!(reg.get(&k).is_none());
    }

    #[test]
    fn cleanup_purges_expired_weaks() {
        let reg = SymbolSeriesRegistry::new();
        let k_a = sym("SPY");
        let k_b = sym("AAPL");
        {
            let a = fresh_series();
            let b = fresh_series();
            reg.register(k_a.clone(), &a);
            reg.register(k_b.clone(), &b);
            assert_eq!(reg.len(), 2);
        }
        // Both series dropped → both entries are stale.
        reg.cleanup();
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_replaces_existing_entry() {
        let reg = SymbolSeriesRegistry::new();
        let a = fresh_series();
        let b = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &a);
        reg.register(k.clone(), &b);
        let got = reg.get(&k).unwrap();
        assert!(Arc::ptr_eq(&got, &b));
    }

    #[test]
    fn multiple_symbols_isolated() {
        let reg = SymbolSeriesRegistry::new();
        let a = fresh_series();
        let b = fresh_series();
        reg.register(sym("SPY"), &a);
        reg.register(sym("AAPL"), &b);
        let got_a = reg.get(&sym("SPY")).unwrap();
        let got_b = reg.get(&sym("AAPL")).unwrap();
        assert!(Arc::ptr_eq(&got_a, &a));
        assert!(Arc::ptr_eq(&got_b, &b));
    }

    // ── Slice 2c fan-out: 6 plan-mandated tests ─────────────────────

    /// Shared-Arc lookup: 5 "panels" cloning the registry-stored Arc
    /// all read the same CandleSeries (no per-panel copy).
    #[test]
    fn fan_out_five_panels_share_one_series() {
        let reg = SymbolSeriesRegistry::new();
        let shared = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &shared);

        // Five independent "panels" clone the registry's Arc.
        let panels: Vec<_> = (0..5).map(|_| reg.get(&k).unwrap()).collect();
        for p in &panels {
            assert!(Arc::ptr_eq(p, &shared));
        }
        assert_eq!(panels.len(), 5);
    }

    /// Fan-out write: one write-guard take on the registry's Arc is
    /// observable by every panel reading from the same Arc.
    #[test]
    fn fan_out_single_write_observed_by_many_panels() {
        use chrono::TimeZone;
        use midas_bars::{BarPeriod, Candle, Completeness, Ohlcv, Symbol};
        use midas_calendar::xnys;

        let reg = SymbolSeriesRegistry::new();
        let shared = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &shared);

        // Seed a candle so update_last_price has a target row.
        let cal = xnys();
        let sy = Symbol::new("SPY", cal.id());
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        let sess = cal.classify(ts);
        let win = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.0, 100.5, 1000, 1, None).unwrap();
        let c = Candle::new(
            sy,
            cal,
            BarPeriod::m1(),
            sess,
            win,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        {
            let mut g = shared.write();
            g.push(c);
        }

        // Collect five panel handles BEFORE the write.
        let panels: Vec<_> = (0..5).map(|_| reg.get(&k).unwrap()).collect();

        // One write-guard take on the registry's Arc.
        {
            let arc = reg.get(&k).unwrap();
            let mut g = arc.write();
            g.update_last_price(101.5);
        }
        // Every panel sees the fold.
        for p in &panels {
            let g = p.read();
            let row = g.at(0).unwrap();
            assert!((row.close() - 101.5).abs() < 1e-3);
        }
    }

    /// Burst coalescing: dropping the last strong Arc clears the
    /// entry from the registry's get() without cleanup() — the weak
    /// pointer just fails to upgrade.
    #[test]
    fn fan_out_burst_coalesces_when_entry_dies() {
        let reg = SymbolSeriesRegistry::new();
        let k = sym("SPY");
        {
            let s = fresh_series();
            reg.register(k.clone(), &s);
            // Burst: 100 quick get() + write sequences.
            for _ in 0..100 {
                if let Some(arc) = reg.get(&k) {
                    let _guard = arc.write();
                }
            }
        }
        // After the panel closed, burst fan-out becomes a no-op.
        for _ in 0..100 {
            assert!(reg.get(&k).is_none());
        }
    }

    /// 20-panel stress: 20 panels share one series; burst 100 writes
    /// through the registry's Arc; every panel observes the final
    /// close. Exercises the "no per-panel lock" design.
    #[test]
    fn fan_out_20_panel_stress() {
        use chrono::TimeZone;
        use midas_bars::{BarPeriod, Candle, Completeness, Ohlcv, Symbol};
        use midas_calendar::xnys;

        let reg = SymbolSeriesRegistry::new();
        let shared = fresh_series();
        let k = sym("SPY");
        reg.register(k.clone(), &shared);

        let cal = xnys();
        let sy = Symbol::new("SPY", cal.id());
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        let sess = cal.classify(ts);
        let win = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.0, 100.5, 1000, 1, None).unwrap();
        let c = Candle::new(
            sy,
            cal,
            BarPeriod::m1(),
            sess,
            win,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        {
            let mut g = shared.write();
            g.push(c);
        }

        // 20 panel handles.
        let panels: Vec<_> = (0..20).map(|_| reg.get(&k).unwrap()).collect();
        // 100 writes through the registry's shared Arc.
        for i in 0..100 {
            let arc = reg.get(&k).unwrap();
            let mut g = arc.write();
            g.update_last_price(100.0 + (i as f64) * 0.01);
        }
        // Every panel sees the final price.
        let expected_close = 100.0 + 99.0 * 0.01;
        for p in &panels {
            let g = p.read();
            let row = g.at(0).unwrap();
            assert!((row.close() - expected_close).abs() < 1e-3);
        }
    }

    /// Empty registry: get() on an unregistered symbol is a no-op
    /// from the handler's perspective (no panic, no lock taken).
    #[test]
    fn empty_registry_get_returns_none() {
        let reg = SymbolSeriesRegistry::new();
        assert!(reg.get(&sym("SPY")).is_none());
        assert_eq!(reg.len(), 0);
    }

    /// Missing symbol: handler's apply helper no-ops when the
    /// registry doesn't know the symbol.
    #[test]
    fn missing_symbol_no_op() {
        let reg = SymbolSeriesRegistry::new();
        reg.register(sym("SPY"), &fresh_series());
        // Query a different symbol.
        assert!(reg.get(&sym("AAPL")).is_none());
    }
}
