//! Contract-cache helper (NM-1).
//!
//! First-subscribe paths need `con_id` before calling the upstream
//! `subscribe_*` methods. Resolving takes a round-trip (reqContractDetails),
//! so the router memoises the result on `RouterState.contract_cache` and
//! re-uses it for the lifetime of the router.
//!
//! This module is intentionally thin — the inline actor uses the same
//! logic — but keeping the helper as a standalone surface makes the
//! `MarketDataRouter::resolve_or_cached` public method a one-liner that
//! tests can exercise directly.

use midas_broker_core::market_data::{ContractDetails, MarketDataError, SecurityType, SymbolKey};

use super::state::RouterState;

/// Resolve a symbol to [`ContractDetails`], hitting the cache first.
///
/// Default `(SecurityType::Stock, "SMART")` covers US equities — the
/// primary router-target universe for S5/S6/S7. Non-stock contracts
/// will land with a contract-builder surface in a later slice.
pub(crate) async fn resolve_or_cached(
    state: &RouterState,
    sym: &SymbolKey,
) -> Result<ContractDetails, MarketDataError> {
    if let Some(c) = state.contract_cache.get(sym) {
        return Ok(c.clone());
    }
    let details = state
        .source
        .resolve_contract(sym, SecurityType::Stock, "SMART")
        .await?;
    state.contract_cache.insert(sym.clone(), details.clone());
    Ok(details)
}
