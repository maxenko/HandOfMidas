//! Centralized per-symbol annotation store.
//!
//! `AnnotationStore` holds all user-drawn annotations keyed by symbol.
//! It replaces `LevelStore` as the single source of truth for chart
//! annotations. Owned by `MidasApp`, passed by reference during
//! scene computation.
//!
//! All mutations bump a generation counter for dirty tracking.

use std::collections::HashMap;

use midas_chart::widget::{Annotation, AnnotationId, AnnotationKind, Presence};
use midas_core::Timeframe;
use serde::{Deserialize, Serialize};

// ── SymbolKey ──────────────────────────────────────────────────────

/// Interned symbol key for annotation storage lookups.
///
/// A thin newtype over `String` to prevent mixing up symbol strings
/// with other strings. Normalizes to uppercase on construction.
/// Implements `Borrow<str>` so `HashMap::get("AAPL")` works without
/// allocating.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey(String);

impl SymbolKey {
    /// Create a new symbol key, normalizing to uppercase.
    pub fn new(symbol: &str) -> Self {
        Self(symbol.to_uppercase())
    }

    /// The normalized symbol string.
    pub fn as_str(&self) -> &str {
        &self.0
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

// ── SymbolAnnotations ──────────────────────────────────────────────

/// All annotations for a single symbol.
struct SymbolAnnotations {
    /// The annotations themselves. Linear scan is fine for n < 100.
    annotations: Vec<Annotation>,
    /// Per-symbol generation counter.
    generation: u64,
}

impl SymbolAnnotations {
    fn new() -> Self {
        Self {
            annotations: Vec::new(),
            generation: 0,
        }
    }
}

// ── AnnotationStore ────────────────────────────────────────────────

/// Centralized annotation storage, owned by `MidasApp`.
///
/// All charts read from this store during scene computation. Mutations
/// happen exclusively in the iced `update()` phase.
pub struct AnnotationStore {
    /// Per-symbol annotation collections.
    by_symbol: HashMap<SymbolKey, SymbolAnnotations>,
    /// Global generation counter. Incremented on ANY mutation to ANY
    /// symbol.
    global_generation: u64,
    /// Monotonically increasing ID counter, shared across all symbols.
    next_id: u64,
}

impl AnnotationStore {
    /// Creates an empty store with no annotations.
    pub fn new() -> Self {
        Self {
            by_symbol: HashMap::new(),
            global_generation: 0,
            next_id: 1,
        }
    }

    // ── Queries ──────────────────────────────────────────────────

    /// Returns all annotations for a symbol, or an empty slice if none.
    ///
    /// The symbol is normalized to uppercase, so `get("aapl")` and
    /// `get("AAPL")` return the same result.
    pub fn get(&self, symbol: &str) -> &[Annotation] {
        let key = SymbolKey::new(symbol);
        self.by_symbol
            .get(&key)
            .map_or(&[], |sa| sa.annotations.as_slice())
    }

    /// Returns annotations visible on a specific timeframe.
    #[allow(dead_code)] // part of planned API
    pub fn get_visible(&self, symbol: &str, timeframe: Timeframe) -> Vec<&Annotation> {
        self.get(symbol)
            .iter()
            .filter(|ann| ann.should_render_on(timeframe))
            .collect()
    }

    /// Returns the generation counter for a symbol.
    #[allow(dead_code)] // part of planned API
    pub fn generation(&self, symbol: &str) -> u64 {
        let key = SymbolKey::new(symbol);
        self.by_symbol.get(&key).map_or(0, |sa| sa.generation)
    }

    /// Returns the global generation counter.
    #[allow(dead_code)] // part of planned API
    pub fn global_generation(&self) -> u64 {
        self.global_generation
    }

    /// Returns an annotation by ID within a symbol, or `None`.
    pub fn get_by_id(&self, symbol: &str, id: AnnotationId) -> Option<&Annotation> {
        self.get(symbol).iter().find(|a| a.id == id)
    }

    /// Returns the `OrderBracket` data for an annotation, or `None` if the
    /// annotation doesn't exist or isn't a bracket.
    pub fn get_bracket(
        &self,
        symbol: &str,
        id: AnnotationId,
    ) -> Option<&midas_chart::widget::order_bracket::OrderBracket> {
        self.get_by_id(symbol, id).and_then(|a| match &a.kind {
            AnnotationKind::OrderBracket(b) => Some(b.as_ref()),
            _ => None,
        })
    }

    /// Finds an annotation by ID across all symbols.
    #[allow(dead_code)] // part of planned API
    pub fn find(&self, id: AnnotationId) -> Option<(&str, &Annotation)> {
        for (key, sa) in &self.by_symbol {
            if let Some(ann) = sa.annotations.iter().find(|a| a.id == id) {
                return Some((key.as_str(), ann));
            }
        }
        None
    }

    // ── Mutations ────────────────────────────────────────────────

    /// Adds an annotation to a symbol. Returns the assigned ID.
    pub fn add(&mut self, symbol: &str, kind: AnnotationKind) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;

        let now = epoch_millis();
        let annotation = Annotation {
            id,
            kind,
            presence: Presence::Active,
            visible_timeframes: None,
            locked: false,
            created_at: now,
            modified_at: now,
        };

        let key = SymbolKey::new(symbol);
        self.by_symbol
            .entry(key.clone())
            .or_insert_with(SymbolAnnotations::new)
            .annotations
            .push(annotation);

        self.bump_generation(&key);
        id
    }

    /// Removes an annotation by ID from a symbol.
    /// Returns `true` if the annotation was found and removed.
    #[allow(dead_code)] // part of planned API
    pub fn remove(&mut self, symbol: &str, id: AnnotationId) -> bool {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return false;
        };
        let Some(idx) = sa.annotations.iter().position(|a| a.id == id) else {
            return false;
        };
        sa.annotations.remove(idx);
        self.bump_generation(&key);
        true
    }

