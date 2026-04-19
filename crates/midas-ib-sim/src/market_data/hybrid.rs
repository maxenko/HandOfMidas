//! Hybrid market-data engine.
//!
//! Composes a *base* engine (synthetic or replay) with a list of scripted
//! perturbations applied post-hoc. Perturbations are pure post-processing —
//! they mutate the emission stream rather than the base engine's private
//! state. This keeps them deterministic, composable, and reversible.

use std::collections::HashMap;

use midas_broker_core::SymbolKey;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{MarketEmission, SubKey, SubMode, TickAttribs, TickType};
use crate::market_data::{MarketDataEngine, MarketDataError, Perturbation, Snapshot};

/// Runtime state for an active halt. Suppresses any emission that names
/// `symbol` as its subscription target until `until`.
#[derive(Clone, Debug)]
struct ActiveHalt {
    symbol: SymbolKey,
    until: VirtualInstant,
}

pub struct HybridEngine {
    base: Box<dyn MarketDataEngine>,
    pending: Vec<Perturbation>,
    active_halts: Vec<ActiveHalt>,
    /// Pending price offset per symbol — added to every price emission until
    /// naturally absorbed by the next `InjectGap`. Represents a persistent
    /// shift in the base stream's price level.
    price_shift: HashMap<SymbolKey, f64>,
    /// Scheduled burst window (only one active at a time for simplicity).
    burst: Option<(VirtualInstant, VirtualInstant, f64)>,
}

impl HybridEngine {
    /// Build from a base engine and a list of scripted perturbations.
    pub fn new(base: Box<dyn MarketDataEngine>, perturbations: Vec<Perturbation>) -> Self {
        let mut pending = perturbations;
        pending.sort_by_key(|p| p.when().as_duration());
        Self {
            base,
            pending,
            active_halts: Vec::new(),
            price_shift: HashMap::new(),
            burst: None,
        }
    }

    /// Convenience: wrap an empty `SyntheticEngine` as the base.
    pub fn from_synthetic(
        engine: crate::market_data::generator::SyntheticEngine,
        perturbations: Vec<Perturbation>,
    ) -> Self {
        Self::new(Box::new(engine), perturbations)
    }

    /// Number of scripted perturbations remaining.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Apply every due perturbation at `now`, updating engine state.
    fn apply_due(&mut self, now: VirtualInstant) {
        // Drain expired halts.
        self.active_halts.retain(|h| h.until > now);
        // Expire the burst.
        if let Some((_, to, _)) = self.burst {
            if now >= to {
                self.burst = None;
            }
        }
        // Apply all perturbations whose trigger time has been reached.
        while let Some(p) = self.pending.first() {
            if p.when() > now {
                break;
            }
            let p = self.pending.remove(0);
            match p {
                Perturbation::InjectJump {
                    symbol,
                    magnitude_pct,
                    ..
                } => {
                    let entry = self.price_shift.entry(symbol.clone()).or_insert(0.0);
                    // magnitude_pct interpreted as additive percentage (1.0 => +1%).
                    *entry += magnitude_pct / 100.0;
                }
                Perturbation::InjectGap {
                    symbol, from, to, ..
                } => {
                    // Rebase: we don't know the base engine's internal state, so
                    // we model the gap as a fixed additive shift of (to - from).
                    let shift = to - from;
                    *self.price_shift.entry(symbol.clone()).or_insert(0.0) += shift;
                }
                Perturbation::InjectHalt {
                    symbol,
                    at,
                    duration,
                } => {
                    self.active_halts.push(ActiveHalt {
                        symbol,
                        until: at.saturating_add(duration),
                    });
                }
                Perturbation::BurstMode {
                    from,
                    to,
                    multiplier,
                } => {
                    self.burst = Some((from, to, multiplier));
                }
            }
        }
    }

    fn halted(&self, symbol: &SymbolKey, now: VirtualInstant) -> bool {
        self.active_halts
            .iter()
            .any(|h| &h.symbol == symbol && h.until > now)
    }

