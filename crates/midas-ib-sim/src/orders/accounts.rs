//! Account, position, and P&L bookkeeping.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Request-position/account events".

use std::collections::BTreeMap;

use midas_broker_core::{ContractSpec, SymbolKey};

use crate::engine::types::{
    AcctValueUpdate, OrderEmission, PortfolioValueUpdate, PositionUpdate, Side,
};

#[derive(Clone, Debug, Default)]
pub struct Position {
    pub contract: Option<ContractSpec>,
    pub shares: f64,
    pub avg_cost: f64,
    pub realized_pnl: f64,
    pub last_mid: f64,
}

impl Position {
    pub fn market_value(&self) -> f64 {
        self.shares * self.last_mid
    }
    pub fn unrealized_pnl(&self) -> f64 {
        self.shares * (self.last_mid - self.avg_cost)
    }
}

#[derive(Clone, Debug)]
pub struct AccountState {
    pub account: String,
    pub cash: f64,
    pub realized_pnl: f64,
    pub positions: BTreeMap<SymbolKey, Position>,
}

impl AccountState {
    pub fn new(account: impl Into<String>, starting_cash: f64) -> Self {
        Self {
            account: account.into(),
            cash: starting_cash,
            realized_pnl: 0.0,
            positions: BTreeMap::new(),
        }
    }

    pub fn equity(&self) -> f64 {
        self.cash
            + self
                .positions
                .values()
                .map(|p| p.market_value())
                .sum::<f64>()
    }

    pub fn unrealized_pnl(&self) -> f64 {
        self.positions.values().map(|p| p.unrealized_pnl()).sum()
    }

    pub fn apply_fill(
        &mut self,
        symbol_key: &SymbolKey,
        contract: &ContractSpec,
        side: Side,
        shares: f64,
        price: f64,
    ) -> (PositionUpdate, PortfolioValueUpdate) {
        let pos = self.positions.entry(symbol_key.clone()).or_default();
        pos.contract = Some(contract.clone());
        pos.last_mid = price;

        let signed_shares = match side {
            Side::Buy => shares,
            Side::Sell => -shares,
        };
        let notional = price * shares;
        match side {
            Side::Buy => self.cash -= notional,
            Side::Sell => self.cash += notional,
        }

        let new_shares = pos.shares + signed_shares;
        if pos.shares.signum() == signed_shares.signum() || pos.shares == 0.0 {
            let old_cost_notional = pos.shares.abs() * pos.avg_cost;
            let new_cost_notional = shares * price;
            let total_shares = pos.shares.abs() + shares;
            pos.avg_cost = if total_shares > 0.0 {
                (old_cost_notional + new_cost_notional) / total_shares
            } else {
                0.0
            };
        } else {
            let closing = shares.min(pos.shares.abs());
            let pnl = match pos.shares.signum() as i32 {
                1 => (price - pos.avg_cost) * closing,
                -1 => (pos.avg_cost - price) * closing,
                _ => 0.0,
            };
            pos.realized_pnl += pnl;
            self.realized_pnl += pnl;
            if shares > pos.shares.abs() {
                pos.avg_cost = price;
            }
        }
        pos.shares = new_shares;
        if pos.shares.abs() < f64::EPSILON {
            pos.shares = 0.0;
            pos.avg_cost = 0.0;
        }

        let pu = PositionUpdate {
            account: self.account.clone(),
            contract: contract.clone(),
            position: pos.shares,
            avg_cost: pos.avg_cost,
        };
        let pv = PortfolioValueUpdate {
            contract: contract.clone(),
            position: pos.shares,
            market_price: pos.last_mid,
            market_value: pos.market_value(),
            average_cost: pos.avg_cost,
            unrealized_pnl: pos.unrealized_pnl(),
            realized_pnl: pos.realized_pnl,
            account: self.account.clone(),
        };
        (pu, pv)
    }

    pub fn snapshot_positions(&self) -> Vec<OrderEmission> {
        let mut out: Vec<OrderEmission> = self
            .positions
            .iter()
            .filter(|(_, p)| p.shares.abs() > f64::EPSILON)
            .filter_map(|(_, p)| {
                p.contract.as_ref().map(|c| {
                    OrderEmission::Position(PositionUpdate {
                        account: self.account.clone(),
                        contract: c.clone(),
                        position: p.shares,
                        avg_cost: p.avg_cost,
                    })
                })
            })
            .collect();
        out.push(OrderEmission::PositionEnd);
        out
    }

