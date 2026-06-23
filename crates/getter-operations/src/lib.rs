//! Reusable getter-owned operations shared by CLI and native embedders.
//!
//! This crate intentionally contains product/domain orchestration that must not
//! be duplicated in Flutter or Android/Kotlin bridge glue. Platform adapters may
//! provide raw facts to callers, but the rules for repository coverage,
//! generated package files, manifests, and tracked state live here.

pub mod autogen;
