# Getter - Multi-Package Update Management System

Get updates for everywhere with a modular, scalable architecture.

## 🏗️ Architecture

Getter has been refactored into a multi-package workspace structure for better modularity, testing, and multi-person collaboration:

### 📦 Package Structure

```
packages/
├── getter-utils/        # Core utilities (HTTP, versioning, time)
├── getter-cache/        # Caching system with pluggable backends
├── getter-provider/     # Update providers (GitHub, GitLab, F-Droid)
├── getter-config/       # Configuration and app tracking
├── getter-appmanager/   # Application management logic
├── getter-rpc/          # RPC server and client
├── getter-core/         # Core integration module
└── getter-cli/          # Command-line interface (RPC-based)
```

### 🎯 Key Features

- **Modular Design**: Each package has a single responsibility
- **RPC Architecture**: CLI communicates purely via RPC (ready for GUI)
- **Concurrent/Single-threaded**: Configurable via feature flags
- **Provider System**: Extensible support for multiple update sources
- **Caching**: Pluggable cache backends for performance
- **Backward Compatible**: Legacy APIs preserved during transition

## 🚀 Getting Started

### Prerequisites

- Rust 1.70+ with Cargo
- Tokio async runtime

### Building

Build the entire workspace:
```bash
cargo build --workspace
```

Build individual packages:
```bash
cargo build --package getter-core
cargo build --package getter-cli
```

Build with specific features:
```bash
# Build with concurrent cache backend
cargo build --package getter-cache --features concurrent

# Build with single-threaded backend
cargo build --package getter-cache --features single-threaded
```

### Testing

Run all tests:
```bash
cargo test --workspace
```

Run comprehensive test script:
```bash
./tests/script/cargo_test.sh
```

Run integration tests only:
```bash
cargo test --test integration_tests
```

### Usage

#### Core API

```rust
use getter_core::Core;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let core = Core::new();
    
    // Add an app to track
    let app_data = std::collections::HashMap::from([
        ("owner".to_string(), "rust-lang".to_string()),
        ("repo".to_string(), "rust".to_string()),
    ]);
    
    core.add_app(
        "rust".to_string(),
        "github".to_string(), 
        app_data,
        std::collections::HashMap::new()
    ).await?;
    
    // List all tracked apps
    let apps = core.list_apps().await?;
    println!("Tracked apps: {:?}", apps);
    
    // Get outdated apps
    let outdated = core.get_outdated_apps().await?;
    println!("Apps with updates: {:?}", outdated);
    
    Ok(())
}
```

#### RPC Server

```rust
use getter_rpc::GetterRpcServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = GetterRpcServer::new("127.0.0.1:8080").await?;
    server.serve().await?;
    Ok(())
}
```

#### RPC Client (CLI)

```rust
use getter_rpc::GetterRpcClient;
use serde_json::json;

#[tokio::main]  
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = GetterRpcClient::new("http://localhost:8080")?;
    
    let app_data = json!({
        "app_id": "rust",
        "hub_uuid": "github",
        "app_data": {"owner": "rust-lang", "repo": "rust"},
        "hub_data": {}
    });
    
    let result = client.add_app(app_data).await?;
    println!("Added app: {:?}", result);
    
    Ok(())
}
```

## 🔧 Development

### Package Dependencies

The packages have the following dependency relationships:

```
getter-cli -> getter-rpc -> getter-core -> {getter-appmanager, getter-config}
getter-appmanager -> {getter-provider, getter-config, getter-cache}
getter-provider -> getter-utils
getter-config -> getter-utils
```

### Adding New Providers

1. Implement the `BaseProvider` trait in `getter-provider`
2. Register the provider in the `ProviderManager`
3. Add provider-specific configuration to `getter-config`

### Feature Flags

- `concurrent`: Enable concurrent cache backend (default)
- `single-threaded`: Enable single-threaded cache backend
- `rustls-platform-verifier-android`: Android-specific TLS verifier

## 📊 Performance

The new architecture provides:

- **Request Deduplication**: Multiple identical requests are automatically deduplicated
- **Memory Efficiency**: Background processing with automatic cleanup
- **Async Processing**: Non-blocking operations throughout
- **Caching**: Configurable caching strategies for API responses

## 🤝 Contributing

1. Each package has its own tests - run them individually during development
2. Integration tests ensure packages work together correctly
3. Use the provided test script for comprehensive validation
4. Follow the existing error handling patterns (Result types)

## 📄 License

MIT License - see LICENSE file for details.
