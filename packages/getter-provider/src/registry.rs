use crate::base_provider::BaseProvider;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Provider registry for automatic provider registration
pub struct ProviderRegistry {
    providers: Vec<Box<dyn Fn() -> Box<dyn BaseProvider> + Send + Sync>>,
}

impl ProviderRegistry {
    fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Get the global provider registry
    pub fn global() -> &'static Mutex<Self> {
        static REGISTRY: OnceLock<Mutex<ProviderRegistry>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(ProviderRegistry::new()))
    }

    /// Register a provider factory function
    pub fn register<P: BaseProvider + Default + 'static>(&mut self) {
        self.providers.push(Box::new(|| Box::new(P::default())));
    }

    /// Create all registered providers
    pub fn create_all(&self) -> HashMap<String, Box<dyn BaseProvider>> {
        let mut providers = HashMap::new();
        for factory in &self.providers {
            let provider = factory();
            let name = provider.get_friendly_name().to_string();
            providers.insert(name, provider);
        }
        providers
    }
}

/// Macro to automatically register a provider
#[macro_export]
macro_rules! register_provider {
    ($provider_type:ty) => {
        // Use a module-local static to ensure registration happens
        #[used]
        #[allow(non_upper_case_globals)]
        static _register: fn() = || {
            $crate::registry::ProviderRegistry::global()
                .lock()
                .unwrap()
                .register::<$provider_type>();
        };

        // Call the registration function in a constructor
        #[ctor::ctor]
        fn register() {
            _register();
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_provider::{BaseProvider, DataMap, FIn, FOut, FunctionType};
    use crate::data::ReleaseData;
    use async_trait::async_trait;

    // Test provider for testing the registry
    pub struct TestProvider;

    impl Default for TestProvider {
        fn default() -> Self {
            TestProvider
        }
    }

    #[async_trait]
    impl BaseProvider for TestProvider {
        fn get_uuid(&self) -> &'static str {
            "test-uuid"
        }

        fn get_friendly_name(&self) -> &'static str {
            "test"
        }

        fn get_cache_request_key(
            &self,
            _function_type: &FunctionType,
            _data_map: &DataMap<'_>,
        ) -> Vec<String> {
            vec!["test-key".to_string()]
        }

        async fn check_app_available(&self, _fin: &FIn) -> FOut<bool> {
            FOut::new(true)
        }

        async fn get_latest_release(&self, _fin: &FIn) -> FOut<ReleaseData> {
            FOut::new_empty()
        }

        async fn get_releases(&self, _fin: &FIn) -> FOut<Vec<ReleaseData>> {
            FOut::new(vec![])
        }
    }

    #[test]
    fn test_manual_registration() {
        let mut registry = ProviderRegistry {
            providers: Vec::new(),
        };
        registry.register::<TestProvider>();

        let providers = registry.create_all();
        assert_eq!(providers.len(), 1);
        assert!(providers.contains_key("test"));
    }
}
