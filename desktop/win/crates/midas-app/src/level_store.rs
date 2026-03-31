//! Centralized per-ticker level store.
//!
//! `LevelStore` holds all horizontal price levels keyed by ticker symbol.
//! It is owned by `MidasApp` and passed by reference through the call chain.
//! Mutations bump a per-ticker generation counter used for dirty tracking.

use std::collections::HashMap;

use midas_chart::HorizontalLevel;
use midas_core::config::LevelConfig;

// ── LevelStore ───���────────────────────────��─────────────────────────

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
    pub fn generation(&self, ticker: &str) -> u64 {
        self.generations.get(ticker).copied().unwrap_or(0)
    }

    // ── Mutations ────────────────────────��───────────────────────

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

    // ── Internal ───────────���─────────────────────────────────────

    /// Bumps the generation counter for a ticker.
    fn bump_generation(&mut self, ticker: &str) {
        let gen = self.generations.entry(ticker.to_owned()).or_insert(0);
        *gen += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_chart::LevelIcon;

    fn make_level(id: u64, price: f64) -> HorizontalLevel {
        HorizontalLevel {
            id,
            price,
            color: [0.85, 0.85, 0.85, 0.8],
            line_width: 1.0,
            label: None,
            icon: LevelIcon::None,
            locked: false,
        }
    }

    #[test]
    fn new_store_is_empty() {
        let store = LevelStore::new();
        assert!(store.levels_for("AAPL").is_empty());
        assert_eq!(store.generation("AAPL"), 0);
    }

    #[test]
    fn alloc_id_is_monotonic() {
        let mut store = LevelStore::new();
        let a = store.alloc_id();
        let b = store.alloc_id();
        let c = store.alloc_id();
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(c, 3);
    }

    #[test]
    fn add_and_query_levels() {
        let mut store = LevelStore::new();
        let id = store.alloc_id();
        store.add_level("AAPL", make_level(id, 185.50));

        let levels = store.levels_for("AAPL");
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].price, 185.50);
        assert!(store.levels_for("MSFT").is_empty());
    }

    #[test]
    fn add_level_bumps_generation() {
        let mut store = LevelStore::new();
        assert_eq!(store.generation("AAPL"), 0);

        store.add_level("AAPL", make_level(1, 100.0));
        assert_eq!(store.generation("AAPL"), 1);

        store.add_level("AAPL", make_level(2, 200.0));
        assert_eq!(store.generation("AAPL"), 2);

        // Other tickers unaffected.
        assert_eq!(store.generation("MSFT"), 0);
    }

    #[test]
    fn remove_level_returns_removed() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        store.add_level("AAPL", make_level(2, 200.0));
        let gen_before = store.generation("AAPL");

        let removed = store.remove_level("AAPL", 1);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().price, 100.0);
        assert_eq!(store.levels_for("AAPL").len(), 1);
        assert!(store.generation("AAPL") > gen_before);
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        let gen_before = store.generation("AAPL");

        assert!(store.remove_level("AAPL", 999).is_none());
        assert!(store.remove_level("MSFT", 1).is_none());
        // Generation should not change for failed removes.
        assert_eq!(store.generation("AAPL"), gen_before);
    }

    #[test]
    fn clear_levels_empties_ticker() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        store.add_level("AAPL", make_level(2, 200.0));
        let gen_before = store.generation("AAPL");

        store.clear_levels("AAPL");
        assert!(store.levels_for("AAPL").is_empty());
        assert!(store.generation("AAPL") > gen_before);
    }

    #[test]
    fn clear_empty_ticker_is_noop() {
        let mut store = LevelStore::new();
        store.clear_levels("AAPL");
        assert_eq!(store.generation("AAPL"), 0);
    }

    #[test]
    fn find_level_across_tickers() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        store.add_level("MSFT", make_level(2, 400.0));

        let (ticker, level) = store.find_level(2).unwrap();
        assert_eq!(ticker, "MSFT");
        assert_eq!(level.price, 400.0);

        assert!(store.find_level(999).is_none());
    }

    #[test]
    fn find_level_mut_within_ticker() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));

        let level = store.find_level_mut("AAPL", 1).unwrap();
        level.price = 150.0;

        assert_eq!(store.levels_for("AAPL")[0].price, 150.0);
    }

    #[test]
    fn find_level_mut_wrong_ticker_returns_none() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        assert!(store.find_level_mut("MSFT", 1).is_none());
    }

    #[test]
    fn config_round_trip() {
        let mut store = LevelStore::new();
        let id1 = store.alloc_id();
        let mut level1 = make_level(id1, 185.50);
        level1.label = Some("Support".into());
        level1.icon = LevelIcon::ArrowUp;
        level1.locked = true;
        store.add_level("AAPL", level1);

        let id2 = store.alloc_id();
        store.add_level("MSFT", make_level(id2, 420.0));

        // Serialize and reconstruct.
        let config = store.to_config();
        assert_eq!(config.len(), 2);
        assert_eq!(config["AAPL"].len(), 1);
        assert_eq!(config["AAPL"][0].price, 185.50);
        assert_eq!(config["AAPL"][0].label.as_deref(), Some("Support"));
        assert_eq!(config["AAPL"][0].icon, "arrow_up");
        assert!(config["AAPL"][0].locked);

        let restored = LevelStore::from_config(&config);
        assert_eq!(restored.levels_for("AAPL").len(), 1);
        assert_eq!(restored.levels_for("AAPL")[0].price, 185.50);
        assert_eq!(
            restored.levels_for("AAPL")[0].label.as_deref(),
            Some("Support")
        );
        assert_eq!(restored.levels_for("AAPL")[0].icon, LevelIcon::ArrowUp);
        assert!(restored.levels_for("AAPL")[0].locked);
        assert_eq!(restored.levels_for("MSFT").len(), 1);
        assert_eq!(restored.levels_for("MSFT")[0].price, 420.0);
    }

    #[test]
    fn config_round_trip_empty() {
        let config: HashMap<String, Vec<LevelConfig>> = HashMap::new();
        let store = LevelStore::from_config(&config);
        assert!(store.levels_for("AAPL").is_empty());
        assert!(store.to_config().is_empty());
    }

    #[test]
    fn empty_ticker_not_serialized() {
        let mut store = LevelStore::new();
        store.add_level("AAPL", make_level(1, 100.0));
        store.clear_levels("AAPL");
        // Empty ticker should not appear in config output.
        assert!(store.to_config().is_empty());
    }

    #[test]
    fn levels_for_mut_creates_entry() {
        let mut store = LevelStore::new();
        let v = store.levels_for_mut("AAPL");
        assert!(v.is_empty());
        v.push(make_level(1, 100.0));
        assert_eq!(store.levels_for("AAPL").len(), 1);
    }

    #[test]
    fn ids_unique_across_tickers_after_from_config() {
        let mut config: HashMap<String, Vec<LevelConfig>> = HashMap::new();
        config.insert(
            "AAPL".into(),
            vec![LevelConfig {
                price: 100.0,
                color: [1.0, 0.0, 0.0, 1.0],
                line_width: 1.0,
                label: None,
                icon: "none".into(),
                locked: false,
            }],
        );
        config.insert(
            "MSFT".into(),
            vec![LevelConfig {
                price: 400.0,
                color: [0.0, 1.0, 0.0, 1.0],
                line_width: 1.0,
                label: None,
                icon: "none".into(),
                locked: false,
            }],
        );
        let store = LevelStore::from_config(&config);
        let aapl_id = store.levels_for("AAPL")[0].id;
        let msft_id = store.levels_for("MSFT")[0].id;
        assert_ne!(aapl_id, msft_id);
    }
}
