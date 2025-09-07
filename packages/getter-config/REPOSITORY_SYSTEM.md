# Multi-Repository Configuration System

## Overview

The getter-config package now implements a Gentoo Portage-like multi-repository system with overlay support, providing:

- Multiple configuration sources (repositories)
- Priority-based overlay merging
- Full backward compatibility with existing cloud configurations
- Human-readable app identifiers (`app-id::hub-id`)

## Features

### 1. Repository Management
- Multiple repositories with different priorities
- Enable/disable repositories
- Local and remote (cloud) repository support
- Automatic synchronization from cloud sources

### 2. Configuration Layering
```
Priority (High to Low):
- Local config (user overrides)
- Local repository (100)
- Community repository (50)
- Testing repository (25)
- Main repository (0)
```

### 3. Cloud Compatibility
- Supports existing UUID-based cloud configurations
- Automatic conversion to human-readable names
- Preserves all metadata (UUIDs, versions, extra fields)

## Directory Structure

```
data/
├── repos.conf              # Repository configuration
├── repos/                  # Repository data
│   ├── getter-main/        # Official repository
│   │   ├── apps/
│   │   │   ├── upgradeall.json
│   │   │   └── ...
│   │   └── hubs/
│   │       ├── github.json
│   │       └── ...
│   ├── community/          # Community overlays
│   └── local/              # Local repository
└── config/                 # User overrides
    ├── app_list            # Tracked apps
    ├── apps/               # App overrides
    └── hubs/               # Hub overrides
```

## Usage

### Initialize Repository Manager
```rust
use getter_config::repository::RepositoryManager;

let mut repo_manager = RepositoryManager::new(data_path)?;
repo_manager.init_default_repositories()?;
```

### Add Custom Repository
```rust
repo_manager.add_repository(
    "community".to_string(),
    Some("https://example.com/community/cloud_config.json".to_string()),
    50  // priority
)?;
```

### Sync from Cloud
```rust
let mut registry = AppRegistry::new(data_path)?;
registry.sync_from_cloud("https://example.com/cloud_config.json").await?;
```

### Use Human-Readable Identifiers
```rust
// Instead of UUID-based:
// "f27f71e1-d7a1-4fd1-bbcc-9744380611a1"

// Use human-readable:
registry.add_app("upgradeall::github")?;
```

## Testing with Real Data

The system has been tested with actual cloud configuration data containing:
- 346 real app configurations
- 13 hub configurations
- Apps from GitHub, Google Play, F-droid, GitLab, and more

### Test Files
- `tests/files/cloud_config.json` - Actual cloud configuration with 346 apps
- `tests/cloud_sync_real_data_test.rs` - Comprehensive tests using real data
- `tests/multi_repo_test.rs` - Multi-repository overlay tests

### Running Tests
```bash
# Run all tests
cargo test

# Run tests with real cloud data
cargo test --test cloud_sync_real_data_test

# Run multi-repository tests
cargo test --test multi_repo_test
```

## Configuration Merging

The system uses JSON Merge Patch (RFC 7386) for configuration merging:

1. Load base configuration from lowest priority repository
2. Apply overlays from higher priority repositories
3. Apply local user overrides
4. Result: Merged configuration with all customizations

Example:
```json
// Main repository (priority 0)
{
  "name": "MyApp",
  "version": "1.0.0",
  "metadata": {
    "description": "Original app"
  }
}

// Community overlay (priority 50)
{
  "metadata": {
    "community_version": "2.0.0",
    "enhanced": true
  }
}

// Result after merging
{
  "name": "MyApp",
  "version": "1.0.0",
  "metadata": {
    "description": "Original app",
    "community_version": "2.0.0",
    "enhanced": true
  }
}
```

## Benefits

1. **Multiple Sources**: Support for official, community, and local repositories
2. **Easy Sharing**: Human-readable configurations that can be shared
3. **Customization**: Local overrides preserved across updates
4. **Compatibility**: Full backward compatibility with existing cloud configs
5. **Flexibility**: Enable/disable repositories, adjust priorities
6. **Transparency**: Clear overlay system shows where configurations come from

## Examples

See the examples directory for complete demonstrations:
- `multi_repo_example.rs` - Multi-repository system demonstration
- `cloud_sync_example.rs` - Cloud synchronization example
- `layered_config_example.rs` - Configuration layering example