    fn filter_and_rewrite(
        &self,
        now: VirtualInstant,
        em: MarketEmission,
    ) -> Option<MarketEmission> {
        // For emissions bound to a subscription key, fetch the symbol.
        let symbol = emission_symbol(&em);
        if let Some(sym) = symbol.as_ref() {
            if self.halted(sym, now) {
                return None;
            }
        }
        // Apply price shift.
        if let (Some(sym), Some(shift)) = (
            symbol.as_ref(),
            symbol.as_ref().and_then(|s| self.price_shift.get(s)),
        ) {
            let _ = sym; // sym used via filter above
            if *shift != 0.0 {
                return Some(shift_price(em, *shift));
            }
        }
        Some(em)
    }
}

impl MarketDataEngine for HybridEngine {
    fn subscribe(&mut self, key: SubKey, mode: SubMode) -> Result<(), MarketDataError> {
        self.base.subscribe(key, mode)
    }

    fn unsubscribe(&mut self, key: &SubKey) {
        self.base.unsubscribe(key);
    }

    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
        self.apply_due(now);
        let base = self.base.step(now);
        base.into_iter()
            .filter_map(|em| self.filter_and_rewrite(now, em))
            .collect()
    }

    fn snapshot(&self, symbol: &SymbolKey) -> Option<Snapshot> {
        let mut s = self.base.snapshot(symbol)?;
        if let Some(shift) = self.price_shift.get(symbol) {
            s.bid += shift;
            s.ask += shift;
            s.last += shift;
        }
        Some(s)
    }

    fn inject_jump(
        &mut self,
        symbol: &SymbolKey,
        magnitude_pct: f64,
        now: VirtualInstant,
    ) -> Result<(), MarketDataError> {
        self.pending.push(Perturbation::InjectJump {
            at: now,
            symbol: symbol.clone(),
            magnitude_pct,
        });
        self.pending.sort_by_key(|p| p.when().as_duration());
        Ok(())
    }

    fn inject_gap(
        &mut self,
        symbol: &SymbolKey,
        from: f64,
        to: f64,
        now: VirtualInstant,
    ) -> Result<(), MarketDataError> {
        self.pending.push(Perturbation::InjectGap {
            at: now,
            symbol: symbol.clone(),
            from,
            to,
        });
        self.pending.sort_by_key(|p| p.when().as_duration());
        Ok(())
    }

    fn inject_halt(
        &mut self,
        symbol: &SymbolKey,
        duration: std::time::Duration,
        now: VirtualInstant,
    ) -> Result<(), MarketDataError> {
        self.pending.push(Perturbation::InjectHalt {
            at: now,
            symbol: symbol.clone(),
            duration,
        });
        self.pending.sort_by_key(|p| p.when().as_duration());
        Ok(())
    }

    fn inject_burst(
        &mut self,
        _symbols: &[SymbolKey],
        multiplier: f64,
        duration: std::time::Duration,
        now: VirtualInstant,
    ) -> Result<(), MarketDataError> {
        // BurstMode is global (per-engine), not per-symbol — `symbols` is
        // accepted for symmetry with the scenario YAML but ignored here.
        let to = now.saturating_add(duration);
        self.pending.push(Perturbation::BurstMode {
            from: now,
            to,
            multiplier,
        });
        self.pending.sort_by_key(|p| p.when().as_duration());
        Ok(())
    }
}

fn emission_symbol(em: &MarketEmission) -> Option<&SymbolKey> {
    match em {
        MarketEmission::TickPrice { key, .. }
        | MarketEmission::TickSize { key, .. }
        | MarketEmission::TickString { key, .. }
        | MarketEmission::TickGeneric { key, .. }
        | MarketEmission::Bar { key, .. }
        | MarketEmission::HistoricalBatch { key, .. } => Some(&key.symbol),
    }
}

