//! Historical-data provider registry (router-refactor slice 10i).
//!
//! **OPEN (post-refactor):** this registry wraps the pre-router
//! [`DataProvider`] trait and is the last surviving consumer of that
//! trait. Every streaming / live code path migrated to
//! [`MarketDataSource`](midas_broker::MarketDataSource) via the
//! router (`app.rs::load_chart_with`, `load_market_snapshot`,
//! `spawn_thumbnail_load`, `drain_thumbnail_queue`,
//! `load_all_thumbnails`, `persistence.rs`). A future cleanup pass can
//! replace `DataProvider::get_candles` calls with
//! `MarketDataSource::historical_bars` + a translation step from
//! `Vec<Bar>` to `CandleBuffer`; the signatures don't align 1:1, so
//! the migration was deferred out of this slice's scope.
//!
//! Owned by `MidasApp` (single iced update thread). Providers are
//! `Arc`'d so they can be cloned into `Task::perform` closures for
//! async operations.

use std::sync::Arc;

use midas_core::provider::DataProvider;

/// Manages available historical-data providers.
///
/// Holds a list of registered providers and tracks which one is active.
/// Provider switching triggers chart reloads in the app layer.
///
/// After initialization (at least one `register_data_provider` call),
/// `active_data_idx` is a valid index into `data_providers`. Before
/// any provider is registered, `active_data_provider()` returns
/// `None`.
///
/// Renamed from `ProviderRegistry` in router-refactor slice 10i once
/// the order-broker side of the registry was retired with the sim /
/// test broker adapters.
pub struct HistoricalDataRegistry {
    /// Registered data providers, in display order.
    data_providers: Vec<Arc<dyn DataProvider>>,
    /// Index of the currently active data provider.
    pub active_data_idx: usize,
}

impl HistoricalDataRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            data_providers: Vec::new(),
            active_data_idx: 0,
        }
    }

    /// Register a data provider. First registered becomes the default active.
    pub fn register_data_provider(&mut self, provider: Arc<dyn DataProvider>) {
        self.data_providers.push(provider);
    }

    /// Names of all registered data providers (for pick_list options).
    pub fn data_provider_names(&self) -> Vec<String> {
        self.data_providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    /// Get the currently active data provider, if any.
    pub fn active_data_provider(&self) -> Option<Arc<dyn DataProvider>> {
        self.data_providers.get(self.active_data_idx).cloned()
    }

    /// Display name of the active data provider.
    pub fn active_data_provider_name(&self) -> String {
        self.data_providers
            .get(self.active_data_idx)
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "None".to_string())
    }

    /// Set the active data provider by index.
    ///
    /// Returns `true` if the index was valid and the active provider changed.
    pub fn set_active_data(&mut self, idx: usize) -> bool {
        if idx >= self.data_providers.len() || idx == self.active_data_idx {
            return false;
        }
        self.active_data_idx = idx;
        true
    }

    /// Find a data provider's index by display name.
    pub fn find_data_provider_index(&self, name: &str) -> Option<usize> {
        self.data_providers.iter().position(|p| p.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_feed::TestProvider;

    #[test]
    fn registry_starts_empty() {
        let reg = HistoricalDataRegistry::new();
        assert!(reg.active_data_provider().is_none());
        assert_eq!(reg.data_provider_names().len(), 0);
    }

    #[test]
    fn registry_register_and_lookup() {
        let mut reg = HistoricalDataRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.data_provider_names(), vec!["Test Data".to_string()]);
        assert!(reg.active_data_provider().is_some());
    }

    #[test]
    fn registry_find_by_name() {
        let mut reg = HistoricalDataRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.find_data_provider_index("Test Data"), Some(0));
        assert_eq!(reg.find_data_provider_index("Unknown"), None);
    }

    #[test]
    fn registry_set_active_data() {
        let mut reg = HistoricalDataRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert!(!reg.set_active_data(5));
        assert!(!reg.set_active_data(0));
    }
}