    /// Updates an annotation in place via a closure.
    ///
    /// The generation counter is bumped unconditionally.
    /// Returns `true` if the annotation was found.
    pub fn update(
        &mut self,
        symbol: &str,
        id: AnnotationId,
        f: impl FnOnce(&mut Annotation),
    ) -> bool {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return false;
        };
        let Some(ann) = sa.annotations.iter_mut().find(|a| a.id == id) else {
            return false;
        };
        ann.modified_at = epoch_millis();
        f(ann);
        self.bump_generation(&key);
        true
    }

    /// Removes all annotations for a symbol.
    #[allow(dead_code)] // part of planned API
    pub fn clear(&mut self, symbol: &str) {
        let key = SymbolKey::new(symbol);
        if let Some(sa) = self.by_symbol.get_mut(&key) {
            if !sa.annotations.is_empty() {
                sa.annotations.clear();
                self.bump_generation(&key);
            }
        }
    }

    /// Removes annotations matching a predicate for a symbol.
    /// Returns the number of annotations removed.
    #[allow(dead_code)] // part of planned API
    pub fn retain(&mut self, symbol: &str, pred: impl FnMut(&Annotation) -> bool) -> usize {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return 0;
        };
        let before = sa.annotations.len();
        sa.annotations.retain(pred);
        let removed = before - sa.annotations.len();
        if removed > 0 {
            self.bump_generation(&key);
        }
        removed
    }

    /// Returns an iterator over all symbols that have annotations.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.by_symbol.keys().map(|k| k.as_str())
    }

    /// Add a pre-built annotation (used when loading from disk).
    /// Does NOT assign a new ID -- uses the annotation's existing ID.
    pub fn add_raw(&mut self, symbol: &str, annotation: Annotation) {
        let key = SymbolKey::new(symbol);
        self.by_symbol
            .entry(key)
            .or_insert_with(SymbolAnnotations::new)
            .annotations
            .push(annotation);
        // Don't bump generation on load — caller handles that.
    }

    /// Override the next-ID counter (used after loading from disk).
    pub fn set_next_id(&mut self, next_id: u64) {
        self.next_id = next_id;
    }

    // ── Internal ─────────────────────────────────────────────────

    fn bump_generation(&mut self, key: &SymbolKey) {
        if let Some(sa) = self.by_symbol.get_mut(key) {
            sa.generation += 1;
        }
        self.global_generation += 1;
    }
}

impl Default for AnnotationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Current time as epoch milliseconds.
fn epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
