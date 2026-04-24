//! Centralized per-symbol annotation store.
//!
//! `AnnotationStore` holds all user-drawn annotations keyed by symbol.
//! It is the single source of truth for chart annotations — including
//! horizontal price levels (`AnnotationKind::Level`) and order brackets
//! (`AnnotationKind::OrderBracket`). Owned by `MidasApp`, passed by
//! reference during scene computation.
//!
//! ## Chart-transition slice 8.5 status
//!
//! The `midas_chart::widget::*` imports below (`Annotation`,
//! `AnnotationId`, `AnnotationKind`, `PriceLine`, `HorizontalLevel`,
//! `LineStyle`, `Presence`, `LineExtent`, `LineStroke`) are the shared
//! **persistent annotation data model** — per plan D9, the
//! `AnnotationStore` format is unchanged across the migration. Session-
//! chart consumers read from this store via scene-native projections
//! (`Vec<midas_scene::layers::LevelView>`); they never handle these
//! types directly. These imports migrate in slice 9c's atomic
//! deletion PR when the `midas-chart` crate is retired and the types
//! move to their new home.
//!
//! All mutations bump a generation counter for dirty tracking.
//!
//! ## Level helpers
//!
//! Horizontal price levels used to live in a separate `LevelStore`
//! (audit P2b). That store has been retired: levels are now stored as
//! [`AnnotationKind::Level`] annotations with the `locked` flag on the
//! [`Annotation`] wrapper. The [`StoredLevel`] type is kept as a
//! read-only projection so call sites that want "the old-style level
//! value" (geometry + locked sibling) still compile; writes go through
//! the level helper methods below ([`AnnotationStore::add_level`],
//! [`AnnotationStore::update_level`], [`AnnotationStore::remove_level`],
//! [`AnnotationStore::clear_levels`]).
//!
//! The on-disk config format is unchanged: [`AnnotationStore::from_level_configs`]
//! and [`AnnotationStore::to_level_configs`] round-trip the same
//! `AppConfig.levels: HashMap<String, Vec<LevelConfig>>` shape that
//! `LevelStore` used, so existing `data/config.toml` files load
//! byte-identically.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use midas_chart::widget::price_line::{LineExtent, LineStroke, PriceLine};
use midas_chart::widget::{Annotation, AnnotationId, AnnotationKind, LineStyle, Presence};
use midas_chart::HorizontalLevel;
use midas_core::config::LevelConfig;
use midas_core::Timeframe;

// ── SymbolKey ──────────────────────────────────────────────────────

/// Re-exported from [`midas_core::SymbolKey`] for callsite ergonomics
/// — every `crate::annotation_store::SymbolKey` import in the app
/// keeps working unchanged. New code should import from `midas_core`
/// directly. Construction now also trims (in addition to
/// uppercasing); see the canonical type for the full contract.
pub use midas_core::SymbolKey;

// ── StoredLevel ────────────────────────────────────────────────────

/// A `HorizontalLevel` plus its `locked` flag, projected out of
/// `AnnotationStore` for read-only consumers (chart snapshot, level
/// editor popup).
///
/// Levels themselves live in [`AnnotationStore`] as
/// [`AnnotationKind::Level`] annotations with `locked` on the
/// [`Annotation`] wrapper. `StoredLevel` is the shape the old
/// `LevelStore` exposed; kept as a view type so existing call sites
/// that read `.line`, `.label`, `.icon`, `.locked` still compile.
/// Deref/DerefMut target [`HorizontalLevel`] so field access like
/// `.price`, `.line`, `.label` works through the wrapper.
#[derive(Clone, Debug)]
pub struct StoredLevel {
    /// The underlying level data (id + geometry + labels).
    pub level: HorizontalLevel,
    /// Whether this level is locked (prevents drag and delete).
    pub locked: bool,
}

impl Deref for StoredLevel {
    type Target = HorizontalLevel;
    fn deref(&self) -> &HorizontalLevel {
        &self.level
    }
}

