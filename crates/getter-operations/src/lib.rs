//! Reusable getter-owned operations shared by CLI and native embedders.
//!
//! This crate intentionally contains product/domain orchestration that must not
//! be duplicated in Flutter or Android/Kotlin bridge glue. Platform adapters may
//! provide raw facts to callers, but the rules for repository coverage,
//! generated package files, manifests, and tracked state live here.

pub mod app;
pub mod autogen;
pub mod download;
pub mod fdroid_autogen;
pub mod fdroid_catalog;
pub mod github_autogen;
pub mod github_latest_commit;
pub mod github_releases;
pub mod legacy_room;
#[cfg(feature = "lua")]
#[doc(hidden)]
pub mod lua_http_policy;
#[cfg(feature = "lua")]
#[doc(hidden)]
mod lua_provider_host;
#[cfg(feature = "lua")]
#[doc(hidden)]
pub mod lua_runtime_hooks;
pub mod provider_cache;
pub mod read_model;
pub mod runtime;
pub mod startup;
