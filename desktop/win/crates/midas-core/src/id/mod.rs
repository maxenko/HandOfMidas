//! Newtype wrappers for domain IDs.
//!
//! Each ID is a thin wrapper around an integer, providing type safety so that
//! a `ChartId` cannot accidentally be used where a `SymbolId` is expected.

use std::fmt;

/// Defines one or more newtype ID wrappers with a `const fn new()` constructor
/// and a human-readable `Display` impl.
///
/// # Example
///
/// ```ignore
/// define_id! {
///     ChartId(u32) => "Chart",
/// }
/// assert_eq!(ChartId::new(7).to_string(), "Chart(7)");
/// ```
macro_rules! define_id {
    ($( $(#[$meta:meta])* $Name:ident($inner:ty) => $label:literal ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(
                Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd,
                serde::Serialize, serde::Deserialize,
            )]
            pub struct $Name(pub $inner);

            impl $Name {
                /// Create a new ID from a raw integer value.
                pub const fn new(id: $inner) -> Self {
                    Self(id)
                }
            }

            impl fmt::Display for $Name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, concat!($label, "({})"), self.0)
                }
            }
        )+
    };
}

define_id! {
    /// Unique identifier for a chart panel within the workspace layout.
    ChartId(u32)      => "Chart",
    /// Unique identifier for a pane (a slot in the binary split tree layout).
    ///
    /// Uses `u64` to accommodate composite IDs or high-throughput allocation.
    PaneId(u64)       => "Pane",
    /// Unique identifier for a traded symbol (e.g., AAPL, MSFT).
    SymbolId(u32)     => "Symbol",
    /// Unique identifier for a watchlist panel within the workspace layout.
    WatchlistId(u32)  => "Watchlist",
    /// Unique identifier for an order panel within the workspace layout.
    OrderPanelId(u32) => "Order",
}

#[cfg(test)]
mod tests;
