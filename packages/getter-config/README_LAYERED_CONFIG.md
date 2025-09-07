# Layered Configuration System

## Overview

The new configuration system in `getter-config` implements a layered approach similar to Gentoo's Portage overlay system. This allows for:

- **Community-maintained defaults** (upstream/repo layer)
- **Local customizations** (config layer)
- **Easy sharing** of configurations
- **Human-readable identifiers** (`app-id::hub-id` format)

## Directory Structure

```
data/
├── repo/                    # Upstream configurations (community maintained)
│   ├── apps/               # Application definitions
│   │   ├── rust.json       # Rust compiler configuration
│   │   ├── firefox.json    # Firefox browser configuration
│   │   └── ...
│   └── hubs/               # Repository/hub definitions
│       ├── github.json     # GitHub provider configuration
│       ├── gitlab.json     # GitLab provider configuration
│       └── ...
└── config/                 # Local configurations
    ├── app_list           # List of tracked apps (app-id::hub-id format)
    ├── tracking.json      # Version tracking information
    ├── apps/              # Local app configuration overrides
    │   └── rust.json      # Override specific settings for Rust
    └── hubs/              # Local hub configuration overrides
        └── github.json    # Override GitHub settings
```

## Key Concepts

### 1. App Identifiers

Apps are identified using the format: `app-id::hub-id`

Examples:
- `rust::github` - Rust compiler from GitHub
- `firefox::mozilla` - Firefox from Mozilla's repository
- `neovim::github` - Neovim editor from GitHub

### 2. Configuration Merging

Configurations are merged using JSON Merge Patch (RFC 7386):

**Upstream (repo/apps/rust.json):**
```json
{
  "name": "rust",
  "metadata": {
    "repo": "rust-lang/rust",
    "check_interval": 3600,
    "tags": ["compiler", "language"]
  }
}
```

**Local Override (config/apps/rust.json):**
```json
{
  "metadata": {
    "check_interval": 7200,
    "custom_flag": true
  }
}
```

**Result (Merged):**
```json
{
  "name": "rust",
  "metadata": {
    "repo": "rust-lang/rust",
    "check_interval": 7200,        // Overridden
    "tags": ["compiler", "language"],
    "custom_flag": true             // Added
  }
}
```

### 3. App List

The `config/app_list` file contains tracked applications:

```
rust::github
firefox::mozilla
neovim::github
# Comments are supported
vscode::microsoft
```

## API Usage

### Basic Usage

```rust
use getter_config::{AppRegistry, AppConfig, HubConfig};

// Initialize registry
let mut registry = AppRegistry::new("/path/to/data")?;

// Add an app to tracking
registry.add_app("rust::github")?;

// Get merged configuration
let app_config = registry.get_app_config("rust")?;
let hub_config = registry.get_hub_config("github")?;

// List all tracked apps
let apps = registry.list_apps();
```

### Creating Configurations

```rust
// Create app configuration
let app = AppConfig {
    name: "myapp".to_string(),
    metadata: HashMap::from([
        ("repo".to_string(), json!("owner/repo")),
    ]),
};

// Save to repo (upstream) or config (local)
registry.save_app_config("myapp", &app, true)?;  // true = repo
registry.save_app_config("myapp", &app, false)?; // false = config
```

## Sharing Configurations

### Sharing Your App List

1. Share your `config/app_list` file
2. Others can copy it to their `config/` directory
3. They'll track the same applications

### Sharing Custom Configurations

1. Share specific files from `config/apps/` or `config/hubs/`
2. Recipients place them in their corresponding directories
3. Their local overrides will apply to the upstream defaults

### Community Repository

The `repo/` directory can be:
- Maintained in a git repository
- Updated independently of local configurations
- Shared across teams or communities

## Migration from Old System

The old system used:
- UUIDs for identification
- Single-file storage
- No layering support

The new system provides:
- Human-readable identifiers
- Layered configuration
- Better sharing capabilities
- Compatibility with the old API through AppManager

## Comparison with Cloud Rules

The old `cloud_rules` system has been removed. The new layered configuration provides similar functionality with better structure:

| Old (cloud_rules) | New (layered_config) |
|-------------------|---------------------|
| Cloud-based rules | Repo layer (upstream) |
| Local overrides | Config layer (local) |
| UUID identifiers | app-id::hub-id format |
| Single source | Multiple merge sources |

## Testing

Run the example to see the system in action:

```bash
cargo run --example layered_config_example
```

Run tests:

```bash
cargo test app_registry
cargo test layered_config
```