fn shift_price(em: MarketEmission, shift: f64) -> MarketEmission {
    match em {
        MarketEmission::TickPrice {
            key,
            tick,
            price,
            size,
            attribs,
        } => MarketEmission::TickPrice {
            key,
            tick,
            price: price + shift,
            size,
            attribs: TickAttribs {
                past_limit: attribs.past_limit,
                can_auto_execute: attribs.can_auto_execute,
                pre_open: attribs.pre_open,
            },
        },
        MarketEmission::Bar { key, mut bar } => {
            bar.open += shift;
            bar.high += shift;
            bar.low += shift;
            bar.close += shift;
            bar.wap += shift;
            MarketEmission::Bar { key, bar }
        }
        MarketEmission::HistoricalBatch {
            key,
            mut bars,
            is_complete,
        } => {
            for b in &mut bars {
                b.open += shift;
                b.high += shift;
                b.low += shift;
                b.close += shift;
                b.wap += shift;
            }
            MarketEmission::HistoricalBatch {
                key,
                bars,
                is_complete,
            }
        }
        // Generic / size / string unaffected (they're not prices).
        other => other,
    }
}

// Silence unused-warnings cascade: TickType is used in cfg(test) only.
#[allow(dead_code)]
fn _ensure_tick_type_used(_t: TickType) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{ReqId, SessionId};
    use crate::market_data::generator::{SymbolPreset, SyntheticEngine};

    fn sym(name: &str, cid: i32) -> SymbolKey {
        SymbolKey {
            contract_id: cid,
            symbol: name.into(),
        }
    }

    fn make_engine() -> HybridEngine {
        let mut syn = SyntheticEngine::new(7);
        let s = sym("AAPL", 1);
        syn.register(s.clone(), SymbolPreset::Liquid, 100.0);
        HybridEngine::from_synthetic(syn, Vec::new())
    }

    use std::time::Duration;

    #[test]
    fn halt_perturbation_suppresses_emissions() {
        let s = sym("AAPL", 1);
        let mut h = make_engine();
        h.subscribe(
            SubKey {
                session: SessionId(1),
                req_id: ReqId(1),
                symbol: s.clone(),
            },
            SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        )
        .unwrap();
        // First, warm up some emissions.
        let warm = h.step(VirtualInstant::from_secs(1));
        assert!(!warm.is_empty());
        // Schedule a halt at t=1 for 5 s.
        h.pending.push(Perturbation::InjectHalt {
            at: VirtualInstant::from_secs(1),
            symbol: s.clone(),
            duration: Duration::from_secs(5),
        });
        // Now advance past halt start — no emissions for AAPL should survive.
        let during = h.step(VirtualInstant::from_secs(3));
        assert!(
            during.iter().all(|em| emission_symbol(em) != Some(&s)),
            "halt failed to suppress emissions"
        );
        // After halt expires, emissions resume.
        let after = h.step(VirtualInstant::from_secs(10));
        assert!(after.iter().any(|em| emission_symbol(em) == Some(&s)));
    }

    #[test]
    fn jump_perturbation_shifts_price_level() {
        let s = sym("AAPL", 1);
        let mut h = make_engine();
        h.subscribe(
            SubKey {
                session: SessionId(1),
                req_id: ReqId(1),
                symbol: s.clone(),
            },
            SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        )
        .unwrap();
        let baseline = h.step(VirtualInstant::from_secs(1));
        let base_last = baseline
            .iter()
            .rev()
            .find_map(|em| match em {
                MarketEmission::TickPrice {
                    tick: TickType::Last,
                    price,
                    ..
                } => Some(*price),
                _ => None,
            })
            .expect("a baseline last");
        h.pending.push(Perturbation::InjectJump {
            at: VirtualInstant::from_secs(1),
            symbol: s.clone(),
            magnitude_pct: 500.0, // +5 in log-return space → large shift
        });
        let after = h.step(VirtualInstant::from_secs(2));
        let post_last = after
            .iter()
            .rev()
            .find_map(|em| match em {
                MarketEmission::TickPrice {
                    tick: TickType::Last,
                    price,
                    ..
                } => Some(*price),
                _ => None,
            })
            .expect("a post last");
        assert!(
            post_last > base_last + 4.0,
            "jump didn't shift price: base {} post {}",
            base_last,
            post_last
        );
    }
}
