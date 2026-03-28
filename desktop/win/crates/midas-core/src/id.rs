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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn chart_id_equality() {
        assert_eq!(ChartId::new(1), ChartId::new(1));
        assert_ne!(ChartId::new(1), ChartId::new(2));
    }

    #[test]
    fn pane_id_equality() {
        assert_eq!(PaneId::new(10), PaneId::new(10));
        assert_ne!(PaneId::new(10), PaneId::new(20));
    }

    #[test]
    fn symbol_id_equality() {
        assert_eq!(SymbolId::new(42), SymbolId::new(42));
        assert_ne!(SymbolId::new(42), SymbolId::new(43));
    }

    #[test]
    fn chart_id_hash() {
        let mut set = HashSet::new();
        set.insert(ChartId::new(1));
        set.insert(ChartId::new(2));
        set.insert(ChartId::new(1)); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn pane_id_hash() {
        let mut set = HashSet::new();
        set.insert(PaneId::new(100));
        set.insert(PaneId::new(200));
        set.insert(PaneId::new(100)); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn symbol_id_hash() {
        let mut set = HashSet::new();
        set.insert(SymbolId::new(5));
        set.insert(SymbolId::new(10));
        set.insert(SymbolId::new(5)); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn chart_id_display() {
        assert_eq!(ChartId::new(7).to_string(), "Chart(7)");
    }

    #[test]
    fn pane_id_display() {
        assert_eq!(PaneId::new(42).to_string(), "Pane(42)");
    }

    #[test]
    fn symbol_id_display() {
        assert_eq!(SymbolId::new(99).to_string(), "Symbol(99)");
    }

    #[test]
    fn ordering() {
        assert!(ChartId::new(1) < ChartId::new(2));
        assert!(PaneId::new(10) < PaneId::new(20));
        assert!(SymbolId::new(0) < SymbolId::new(1));
    }

    #[test]
    fn copy_semantics() {
        let a = ChartId::new(5);
        let b = a; // Copy
        assert_eq!(a, b); // `a` is still valid
    }

    #[test]
    fn serde_roundtrip() {
        let id = ChartId::new(42);
        let json = serde_json::to_string(&id).unwrap();
        let back: ChartId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);

        let pid = PaneId::new(999);
        let json = serde_json::to_string(&pid).unwrap();
        let back: PaneId = serde_json::from_str(&json).unwrap();
        assert_eq!(pid, back);

        let sid = SymbolId::new(7);
        let json = serde_json::to_string(&sid).unwrap();
        let back: SymbolId = serde_json::from_str(&json).unwrap();
        assert_eq!(sid, back);
    }
}