    pub fn snapshot_account_data(&self) -> Vec<OrderEmission> {
        let mut out = Vec::new();
        let keys: [(&str, String); 5] = [
            ("CashBalance", format!("{:.2}", self.cash)),
            ("NetLiquidation", format!("{:.2}", self.equity())),
            ("AvailableFunds", format!("{:.2}", self.cash.max(0.0))),
            ("RealizedPnL", format!("{:.2}", self.realized_pnl)),
            ("UnrealizedPnL", format!("{:.2}", self.unrealized_pnl())),
        ];
        for (k, v) in keys {
            out.push(OrderEmission::AcctValue(AcctValueUpdate {
                key: k.to_string(),
                value: v,
                currency: "USD".into(),
                account: self.account.clone(),
            }));
        }
        for (_, p) in self.positions.iter() {
            if let Some(c) = p.contract.clone() {
                out.push(OrderEmission::PortfolioValue(PortfolioValueUpdate {
                    contract: c,
                    position: p.shares,
                    market_price: p.last_mid,
                    market_value: p.market_value(),
                    average_cost: p.avg_cost,
                    unrealized_pnl: p.unrealized_pnl(),
                    realized_pnl: p.realized_pnl,
                    account: self.account.clone(),
                }));
            }
        }
        out.push(OrderEmission::AcctDownloadEnd(self.account.clone()));
        out
    }
}

