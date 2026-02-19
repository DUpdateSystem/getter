//! Hub-based download dispatcher
//!
//! Routes download tasks to registered external downloaders based on hub_uuid.
//! If no external downloader is registered for the given hub_uuid, falls back
//! to the default built-in downloader (TraumaDownloader / HTTP).

use super::error::Result;
use super::traits::{Downloader, DownloaderCapabilities, ProgressCallback, RequestOptions};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Shared internal state for HubDispatchDownloader.
///
/// Wrapped in Arc so that clones are cheap and share the same state.
/// This allows the RPC server to hold a reference for register/unregister
/// while DownloadTaskManager holds another reference as Box<dyn Downloader>.
struct HubDispatchState {
    /// hub_uuid -> external downloader
    external: RwLock<HashMap<String, Arc<Box<dyn Downloader>>>>,
    /// Default downloader for unregistered hub_uuids (TraumaDownloader)
    default: Arc<Box<dyn Downloader>>,
    /// Active task tracking: url -> hub_uuid (for routing cancel/pause/resume)
    active_tasks: RwLock<HashMap<String, Option<String>>>,
}

/// Downloads dispatcher that routes tasks by hub_uuid.
///
/// - If `hub_uuid` is provided (via `RequestOptions.metadata["hub_uuid"]`)
///   and a downloader is registered for it, the task is routed there.
/// - Otherwise, the default built-in HTTP downloader handles it.
///
/// This struct is cheaply cloneable (Arc-based shared state).
pub struct HubDispatchDownloader {
    state: Arc<HubDispatchState>,
}

impl HubDispatchDownloader {
    /// Create a new dispatcher with the given default downloader.
    pub fn new(default: Box<dyn Downloader>) -> Self {
        Self {
            state: Arc::new(HubDispatchState {
                external: RwLock::new(HashMap::new()),
                default: Arc::new(default),
                active_tasks: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Register an external downloader for a specific hub_uuid.
    pub fn register(&self, hub_uuid: &str, downloader: Box<dyn Downloader>) {
        self.state
            .external
            .write()
            .insert(hub_uuid.to_string(), Arc::new(downloader));
    }

    /// Unregister the external downloader for a specific hub_uuid.
    pub fn unregister(&self, hub_uuid: &str) {
        self.state.external.write().remove(hub_uuid);
    }

    /// Check if a downloader is registered for the given hub_uuid.
    pub fn has_downloader(&self, hub_uuid: &str) -> bool {
        self.state.external.read().contains_key(hub_uuid)
    }

    /// Resolve which downloader to use based on hub_uuid.
    fn resolve(&self, hub_uuid: Option<&str>) -> Arc<Box<dyn Downloader>> {
        if let Some(uuid) = hub_uuid {
            if let Some(dl) = self.state.external.read().get(uuid) {
                return dl.clone();
            }
        }
        self.state.default.clone()
    }

    /// Extract hub_uuid from RequestOptions metadata.
    fn extract_hub_uuid(options: Option<&RequestOptions>) -> Option<String> {
        options
            .and_then(|o| o.metadata.as_ref())
            .and_then(|m| m.get("hub_uuid"))
            .cloned()
    }

    /// Record an active task for url -> hub_uuid routing.
    fn track_task(&self, url: &str, hub_uuid: Option<String>) {
        self.state
            .active_tasks
            .write()
            .insert(url.to_string(), hub_uuid);
    }

    /// Remove an active task record.
    fn untrack_task(&self, url: &str) {
        self.state.active_tasks.write().remove(url);
    }

    /// Look up which hub_uuid (if any) an active URL belongs to.
    fn lookup_task_hub(&self, url: &str) -> Option<Option<String>> {
        self.state.active_tasks.read().get(url).cloned()
    }
}

impl Clone for HubDispatchDownloader {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

#[async_trait]
impl Downloader for HubDispatchDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<ProgressCallback>,
        options: Option<RequestOptions>,
    ) -> Result<()> {
        let hub_uuid = Self::extract_hub_uuid(options.as_ref());
        let downloader = self.resolve(hub_uuid.as_deref());

        // Track this task so cancel/pause/resume can find the right downloader
        self.track_task(url, hub_uuid);

        let result = downloader.download(url, dest, progress, options).await;

        // Clean up tracking on completion (success or failure)
        self.untrack_task(url);

        result
    }

    async fn download_batch(&self, tasks: Vec<(String, PathBuf)>) -> Vec<Result<()>> {
        // Batch downloads go through the default downloader since there's
        // no per-task metadata available in the batch interface.
        self.state.default.download_batch(tasks).await
    }

    fn name(&self) -> &str {
        "hub_dispatch"
    }

    fn capabilities(&self) -> &DownloaderCapabilities {
        // Return default backend capabilities
        self.state.default.capabilities()
    }

    async fn cancel(&self, url: &str) -> Result<()> {
        let hub_uuid = self.lookup_task_hub(url).flatten();
        self.resolve(hub_uuid.as_deref()).cancel(url).await
    }

    async fn pause(&self, url: &str) -> Result<()> {
        let hub_uuid = self.lookup_task_hub(url).flatten();
        self.resolve(hub_uuid.as_deref()).pause(url).await
    }

    async fn resume(&self, url: &str) -> Result<()> {
        let hub_uuid = self.lookup_task_hub(url).flatten();
        self.resolve(hub_uuid.as_deref()).resume(url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::TraumaDownloader;

    #[test]
    fn test_create_dispatcher() {
        let default = Box::new(TraumaDownloader::default_settings());
        let dispatcher = HubDispatchDownloader::new(default);
        assert_eq!(dispatcher.name(), "hub_dispatch");
        assert!(!dispatcher.has_downloader("some-uuid"));
    }

    #[test]
    fn test_register_unregister() {
        let default = Box::new(TraumaDownloader::default_settings());
        let dispatcher = HubDispatchDownloader::new(default);

        let ext = Box::new(TraumaDownloader::default_settings());
        dispatcher.register("test-uuid", ext);
        assert!(dispatcher.has_downloader("test-uuid"));

        dispatcher.unregister("test-uuid");
        assert!(!dispatcher.has_downloader("test-uuid"));
    }

    #[test]
    fn test_clone_shares_state() {
        let default = Box::new(TraumaDownloader::default_settings());
        let dispatcher = HubDispatchDownloader::new(default);

        let clone = dispatcher.clone();

        let ext = Box::new(TraumaDownloader::default_settings());
        dispatcher.register("shared-uuid", ext);

        // Clone should see the registration
        assert!(clone.has_downloader("shared-uuid"));
    }
}
