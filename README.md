# UpgradeAll getter

Reusable Rust getter core for the UpgradeAll rewrite.

This repository is intentionally usable outside the UpgradeAll UI. The UpgradeAll app is a UI/platform adapter; package/repository evaluation, storage, migration mapping, update policy, and CLI behavior belong here.

## Current rewrite spine

- Rust workspace split into `getter-core`, `getter-storage`, `getter-cli`, and placeholder provider/downloader/RPC/FFI crates.
- Package IDs are readable, for example `android/org.fdroid.fdroid`.
- Lua package repositories use `repo.toml` plus `packages/`, `lib/`, and `templates/` directories.
- SQLite storage uses a main DB and a separate cache DB.
- `getter-cli` exposes JSON command contracts for init, app list, repository registration/evaluation, package evaluation, storage validation, legacy bridge-bundle import, and sanitized legacy report listing.

## Verify

```bash
cargo fmt --all --check
cargo test --workspace --lib --bins
cargo test -p getter-cli --test bdd_cli
cargo check --workspace --all-targets
```

The CI helper at `tests/script/cargo_test.sh` runs the same core checks plus feature-compatibility checks.
