//! Provider implementations
//!
//! This module contains all provider implementations. Each provider should:
//! 1. Implement the `BaseProvider` trait
//! 2. Have a `Default` implementation
//! 3. Use `register_provider!(ProviderName)` at the end of the file
//!
//! Adding a new provider is as simple as creating a new file in this directory!
//!
//! See `README.md` in this directory for detailed instructions.

pub mod fdroid;
pub mod github;
pub mod gitlab;
pub mod lsposed;

// Re-export all providers for convenience
pub use fdroid::FDroidProvider;
pub use github::GitHubProvider;
pub use gitlab::GitLabProvider;
pub use lsposed::LsposedRepoProvider;