impl DerefMut for StoredLevel {
    fn deref_mut(&mut self) -> &mut HorizontalLevel {
        &mut self.level
    }
}

impl StoredLevel {
    /// Project an [`Annotation`] into a [`StoredLevel`] if and only if
    /// its kind is [`AnnotationKind::Level`]. Returns `None` for other
    /// annotation kinds.
    fn from_annotation(ann: &Annotation) -> Option<Self> {
        match &ann.kind {
            AnnotationKind::Level(level) => Some(Self {
                level: level.clone(),
                locked: ann.locked,
            }),
            _ => None,
        }
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
    #[allow(dead_code)]
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

    // ── Level helpers (post-P2b: replaces LevelStore) ────────────

    /// All horizontal levels for a symbol, projected from the stored
    /// annotations. Returns an empty vec if no levels exist.
    ///
    /// This is the read path that parallels the old
    /// `LevelStore::levels_for` — callers use it to render levels, hit-
    /// test them, or serialize them to config.
    pub fn levels_for(&self, symbol: &str) -> Vec<StoredLevel> {
        self.get(symbol)
            .iter()
            .filter_map(StoredLevel::from_annotation)
            .collect()
    }

    /// Whether any symbol has at least one level annotation. Used by
    /// the persistence layer's cheap "is there anything to save?" check.
    #[allow(dead_code)] // part of planned API
    pub fn has_any_levels(&self) -> bool {
        self.by_symbol.values().any(|sa| {
            sa.annotations
                .iter()
                .any(|a| matches!(a.kind, AnnotationKind::Level(_)))
        })
    }

    /// Find a level annotation by its numeric id (the `HorizontalLevel.id`
    /// field — the old u64 used by `LevelStore` and still carried on
    /// `HorizontalLevel`). Returns the owning symbol plus a projected
    /// [`StoredLevel`]. Used by the level-editor lookup path.
    pub fn find_level(&self, level_id: u64) -> Option<(String, StoredLevel)> {
        for (key, sa) in &self.by_symbol {
            for ann in &sa.annotations {
                if let AnnotationKind::Level(ref level) = ann.kind {
                    if level.id == level_id {
                        return Some((
                            key.as_str().to_owned(),
                            StoredLevel {
                                level: level.clone(),
                                locked: ann.locked,
                            },
                        ));
                    }
                }
            }
        }
        None
    }

    /// Add a pre-built level (geometry + locked) under a symbol.
    ///
    /// The level's inner `id` comes from [`AnnotationStore::alloc_level_id`]
    /// (or an equivalent caller-allocated id). The resulting annotation
    /// takes that same id for its `AnnotationId` so the two ID spaces
    /// stay unified. Returns the assigned [`AnnotationId`].
    pub fn add_level(&mut self, symbol: &str, entry: StoredLevel) -> AnnotationId {
        let level_id = entry.level.id;
        let ann_id = AnnotationId(level_id);
        // Keep `next_id` ahead of any hand-allocated ids so future
        // `add()` calls don't collide.
        if level_id >= self.next_id {
            self.next_id = level_id + 1;
        }

        let now = epoch_millis();
        let annotation = Annotation {
            id: ann_id,
            kind: AnnotationKind::Level(entry.level),
            presence: Presence::Active,
            visible_timeframes: None,
            locked: entry.locked,
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
        ann_id
    }

    /// Allocate a fresh level id from the shared annotation-id counter.
    ///
    /// Matches the old `LevelStore::alloc_id` API; the returned u64 is
    /// suitable as `HorizontalLevel.id`.
    pub fn alloc_level_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Remove a level by its numeric id under a symbol. Returns `true`
    /// if the level was found and removed.
    pub fn remove_level(&mut self, symbol: &str, level_id: u64) -> bool {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return false;
        };
        let idx = sa.annotations.iter().position(|a| match &a.kind {
            AnnotationKind::Level(level) => level.id == level_id,
            _ => false,
        });
        let Some(idx) = idx else {
            return false;
        };
        sa.annotations.remove(idx);
        self.bump_generation(&key);
        true
    }

    /// Remove all levels for a symbol. Other annotation kinds (brackets
    /// etc.) under the same symbol are preserved.
    pub fn clear_levels(&mut self, symbol: &str) {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return;
        };
        let before = sa.annotations.len();
        sa.annotations
            .retain(|a| !matches!(a.kind, AnnotationKind::Level(_)));
        if sa.annotations.len() != before {
            self.bump_generation(&key);
        }
    }

