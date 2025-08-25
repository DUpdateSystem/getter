# Provider Implementations

This directory contains all provider implementations for the getter system.

## How to Add a New Provider

Adding a new provider is simple! Just create a new file in this directory following this template:

```rust
// src/providers/my_new_provider.rs

use async_trait::async_trait;
use crate::base_provider::{BaseProvider, FIn, FOut, FunctionType, DataMap};
use crate::data::ReleaseData;
use crate::register_provider;

/// Provider for MyNewService
pub struct MyNewServiceProvider;

impl Default for MyNewServiceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MyNewServiceProvider {
    pub fn new() -> Self {
        MyNewServiceProvider
    }
}

#[async_trait]
impl BaseProvider for MyNewServiceProvider {
    fn get_uuid(&self) -> &'static str {
        "your-unique-uuid-here"
    }

    fn get_friendly_name(&self) -> &'static str {
        "my_service" // This becomes the provider key
    }

    fn get_cache_request_key(&self, function_type: &FunctionType, data_map: &DataMap<'_>) -> Vec<String> {
        // Generate cache keys based on the request
        vec![format!("my_service:{}", "some_identifier")]
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        // Implement availability check logic
        FOut::new(true)
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        // Implement latest release fetching
        FOut::new_empty()
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        // Implement releases fetching
        FOut::new(vec![])
    }
}

// 🎉 This single line registers the provider automatically!
register_provider!(MyNewServiceProvider);
```

## Steps:

1. **Create the file**: `src/providers/my_new_provider.rs`
2. **Add to mod.rs**: Add your provider to `mod.rs` in this directory
3. **Export in lib.rs**: Add your provider to the main lib.rs exports
4. **That's it!** Your provider will be automatically registered

## Current Providers

- **GitHub** (`github.rs`) - GitHub releases and packages
- **GitLab** (`gitlab.rs`) - GitLab releases (example implementation)

## Notes

- The `get_friendly_name()` return value is used as the provider key
- Each provider must implement `Default` 
- Use `register_provider!(YourProvider)` at the end of each file
- The system automatically discovers and registers all providers at startup