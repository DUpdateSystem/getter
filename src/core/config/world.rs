pub mod world_list;
pub mod local_repo;
pub mod world_config_wrapper;

use once_cell::sync::Lazy;

use crate::{utils::instance::{InstanceContainer, InstanceGuard}, error::Result};

use self::world_list::WorldList;

static INSTANCE_CONTAINER: Lazy<InstanceContainer<WorldList>> =
    Lazy::new(|| InstanceContainer::new());

pub fn init_world_list(world_list_path: &str) -> Result<()> {
    INSTANCE_CONTAINER.init(WorldList::load(world_list_path)?);
    Ok(())
}

pub fn _get<'a>() -> InstanceGuard<'a, WorldList> {
    INSTANCE_CONTAINER.get()
}

#[macro_export]
macro_rules! get_world_list {
    () => {{
        $crate::core::config::world::_get()
            .get()
            .expect("World list is not initialized")
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_world_list_micro() {
        let world_list_path = "./test_get_world_list_micro";
        init_world_list(world_list_path).unwrap();
        let _ = get_world_list!();
    }
}
