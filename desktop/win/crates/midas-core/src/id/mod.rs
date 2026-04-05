//! Newtype wrappers for domain IDs.
//!
//! Each ID is a thin wrapper around an integer, providing type safety so that
//! a `ChartId` cannot accidentally be used where a `SymbolId` is expected.

use std::fmt;

/// Unique identifier for a chart panel within the workspace layout.
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct ChartId(pub u32);

/// Unique identifier for a pane (a slot in the binary split tree layout).
///
/// Uses `u64` to accommodate composite IDs or high-throughput allocation.
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct PaneId(pub u64);

/// Unique identifier for a traded symbol (e.g., AAPL, MSFT).
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct SymbolId(pub u32);

/// Unique identifier for a watchlist panel within the workspace layout.
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct WatchlistId(pub u32);

/// Unique identifier for an order panel within the workspace layout.
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct OrderPanelId(pub u32);

impl ChartId {
    /// Create a new `ChartId` from a raw `u32` value.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl PaneId {
    /// Create a new `PaneId` from a raw `u64` value.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

impl SymbolId {
    /// Create a new `SymbolId` from a raw `u32` value.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl WatchlistId {
    /// Create a new `WatchlistId` from a raw `u32` value.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl OrderPanelId {
    /// Create a new `OrderPanelId` from a raw `u32` value.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Display for ChartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Chart({})", self.0)
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pane({})", self.0)
    }
}

impl fmt::Display for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Symbol({})", self.0)
    }
}

impl fmt::Display for WatchlistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Watchlist({})", self.0)
    }
}

impl fmt::Display for OrderPanelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Order({})", self.0)
    }
}

#[cfg(test)]
mod tests;