/// Derive a deterministic [`SymbolKey`] for a [`ContractSpec`].
///
/// The contract id is folded through a SplitMix64 mixer driven off the
/// variant tag + every distinguishing field (symbol, exchange, currency,
/// expiry, strike-bits, right, pair). This is stable across Rust toolchain
/// versions — unlike [`std::collections::hash_map::DefaultHasher`], which
/// std docs declare non-portable.
///
/// The output `i32` is non-negative so downstream consumers expecting a
/// positive contract id aren't caught out by `i32::MIN.abs()` panics.
pub fn symbol_key_for(contract: &ContractSpec) -> SymbolKey {
    use crate::orders::determinism::splitmix64;

    // Fold bytes into a running `u64` via repeated SplitMix64 application.
    // Equivalent to a tiny stream hash with a fixed avalanche function.
    #[inline]
    fn mix_bytes(mut acc: u64, bytes: &[u8]) -> u64 {
        for &b in bytes {
            acc = splitmix64(acc ^ b as u64);
        }
        // Length-terminate so "ab" and "a\0b" don't alias after concat.
        splitmix64(acc ^ bytes.len() as u64)
    }

    #[inline]
    fn mix_str(acc: u64, s: &str) -> u64 {
        mix_bytes(acc, s.as_bytes())
    }

    // Start from a fixed nonzero constant so the empty-variant case still
    // propagates bits through the mixer.
    const DOMAIN_SEED: u64 = 0xA55A_A55A_5AA5_5AA5;
    let mut acc = splitmix64(DOMAIN_SEED);

    match contract {
        ContractSpec::Stock {
            symbol,
            exchange,
            currency,
        } => {
            acc = splitmix64(acc ^ 1);
            acc = mix_str(acc, symbol);
            acc = mix_str(acc, exchange);
            acc = mix_str(acc, currency);
        }
        ContractSpec::Option {
            symbol,
            expiry,
            strike,
            right,
            exchange,
        } => {
            acc = splitmix64(acc ^ 2);
            acc = mix_str(acc, symbol);
            acc = mix_str(acc, expiry);
            // OrderedFloat round-trips through the underlying f64 bits; using
            // to_bits sidesteps any NaN / -0.0 weirdness.
            acc = splitmix64(acc ^ strike.into_inner().to_bits());
            acc = splitmix64(
                acc ^ match right {
                    midas_broker_core::OptionRight::Call => 0xC,
                    midas_broker_core::OptionRight::Put => 0xD,
                },
            );
            acc = mix_str(acc, exchange);
        }
        ContractSpec::Future {
            symbol,
            expiry,
            exchange,
        } => {
            acc = splitmix64(acc ^ 3);
            acc = mix_str(acc, symbol);
            acc = mix_str(acc, expiry);
            acc = mix_str(acc, exchange);
        }
        ContractSpec::Forex { pair } => {
            acc = splitmix64(acc ^ 4);
            acc = mix_str(acc, pair);
        }
    }

    // Take the low 31 bits as a positive i32 — gives us 2^31 distinct ids
    // and avoids `i32::MIN.abs()` (which panics in debug).
    let contract_id = (acc & 0x7FFF_FFFF) as i32;
    SymbolKey {
        contract_id,
        symbol: contract.symbol().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock(sym: &str) -> ContractSpec {
        ContractSpec::Stock {
            symbol: sym.into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        }
    }

    #[test]
    fn long_position_from_one_buy() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 150.0);
        let p = &a.positions[&k];
        assert_eq!(p.shares, 100.0);
        assert_eq!(p.avg_cost, 150.0);
    }

    #[test]
    fn averaging_two_buys() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 150.0);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 160.0);
        assert!((a.positions[&k].avg_cost - 155.0).abs() < 1e-9);
    }

    #[test]
    fn realize_pnl_on_flat() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 150.0);
        a.apply_fill(&k, &c, Side::Sell, 100.0, 155.0);
        assert!((a.realized_pnl - 500.0).abs() < 1e-9);
    }

    #[test]
    fn short_then_cover() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("TSLA");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Sell, 50.0, 200.0);
        a.apply_fill(&k, &c, Side::Buy, 50.0, 190.0);
        assert!((a.realized_pnl - 500.0).abs() < 1e-9);
    }

    #[test]
    fn reversal_through_zero_sets_new_basis() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("MSFT");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 50.0, 100.0);
        a.apply_fill(&k, &c, Side::Sell, 150.0, 110.0);
        let p = &a.positions[&k];
        assert_eq!(p.shares, -100.0);
        assert_eq!(p.avg_cost, 110.0);
    }

    #[test]
    fn snapshot_positions_emits_position_end_last() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 150.0);
        let snap = a.snapshot_positions();
        assert!(matches!(snap.last(), Some(OrderEmission::PositionEnd)));
    }

    #[test]
    fn account_data_snapshot_ends_with_download_end() {
        let mut a = AccountState::new("U1", 100_000.0);
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        a.apply_fill(&k, &c, Side::Buy, 100.0, 150.0);
        let snap = a.snapshot_account_data();
        assert!(matches!(
            snap.last(),
            Some(OrderEmission::AcctDownloadEnd(_))
        ));
    }

    /// Pin `symbol_key_for` to a byte-exact `contract_id` so a fixture replay
    /// across Rust toolchain versions keeps producing identical SymbolKeys.
    /// Changing this value means every recorded scenario output must be
    /// regenerated — treat it as a fixture-breaking change.
    #[test]
    fn symbol_key_for_is_stable_across_toolchain() {
        let c = stock("AAPL");
        let k = symbol_key_for(&c);
        assert_eq!(
            k.contract_id, 0x6DCB_34D9,
            "symbol_key_for drift: fixture-breaking change. got 0x{:08x}",
            k.contract_id
        );
        assert_eq!(k.symbol, "AAPL");
    }

    #[test]
    fn symbol_key_contract_id_is_non_negative() {
        // Cross-spot-check a handful of arbitrary contracts. Any negative
        // id means the sign-masking regressed.
        for sym in ["A", "AAPL", "GOOGL", "TSLA", "XYZ_WITH_LONG_SYMBOL_NAME"] {
            let k = symbol_key_for(&stock(sym));
            assert!(k.contract_id >= 0, "negative contract_id for {sym}");
        }
    }

    #[test]
    fn symbol_key_distinguishes_variants() {
        // Variant tag must actually matter — Stock/Future/Option/Forex with
        // the same "symbol" must map to different contract ids.
        let s = symbol_key_for(&ContractSpec::Stock {
            symbol: "X".into(),
            exchange: "E".into(),
            currency: "USD".into(),
        });
        let f = symbol_key_for(&ContractSpec::Future {
            symbol: "X".into(),
            expiry: "E".into(),
            exchange: "E".into(),
        });
        let fx = symbol_key_for(&ContractSpec::Forex { pair: "X".into() });
        assert_ne!(s.contract_id, f.contract_id);
        assert_ne!(s.contract_id, fx.contract_id);
        assert_ne!(f.contract_id, fx.contract_id);
    }
}
