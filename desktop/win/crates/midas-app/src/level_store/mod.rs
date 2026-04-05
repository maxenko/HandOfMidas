//! Centralized per-ticker level store.
//!
//! `LevelStore` holds all horizontal price levels keyed by ticker symbol.
//! It is owned by `MidasApp` and passed by reference through the call chain.
//! Mutations bump a per-ticker generation counter used for dirty tracking.

use std::collections::HashMap;

use midas_chart::HorizontalLevel;
use midas_core::config::LevelConfig;

// ── LevelStore ─────────────────────────────────────────────────────

/// Centralized store for horizontal price levels, keyed by ticker symbol.
///
/// Owned by `MidasApp`. Passed as `&LevelStore` for reads (compute,
/// render snapshot) and `&mut LevelStore` for writes (create, drag,
/// delete, edit).
pub struct LevelStore {
    /// Ticker symbol → levels for that ticker.
    levels: HashMap<String, Vec<HorizontalLevel>>,
    /// Per-ticker generation counter. Incremented on any mutation.
    generations: HashMap<String, u64>,
    /// Monotonically incrementing ID counter, shared across all tickers.
    next_id: u64,
}

impl LevelStore {
    /// Creates an empty level store.
    pub fn new() -> Self {
        Self {
            levels: HashMap::new(),
            generations: HashMap::new(),
            next_id: 1,
        }
    }

    // ── Queries ──────────────────────────────────────────────────

    /// Returns the levels for a ticker, or an empty slice if none exist.
    pub fn levels_for(&self, ticker: &str) -> &[HorizontalLevel] {
        self.levels.get(ticker).map_or(&[], |v| v.as_slice())
    }

    /// Returns a mutable reference to the levels for a ticker,
    /// creating an empty entry if needed.
    #[allow(dead_code)] // part of planned API
    pub fn levels_for_mut(&mut self, ticker: &str) -> &mut Vec<HorizontalLevel> {
        self.levels.entry(ticker.to_owned()).or_default()
    }

    /// Finds a level by ID across all tickers. O(n) but n is small
    /// (typically < 50 levels total). Used only for editor lookups.
    pub fn find_level(&self, id: u64) -> Option<(&str, &HorizontalLevel)> {
        for (ticker, levels) in &self.levels {
            if let Some(level) = levels.iter().find(|l| l.id == id) {
                return Some((ticker.as_str(), level));
            }
        }
        None
    }

    /// Mutable lookup by ID within a known ticker. O(n) in that
    /// ticker's levels, typically < 20.
    pub fn find_level_mut(&mut self, ticker: &str, id: u64) -> Option<&mut HorizontalLevel> {
        self.levels
            .get_mut(ticker)
            .and_then(|v| v.iter_mut().find(|l| l.id == id))
    }

    /// Returns the current generation counter for a ticker.
    #[allow(dead_code)] // part of planned API
    pub fn generation(&self, ticker: &str) -> u64 {
        self.generations.get(ticker).copied().unwrap_or(0)
    }

    // ── Mutations ─────────────────────────────────────────────────

    /// Allocates a globally unique level ID.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Adds a level to a ticker's list and bumps the generation.
    pub fn add_level(&mut self, ticker: &str, level: HorizontalLevel) {
        self.levels
            .entry(ticker.to_owned())
            .or_default()
            .push(level);
        self.bump_generation(ticker);
    }

    /// Removes a level by ID from a ticker's list. Returns the removed
    /// level, or `None` if not found.
    pub fn remove_level(&mut self, ticker: &str, id: u64) -> Option<HorizontalLevel> {
        let levels = self.levels.get_mut(ticker)?;
        let idx = levels.iter().position(|l| l.id == id)?;
        let removed = levels.remove(idx);
        self.bump_generation(ticker);
        Some(removed)
    }

    /// Removes all levels for a ticker.
    pub fn clear_levels(&mut self, ticker: &str) {
        if let Some(levels) = self.levels.get_mut(ticker) {
            if !levels.is_empty() {
                levels.clear();
                self.bump_generation(ticker);
            }
        }
    }

    // ── Persistence ──────────────────────────────────────────────

    /// Reconstructs a `LevelStore` from persisted config.
    pub fn from_config(levels: &HashMap<String, Vec<LevelConfig>>) -> Self {
        let mut store = Self::new();
        for (ticker, cfgs) in levels {
            let mut ticker_levels = Vec::with_capacity(cfgs.len());
            for cfg in cfgs {
                let id = store.alloc_id();
                ticker_levels.push(HorizontalLevel {
                    id,
                    price: cfg.price,
                    color: cfg.color,
                    line_width: cfg.line_width,
                    label: cfg.label.clone(),
                    icon: midas_chart::LevelIcon::from_str_id(&cfg.icon),
                    locked: cfg.locked,
                });
            }
            store.levels.insert(ticker.clone(), ticker_levels);
        }
        store
    }

    /// Serializes to a config-ready map.
    pub fn to_config(&self) -> HashMap<String, Vec<LevelConfig>> {
        let mut out = HashMap::new();
        for (ticker, levels) in &self.levels {
            if levels.is_empty() {
                continue;
            }
            let cfgs: Vec<LevelConfig> = levels
                .iter()
                .map(|l| LevelConfig {
                    price: l.price,
                    color: l.color,
                    line_width: l.line_width,
                    label: l.label.clone(),
                    icon: l.icon.to_str_id().to_owned(),
                    locked: l.locked,
                })
                .collect();
            out.insert(ticker.clone(), cfgs);
        }
        out
    }

    // ── Internal ────────────────────────────────────────────────

    /// Bumps the generation counter for a ticker.
    fn bump_generation(&mut self, ticker: &str) {
        let gen = self.generations.entry(ticker.to_owned()).or_insert(0);
        *gen += 1;
    }
}

#[cfg(test)]
mod tests;
