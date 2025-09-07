# Cloud Configuration Compatibility

## Overview

The new layered configuration system maintains full compatibility with existing cloud configurations while providing enhanced functionality through a Gentoo Portage-like overlay system.

## Architecture

```
┌─────────────────────────────┐
│     Cloud Repository        │
│  (GitHub/Remote Server)     │
│                             │
│  cloud_config.json          │
│  - UUID-based configs       │
│  - Legacy format            │
└──────────────┬──────────────┘
               │ Sync
               ▼
┌─────────────────────────────┐
│      Local repo/ Layer      │
│   (Synced from cloud)       │
│                             │
│  apps/*.json                │
│  hubs/*.json                │
│  uuid_mapping.json          │
└──────────────┬──────────────┘
               │ Merge
               ▼
┌─────────────────────────────┐
│     Local config/ Layer     │
│    (User customizations)    │
│                             │
│  app_list                   │
│  apps/*.json (overrides)    │
│  hubs/*.json (overrides)    │
└─────────────────────────────┘
```

## Cloud Configuration Format

The system supports the existing cloud configuration format:

```json
{
  "app_config_list": [
    {
      "base_version": 2,
      "config_version": 1,
      "uuid": "f27f71e1-d7a1-4fd1-bbcc-9744380611a1",
      "base_hub_uuid": "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
      "info": {
        "name": "UpgradeAll",
        "url": "https://github.com/DUpdateSystem/UpgradeAll",
        "extra_map": {
          "android_app_package": "net.xzos.upgradeall"
        }
      }
    }
  ],
  "hub_config_list": [
    {
      "base_version": 6,
      "config_version": 3,
      "uuid": "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
      "info": {
        "hub_name": "GitHub",
        "hub_icon_url": ""
      },
      "api_keywords": ["owner", "repo"],
      "app_url_templates": ["https://github.com/%owner/%repo/"]
    }
  ]
}
```

## UUID to Human-Readable Mapping

The system automatically converts UUIDs to human-readable identifiers:

| UUID | Human-Readable ID | Full Identifier |
|------|-------------------|-----------------|
| f27f71e1-d7a1-4fd1-bbcc-9744380611a1 | upgradeall | upgradeall::github |
| ec2f237e-a502-4a1c-864b-3b64eaa75303 | apkgrabber | apkgrabber::github |
| fd9b2602-62c5-4d55-bd1e-0d6537714ca0 | github | (hub) |
| 65c2f60c-7d08-48b8-b4ba-ac6ee924f6fa | google-play | (hub) |

## Synchronization Process

### 1. Fetch from Cloud

```rust
use getter_config::{AppRegistry, cloud_sync::CloudSync};

// Sync from cloud URL
let mut registry = AppRegistry::new("/path/to/data")?;
registry.sync_from_cloud("https://example.com/cloud_config.json").await?;
```

### 2. Import and Track Apps

```rust
// Import all apps from cloud and add to tracking
let imported = registry.import_cloud_apps("https://example.com/cloud_config.json").await?;
println!("Imported {} apps", imported.len());
```

### 3. Manual Sync

```rust
let mut cloud_sync = CloudSync::with_url("https://example.com/cloud_config.json".to_string());
let cloud_config = cloud_sync.fetch_cloud_config().await?;
cloud_sync.sync_to_repo(&Path::new("/data/repo")).await?;
```

## Data Preservation

The system preserves all cloud configuration data:

### Original UUID
```json
{
  "metadata": {
    "uuid": "f27f71e1-d7a1-4fd1-bbcc-9744380611a1",
    "base_hub_uuid": "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
  }
}
```

### Version Information
```json
{
  "metadata": {
    "base_version": 2,
    "config_version": 1
  }
}
```

### Extra Fields
All fields from `extra_map` are preserved in the metadata.

## Backward Compatibility

1. **UUID Lookup**: The system maintains a `uuid_mapping.json` file for UUID to name resolution
2. **Legacy Support**: Old systems can still use UUIDs through the mapping
3. **Data Integrity**: All original cloud configuration data is preserved

## Migration Path

### For Existing Users

1. Your existing cloud configurations continue to work
2. Run sync to pull cloud configs into the new repo layer
3. Optionally add local overrides in the config layer
4. Use human-readable identifiers going forward

### For New Users

1. Start with human-readable identifiers
2. Cloud configs automatically convert to the new format
3. Local customizations are kept separate

## Cloud URLs

The system can sync from various sources:

- GitHub: `https://raw.githubusercontent.com/USER/REPO/BRANCH/cloud_config.json`
- Custom Server: `https://your-server.com/api/cloud_config.json`
- Local File: `file:///path/to/cloud_config.json`

## Benefits

1. **Compatibility**: Full support for existing cloud configurations
2. **Readability**: Human-readable identifiers instead of UUIDs
3. **Layering**: Cloud configs (repo) + local overrides (config)
4. **Flexibility**: Can sync from multiple cloud sources
5. **Preservation**: All original data is preserved
6. **Sharing**: Easy to share configurations with others

## Testing

Run examples to see the system in action:

```bash
# Cloud sync example
cargo run --example cloud_sync_example

# Layered config example
cargo run --example layered_config_example
```

## Future Enhancements

- Automatic periodic sync from cloud
- Multiple cloud source support
- Conflict resolution strategies
- Cloud config versioning
- Differential sync (only changed items)