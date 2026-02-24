use std::collections::HashMap;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::database::models::app::AppRecord;

/// Global Android API client, registered via `register_android_api` RPC.
static ANDROID_API: OnceCell<AndroidApi> = OnceCell::new();

pub fn set_android_api(url: String) {
    let _ = ANDROID_API.set(AndroidApi { url });
}

pub fn get_android_api() -> Option<&'static AndroidApi> {
    ANDROID_API.get()
}

/// Rust-side HTTP JSON-RPC client that calls back into the Kotlin
/// KotlinHubRpcServer for Android-specific functionality.
///
/// Kotlin exposes `get_local_version` and `get_installed_apps` methods
/// on the same Ktor HTTP server that handles hub provider calls.
pub struct AndroidApi {
    url: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetLocalVersionParams {
    app_id: HashMap<String, Option<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetInstalledAppsParams {
    ignore_system: bool,
}

impl AndroidApi {
    /// Query Kotlin for the locally-installed version of an app.
    ///
    /// Calls `get_local_version({app_id})` on the Kotlin Ktor server.
    /// Returns `None` if the app is not installed or the call fails.
    pub async fn get_local_version(
        &self,
        app_id: &HashMap<String, Option<String>>,
    ) -> Option<String> {
        let client = HttpClientBuilder::default().build(&self.url).ok()?;
        let params = rpc_params!(GetLocalVersionParams {
            app_id: app_id.clone(),
        });
        client
            .request::<Option<String>, _>("get_local_version", params)
            .await
            .unwrap_or(None)
    }

    /// Query Kotlin for all installed Android apps and Magisk modules.
    ///
    /// Calls `get_installed_apps({ignore_system})` on the Kotlin Ktor server.
    /// Returns an empty list if the call fails.
    pub async fn get_installed_apps(&self, ignore_system: bool) -> Vec<AppRecord> {
        let client = match HttpClientBuilder::default().build(&self.url) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let params = rpc_params!(GetInstalledAppsParams { ignore_system });
        client
            .request::<Vec<AppRecord>, _>("get_installed_apps", params)
            .await
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_local_version_params_serialization() {
        let app_id: HashMap<String, Option<String>> = HashMap::from([(
            "android_app_package".to_string(),
            Some("com.example.app".to_string()),
        )]);
        let params = GetLocalVersionParams {
            app_id: app_id.clone(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("android_app_package"));
        assert!(json.contains("com.example.app"));
    }

    #[test]
    fn test_get_installed_apps_params_serialization() {
        let params = GetInstalledAppsParams {
            ignore_system: true,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("ignore_system"));
        assert!(json.contains("true"));
    }
}
