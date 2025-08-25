pub mod rule_list;
pub mod utils;
pub mod world_list;

use once_cell::sync::Lazy;
use std::error::Error;
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;

use crate::world_list::WorldList;
use getter_utils::instance::InstanceContainer;

// Re-export commonly used types
pub use rule_list::{RuleList, TrackedApp};
pub use utils::{all_dir, get_data_path, DataDir};
pub use world_list::WORLD_CONFIG_LIST_NAME;

static INSTANCE_CONTAINER: Lazy<InstanceContainer<WorldList>> =
    Lazy::new(|| InstanceContainer::new(WorldList::new()));

pub async fn init_world_list(world_list_path: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    get_world_list().await.lock().await.load(world_list_path)?;
    Ok(())
}

pub async fn get_world_list() -> Arc<Mutex<WorldList>> {
    INSTANCE_CONTAINER.get().await.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_world_list_micro() {
        let world_list_path = Path::new("./test_get_world_list_micro");
        init_world_list(world_list_path).await.unwrap();
        let _ = get_world_list().await;
    }
}
