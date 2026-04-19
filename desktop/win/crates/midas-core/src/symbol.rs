//! Normalised ticker-symbol newtype.
//!
//! [`SymbolKey`] is the canonical representation of a traded
//! symbol across the desktop crates — a thin newtype over `String`
//! that normalises to **trimmed, uppercase ASCII** on construction.
//! That single invariant kills off a class of bugs where
//! `"AAPL"`, `"aapl"`, `" AAPL"` would silently compare unequal
//! depending on which boundary they crossed.
//!
//! Lives in `midas-core` (not `midas-app`'s
//! `annotation_store`) so every leaf crate that handles symbols can
//! depend on it without a back-edge.
//!
//! # Equality and lookups
//!
//! `Borrow<str>` is implemented over the **normalised** string, so
//! `HashMap<SymbolKey, _>::get("AAPL")` works without allocation
//! exactly when `"AAPL"` is already normalised. Callers that want a
//! case-insensitive lookup over a non-normalised input should
//! construct the key first (`SymbolKey::new(input)`).

use serde::{Deserialize, Serialize};

/// Normalised symbol identifier (trimmed + uppercase ASCII).
///
/// Serialises transparently as the inner string so on-disk TOML and
/// redb payloads stay byte-identical across releases — if `SymbolKey`
/// ever gains a second field, `#[serde(transparent)]` stops compiling
/// and we're forced to version the schema rather than silently drift.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolKey(String);

impl SymbolKey {
    /// Construct a new key by normalising `symbol` (trim → uppercase).
    /// Empty / whitespace-only input is preserved as the empty
    /// string; callers that care should reject upstream.
    pub fn new(symbol: &str) -> Self {
        Self(symbol.trim().to_uppercase())
    }

    /// Construct without normalising. Reserved for the deserialiser
    /// internal path and tests that intentionally craft
    /// non-normalised values to verify equality semantics; production
    /// code should not call this.
    #[doc(hidden)]
    pub fn from_normalised(s: String) -> Self {
        Self(s)
    }

    /// The normalised symbol string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert into the inner `String`. Useful when the symbol is
    /// being moved into a serde field that expects a bare string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::borrow::Borrow<str> for SymbolKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SymbolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SymbolKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for SymbolKey {
    fn from(s: String) -> Self {
        Self::new(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uppercases() {
        assert_eq!(SymbolKey::new("aapl").as_str(), "AAPL");
    }

    #[test]
    fn new_trims_whitespace() {
        assert_eq!(SymbolKey::new("  AAPL  ").as_str(), "AAPL");
    }

    #[test]
    fn new_trims_and_uppercases() {
        assert_eq!(SymbolKey::new(" aapl\n").as_str(), "AAPL");
    }

    #[test]
    fn equality_round_trips_through_normalisation() {
        assert_eq!(SymbolKey::new("aapl"), SymbolKey::new("AAPL"));
        assert_eq!(SymbolKey::new(" AAPL "), SymbolKey::new("aapl"));
    }

    #[test]
    fn borrow_str_enables_hashmap_lookup_without_allocation() {
        use std::collections::HashMap;
        let mut map: HashMap<SymbolKey, u32> = HashMap::new();
        map.insert(SymbolKey::new("AAPL"), 1);
        // Lookup with a plain &str (already-normalised) hits via
        // `Borrow<str>` — no `SymbolKey::new` allocation needed.
        assert_eq!(map.get("AAPL"), Some(&1));
    }

    #[test]
    fn empty_input_round_trips_to_empty_key() {
        // No upstream rejection on construction; the empty key is a
        // legal value the caller must check for itself.
        assert_eq!(SymbolKey::new("").as_str(), "");
        assert_eq!(SymbolKey::new("   ").as_str(), "");
    }

    #[test]
    fn from_str_is_normalising() {
        let k: SymbolKey = " aapl ".into();
        assert_eq!(k.as_str(), "AAPL");
    }
}
