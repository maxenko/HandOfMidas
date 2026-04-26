//! `SymbolResolver` — ticker → `(Symbol, calendar, con_id)` lookup.
//!
//! Per R2-G-9 the ideal `Symbol` type does NOT carry the calendar
//! directly; resolution always goes through a `SymbolResolver`. This
//! keeps calendar identity explicit and testable, and allows different
//! providers (sim vs IB) to resolve the same ticker differently — IB
//! via `reqContractDetails`, sim via a synthesized stable hash.
//!
//! Two built-in resolvers:
//! - [`StaticSymbolResolver`] — hashmap-backed, seeded with a handful
//!   of well-known tickers (AAPL, SPY, MSFT, BTC-USD, …). Register
//!   additional tickers with [`StaticSymbolResolver::register`].
//! - [`HeuristicSymbolResolver`] — heuristically routes anything
//!   resembling a crypto pair ("-USD"/"USDT"/"BTC"/"ETH") to the
//!   crypto calendar; everything else goes to XNYS. Synthesizes
//!   `con_id` via a stable DJB2-style hash of the ticker so IDs are
//!   repeatable across runs. The synthesized `con_id` is sim-only —
//!   IB's real `reqContractDetails` returns the authoritative id —
//!   but `.calendar` is a pure function of the ticker string and is
//!   universally valid for both sim and IB symbols. The legacy
//!   chart's session-band overlay (ETH shading) reads `.calendar`
//!   directly and ignores `.contract_id`.

use std::collections::HashMap;

use midas_bars::Symbol;
use midas_calendar::{crypto_spot, xnys, ExchangeCalendar};

/// Result of a successful resolution.
///
/// `contract_id` is the IB `con_id` the provider needs when calling
/// `historical_bars` / `subscribe_realtime_bars`. For sim-only tickers
/// it's a synthesized stable value.
#[derive(Clone)]
pub struct ResolvedSymbol {
    pub symbol: Symbol,
    pub calendar: &'static dyn ExchangeCalendar,
    pub contract_id: i32,
}

impl std::fmt::Debug for ResolvedSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedSymbol")
            .field("symbol", &self.symbol)
            .field("calendar", &self.calendar.id())
            .field("contract_id", &self.contract_id)
            .finish()
    }
}

/// Resolution failure modes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("unknown ticker: {0}")]
    UnknownTicker(String),
    #[error("lookup failed: {0}")]
    Lookup(String),
}

/// Ticker → symbol/calendar/con_id resolver.
pub trait SymbolResolver: Send + Sync + 'static {
    fn resolve(&self, ticker: &str) -> Result<ResolvedSymbol, ResolveError>;
}

// ---------------------------------------------------------------------------
// StaticSymbolResolver
// ---------------------------------------------------------------------------

/// Hashmap-backed resolver seeded with a default set of tickers.
///
/// Construct with [`StaticSymbolResolver::new`] to get the defaults, or
/// [`StaticSymbolResolver::empty`] for a blank slate. Call
/// [`register`](Self::register) to add entries.
pub struct StaticSymbolResolver {
    entries: HashMap<String, (&'static dyn ExchangeCalendar, i32)>,
}

impl StaticSymbolResolver {
    /// Build an empty resolver. Use [`register`](Self::register) to add
    /// mappings.
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Build a resolver seeded with a small canonical roster:
    /// - Crypto: `BTC-USD` (con_id 1_000_000_001), `ETH-USD`
    ///   (1_000_000_002). Synthetic IDs sit well outside IB's real
    ///   con_id range.
    /// - Equities (XNYS): `AAPL` (265598), `SPY` (756733),
    ///   `MSFT` (272093), `TSLA` (76792991), `NVDA` (4815747).
    pub fn new() -> Self {
        let mut r = Self::empty();
        r.register("BTC-USD", crypto_spot(), 1_000_000_001);
        r.register("ETH-USD", crypto_spot(), 1_000_000_002);
        r.register("AAPL", xnys(), 265598);
        r.register("SPY", xnys(), 756733);
        r.register("MSFT", xnys(), 272093);
        r.register("TSLA", xnys(), 76_792_991);
        r.register("NVDA", xnys(), 4_815_747);
        r
    }

