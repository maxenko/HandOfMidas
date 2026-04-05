//! Central registry of all available data providers and order brokers.
//!
//! Owned by `MidasApp` (single iced update thread). Providers are `Arc`'d
//! so they can be cloned into `Task::perform` closures for async operations.

use std::sync::Arc;

use midas_core::provider::{DataProvider, OrderBroker};

/// Manages available data providers and order brokers.
///
/// Holds a list of registered providers and tracks which one is active.
/// Provider switching triggers chart reloads in the app layer.
///
/// After initialization (at least one `register_data_provider` call),
/// `active_data_idx` is a valid index into `data_providers`. Before any
/// provider is registered, `active_data_provider()` returns `None`.
pub struct ProviderRegistry {
    /// Registered data providers, in display order.
    data_providers: Vec<Arc<dyn DataProvider>>,
    /// Index of the currently active data provider.
    pub active_data_idx: usize,
    /// Registered order brokers, in display order.
    order_brokers: Vec<Arc<dyn OrderBroker>>,
    /// Index of the currently active order broker, or None for "None".
    pub active_broker_idx: Option<usize>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            data_providers: Vec::new(),
            active_data_idx: 0,
            order_brokers: Vec::new(),
            active_broker_idx: None,
        }
    }

    /// Register a data provider. First registered becomes the default active.
    pub fn register_data_provider(&mut self, provider: Arc<dyn DataProvider>) {
        self.data_providers.push(provider);
    }

    /// Register an order broker.
    #[allow(dead_code)]
    pub fn register_order_broker(&mut self, broker: Arc<dyn OrderBroker>) {
        self.order_brokers.push(broker);
    }

    /// Names of all registered data providers (for pick_list options).
    pub fn data_provider_names(&self) -> Vec<String> {
        self.data_providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    /// Names of all registered order brokers, with "None" prepended.
    pub fn order_broker_names(&self) -> Vec<String> {
        let mut names = vec!["None".to_string()];
        for b in &self.order_brokers {
            names.push(b.name().to_string());
        }
        names
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

    /// Get the currently active order broker, if any.
    pub fn active_broker(&self) -> Option<Arc<dyn OrderBroker>> {
        self.active_broker_idx
            .and_then(|idx| self.order_brokers.get(idx).cloned())
    }

    /// Display name of the active broker, or `"None"` if no broker is active.
    pub fn active_broker_display_name(&self) -> String {
        self.active_broker_idx
            .and_then(|idx| self.order_brokers.get(idx))
            .map(|b| b.name().to_string())
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

    /// Set the active order broker by index.
    ///
    /// Pass `None` to disconnect from all brokers.
    pub fn set_active_broker(&mut self, idx: Option<usize>) -> bool {
        if let Some(i) = idx {
            if i >= self.order_brokers.len() {
                return false;
            }
        }
        if idx == self.active_broker_idx {
            return false;
        }
        self.active_broker_idx = idx;
        true
    }

    /// Find a data provider's index by display name.
    pub fn find_data_provider_index(&self, name: &str) -> Option<usize> {
        self.data_providers.iter().position(|p| p.name() == name)
    }

    /// Find a broker's index by display name.
    pub fn find_broker_index(&self, name: &str) -> Option<usize> {
        self.order_brokers.iter().position(|b| b.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_feed::TestProvider;

    #[test]
    fn registry_starts_empty() {
        let reg = ProviderRegistry::new();
        assert!(reg.active_data_provider().is_none());
        assert_eq!(reg.data_provider_names().len(), 0);
    }

    #[test]
    fn registry_register_and_lookup() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.data_provider_names(), vec!["Test Data".to_string()]);
        assert!(reg.active_data_provider().is_some());
    }

    #[test]
    fn registry_find_by_name() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.find_data_provider_index("Test Data"), Some(0));
        assert_eq!(reg.find_data_provider_index("Unknown"), None);
    }

    #[test]
    fn registry_broker_none_by_default() {
        let reg = ProviderRegistry::new();
        assert!(reg.active_broker().is_none());
        assert_eq!(reg.order_broker_names(), vec!["None".to_string()]);
    }

    #[test]
    fn registry_set_active_data() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert!(!reg.set_active_data(5));
        assert!(!reg.set_active_data(0));
    }
}