    /// Update a level annotation in place via a closure.
    ///
    /// Passes the caller both the inner [`HorizontalLevel`] geometry
    /// and the wrapper `locked` flag as mutable borrows; one-call
    /// mutation is enough for every editor path (price, label, icon,
    /// color, thickness, lock toggle).
    ///
    /// Returns `true` if the level was found.
    pub fn update_level(
        &mut self,
        symbol: &str,
        level_id: u64,
        f: impl FnOnce(&mut HorizontalLevel, &mut bool),
    ) -> bool {
        let key = SymbolKey::new(symbol);
        let Some(sa) = self.by_symbol.get_mut(&key) else {
            return false;
        };
        let Some(ann) = sa.annotations.iter_mut().find(|a| match &a.kind {
            AnnotationKind::Level(level) => level.id == level_id,
            _ => false,
        }) else {
            return false;
        };
        if let AnnotationKind::Level(ref mut level) = ann.kind {
            ann.modified_at = epoch_millis();
            f(level, &mut ann.locked);
        } else {
            // position() already ruled this branch out — unreachable.
            return false;
        }
        self.bump_generation(&key);
        true
    }

    // ── Level persistence (level_configs round-trip) ─────────────

    /// Populate the store from the flat `AppConfig.levels` map used by
    /// `LevelStore` v1. The on-disk schema is unchanged; every
    /// [`LevelConfig`] becomes a fresh [`AnnotationKind::Level`]
    /// annotation with a freshly-allocated id.
    ///
    /// Does NOT clear existing annotations — callers are expected to
    /// invoke this on a fresh store at startup.
    pub fn import_level_configs(&mut self, cfgs: &HashMap<String, Vec<LevelConfig>>) {
        for (symbol, level_cfgs) in cfgs {
            for cfg in level_cfgs {
                let id = self.alloc_level_id();
                let level = HorizontalLevel {
                    id,
                    line: PriceLine {
                        price: cfg.price,
                        extent: LineExtent::default(),
                        stroke: LineStroke {
                            color: cfg.color,
                            width: cfg.line_width,
                            style: LineStyle::default(),
                        },
                    },
                    label: cfg.label.clone(),
                    icon: midas_chart::LevelIcon::from_str_id(&cfg.icon),
                };
                self.add_level(
                    symbol,
                    StoredLevel {
                        level,
                        locked: cfg.locked,
                    },
                );
            }
        }
    }

    /// Serialize every [`AnnotationKind::Level`] annotation back into
    /// the flat `AppConfig.levels` map shape. Non-level annotations
    /// (brackets etc.) are skipped — they persist through their own
    /// paths.
    pub fn to_level_configs(&self) -> HashMap<String, Vec<LevelConfig>> {
        let mut out = HashMap::new();
        for (key, sa) in &self.by_symbol {
            let cfgs: Vec<LevelConfig> = sa
                .annotations
                .iter()
                .filter_map(|ann| match &ann.kind {
                    AnnotationKind::Level(level) => Some(LevelConfig {
                        price: level.line.price,
                        color: level.line.stroke.color,
                        line_width: level.line.stroke.width,
                        label: level.label.clone(),
                        icon: level.icon.to_str_id().to_owned(),
                        locked: ann.locked,
                    }),
                    _ => None,
                })
                .collect();
            if !cfgs.is_empty() {
                out.insert(key.as_str().to_owned(), cfgs);
            }
        }
        out
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