    /// Add or override a ticker mapping.
    pub fn register(
        &mut self,
        ticker: &str,
        calendar: &'static dyn ExchangeCalendar,
        contract_id: i32,
    ) {
        self.entries
            .insert(ticker.to_string(), (calendar, contract_id));
    }
}

impl Default for StaticSymbolResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolResolver for StaticSymbolResolver {
    fn resolve(&self, ticker: &str) -> Result<ResolvedSymbol, ResolveError> {
        match self.entries.get(ticker) {
            Some(&(calendar, contract_id)) => Ok(ResolvedSymbol {
                symbol: Symbol::from_ticker_leak(ticker, calendar.id()),
                calendar,
                contract_id,
            }),
            None => Err(ResolveError::UnknownTicker(ticker.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// HeuristicSymbolResolver
// ---------------------------------------------------------------------------

/// Heuristic resolver. Routes crypto-looking tickers to the crypto
/// calendar, everything else to XNYS, and synthesizes a stable
/// `con_id` from a seeded DJB2 hash of the ticker. IDs are reproducible
/// across runs without a shared static table.
///
/// **Scope of "sim-only"**: only the synthesized `.contract_id` is
/// sim-specific — IB's `reqContractDetails` provides the real id when
/// the resolver runs against the IB backend. The `.calendar` field is
/// a pure function of the ticker string and is universally valid for
/// both sim and IB symbols. The legacy chart's session-band overlay
/// (ETH shading) reads `.calendar` only.
///
/// Heuristics (any match → crypto):
/// - Suffix `-USD`, `-USDT`, `-USDC`.
/// - Ticker contains `BTC` or `ETH` anywhere.
///
/// Everything else falls through to XNYS. The synthesized con_id starts
/// at 2_000_000_000 for crypto and 3_000_000_000 for XNYS (coerced to
/// positive `i32`) so the two buckets are visually distinct and do not
/// collide with `StaticSymbolResolver`'s synthetic 1_000_000_00x range.
pub struct HeuristicSymbolResolver;

impl HeuristicSymbolResolver {
    pub const fn new() -> Self {
        Self
    }

    fn is_crypto(ticker: &str) -> bool {
        let upper = ticker.to_ascii_uppercase();
        upper.ends_with("-USD")
            || upper.ends_with("-USDT")
            || upper.ends_with("-USDC")
            || upper.contains("BTC")
            || upper.contains("ETH")
    }

    /// Seeded DJB2 hash. Deterministic across runs — unlike the default
    /// `std::collections::hash_map::DefaultHasher` which is
    /// randomly-seeded per process.
    fn stable_hash(ticker: &str, seed: u32) -> u32 {
        let mut h: u32 = 5381u32.wrapping_add(seed);
        for b in ticker.as_bytes() {
            h = h.wrapping_mul(33).wrapping_add(u32::from(*b));
        }
        h
    }
}

impl Default for HeuristicSymbolResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolResolver for HeuristicSymbolResolver {
    fn resolve(&self, ticker: &str) -> Result<ResolvedSymbol, ResolveError> {
        let (calendar, seed, base): (&'static dyn ExchangeCalendar, u32, i32) =
            if Self::is_crypto(ticker) {
                (crypto_spot(), 0xC0DE_C0DE, 2_000_000_000)
            } else {
                (xnys(), 0x57B1_57B1, 0)
            };
        // Map the 32-bit hash into [0, i32::MAX - base) so `base + off`
        // stays non-negative. `base` is 0 or 2_000_000_000; leave a
        // 128 M-slot spread for non-crypto that we don't need to collide
        // with the 2B+ crypto space.
        let window: u32 = match base {
            0 => 1_000_000_000,
            _ => 100_000_000,
        };
        let off = Self::stable_hash(ticker, seed) % window;
        let contract_id = base.saturating_add(off as i32);
        Ok(ResolvedSymbol {
            symbol: Symbol::from_ticker_leak(ticker, calendar.id()),
            calendar,
            contract_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use midas_calendar::{CRYPTO_SPOT_ID, XNYS_ID};

    use super::*;

    #[test]
    fn static_resolver_default_knows_btc() {
        let r = StaticSymbolResolver::new();
        let res = r.resolve("BTC-USD").unwrap();
        assert_eq!(res.calendar.id(), CRYPTO_SPOT_ID);
        assert_eq!(res.symbol.ticker(), "BTC-USD");
        assert_eq!(res.symbol.calendar(), CRYPTO_SPOT_ID);
        assert_eq!(res.contract_id, 1_000_000_001);
    }

    #[test]
    fn static_resolver_default_knows_aapl() {
        let r = StaticSymbolResolver::new();
        let res = r.resolve("AAPL").unwrap();
        assert_eq!(res.calendar.id(), XNYS_ID);
        assert_eq!(res.symbol.ticker(), "AAPL");
        assert_eq!(res.contract_id, 265598);
    }

    #[test]
    fn static_resolver_unknown_errors() {
        let r = StaticSymbolResolver::new();
        let err = r.resolve("WEIRD_SYMBOL_XYZ").unwrap_err();
        assert!(matches!(err, ResolveError::UnknownTicker(_)));
    }

    #[test]
    fn static_resolver_register_adds() {
        let mut r = StaticSymbolResolver::empty();
        assert!(r.resolve("AAPL").is_err());
        r.register("AAPL", xnys(), 42);
        let res = r.resolve("AAPL").unwrap();
        assert_eq!(res.contract_id, 42);
        assert_eq!(res.calendar.id(), XNYS_ID);
    }

    #[test]
    fn static_resolver_register_overrides() {
        let mut r = StaticSymbolResolver::new();
        // Default AAPL = 265598; override.
        r.register("AAPL", xnys(), 99);
        let res = r.resolve("AAPL").unwrap();
        assert_eq!(res.contract_id, 99);
    }

    #[test]
    fn static_resolver_seeds_all_expected() {
        let r = StaticSymbolResolver::new();
        for (t, expected_cal) in [
            ("BTC-USD", CRYPTO_SPOT_ID),
            ("ETH-USD", CRYPTO_SPOT_ID),
            ("AAPL", XNYS_ID),
            ("SPY", XNYS_ID),
            ("MSFT", XNYS_ID),
            ("TSLA", XNYS_ID),
            ("NVDA", XNYS_ID),
        ] {
            let res = r.resolve(t).unwrap_or_else(|_| panic!("missing {t}"));
            assert_eq!(res.calendar.id(), expected_cal, "ticker {t}");
        }
    }

    #[test]
    fn heuristic_btc_usd_goes_to_crypto() {
        let r = HeuristicSymbolResolver::new();
        let res = r.resolve("BTC-USD").unwrap();
        assert_eq!(res.calendar.id(), CRYPTO_SPOT_ID);
    }

    #[test]
    fn heuristic_eth_usdt_goes_to_crypto() {
        let r = HeuristicSymbolResolver::new();
        let res = r.resolve("ETH-USDT").unwrap();
        assert_eq!(res.calendar.id(), CRYPTO_SPOT_ID);
    }

    #[test]
    fn heuristic_spy_goes_to_xnys() {
        let r = HeuristicSymbolResolver::new();
        let res = r.resolve("SPY").unwrap();
        assert_eq!(res.calendar.id(), XNYS_ID);
    }

    #[test]
    fn heuristic_stable_hash_is_reproducible() {
        let r = HeuristicSymbolResolver::new();
        let a = r.resolve("BTC-USD").unwrap();
        let b = r.resolve("BTC-USD").unwrap();
        assert_eq!(a.contract_id, b.contract_id);
        // Two fresh resolvers also agree — seed is the hash, not process.
        let r2 = HeuristicSymbolResolver::new();
        let c = r2.resolve("BTC-USD").unwrap();
        assert_eq!(a.contract_id, c.contract_id);
    }

    #[test]
    fn heuristic_distinct_tickers_distinct_ids() {
        // Not strictly guaranteed for a hash, but DJB2 across short
        // readable tickers inside a 1B/100M window virtually never
        // collides on our canonical set.
        let r = HeuristicSymbolResolver::new();
        let a = r.resolve("AAPL").unwrap();
        let b = r.resolve("MSFT").unwrap();
        let c = r.resolve("BTC-USD").unwrap();
        assert_ne!(a.contract_id, b.contract_id);
        assert_ne!(a.contract_id, c.contract_id);
        assert_ne!(b.contract_id, c.contract_id);
    }

    #[test]
    fn heuristic_crypto_ids_live_above_2b() {
        let r = HeuristicSymbolResolver::new();
        let c = r.resolve("BTC-USD").unwrap();
        assert!(c.contract_id >= 2_000_000_000);
    }

    #[test]
    fn heuristic_equity_ids_live_below_1b() {
        let r = HeuristicSymbolResolver::new();
        let c = r.resolve("AAPL").unwrap();
        assert!(c.contract_id < 1_000_000_000);
        assert!(c.contract_id >= 0);
    }

    #[test]
    fn heuristic_is_crypto_case_insensitive() {
        let r = HeuristicSymbolResolver::new();
        assert_eq!(r.resolve("btc-usd").unwrap().calendar.id(), CRYPTO_SPOT_ID);
        assert_eq!(r.resolve("BTC-USD").unwrap().calendar.id(), CRYPTO_SPOT_ID);
    }
}
