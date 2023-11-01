mod provider;
mod utils;

use std::collections::HashMap;
use tokio::runtime::Runtime;

use provider::github::GithubProvider;
use provider::base_provider::BaseProvider;

pub fn get_a() -> bool {
    let rt = Runtime::new().unwrap();
    let provider = GithubProvider;

    // 这里我们假设 check_app_available 是 GithubProvider 的一个异步方法
    let mut id_map = HashMap::new();
    id_map.insert("available".to_string(), "true".to_string());
    let check_app_result = rt.block_on(provider.check_app_available(&id_map));
    check_app_result
}
